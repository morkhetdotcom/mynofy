package dev.mynofy;

import com.google.protobuf.ByteString;
import com.google.protobuf.Message;
import dev.mynofy.protocol.Wire;
import java.io.*;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.security.*;
import java.time.*;
import java.time.format.DateTimeFormatter;
import java.util.Arrays;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/** One in-flight operation per client, without retries, workflow state or automatic polling. */
public final class Client implements AutoCloseable {
  public record Config(
      URI url, Path socket, String owner, Key key, Path caFile, String clientId, Duration timeout) {
    public Config {
      name(owner, "owner");
      if (key == null || (url == null) == (socket == null))
        throw new IllegalArgumentException("configure exactly one transport and a key");
      if (timeout == null) timeout = Duration.ofSeconds(10);
      if (timeout.isNegative() || timeout.isZero())
        throw new IllegalArgumentException("timeout must be positive");
      timeout.toNanos(); // Reject unrepresentable durations before connecting.
      if (url != null
          && (!"wss".equals(url.getScheme())
              || url.getHost() == null
              || url.getUserInfo() != null
              || url.getRawFragment() != null))
        throw new IllegalArgumentException("expected wss:// URI without credentials or fragment");
      if (clientId == null) clientId = key.clientId();
      name(clientId, "client ID");
    }

    public static Config unix(Path socket, String owner, Key key) {
      return new Config(null, socket, owner, key, null, null, null);
    }

    public static Config wss(URI url, String owner, Key key, Path ca) {
      return new Config(url, null, owner, key, ca, null, null);
    }
  }

  static String validatedBootId(String value) throws IOException {
    if (value != null) {
      try {
        Key.decode(value, 16);
      } catch (IllegalArgumentException error) {
        throw new IOException("invalid owner boot identity", error);
      }
    }
    return value;
  }

  public record ConnectionInfo(String owner, String clientId, int restartCounter, String bootId) {
    public ConnectionInfo(String owner, String clientId, int restartCounter) {
      this(owner, clientId, restartCounter, null);
    }
  }

  public static final class BusyException extends IllegalStateException {
    BusyException() {
      super("one operation per client; operation not queued");
    }
  }

  public static final class ClosedException extends IOException {
    ClosedException() {
      super("client closed");
    }
  }

  public static final class BrokerException extends IOException {
    private final Wire.Error error;

    BrokerException(Wire.Error error) {
      super(error.getCode() + ": " + error.getDetail());
      this.error = error;
    }

    public Wire.Error error() {
      return error;
    }
  }

  public static final class TransportException extends IOException {
    private final boolean outcomeUnknown;

    TransportException(Throwable cause, boolean outcomeUnknown) {
      super("mynofy transport failed", cause);
      this.outcomeUnknown = outcomeUnknown;
    }

    public boolean outcomeUnknown() {
      return outcomeUnknown;
    }
  }

  private final Config config;
  private final Transport transport;
  private final AtomicBoolean busy = new AtomicBoolean(), closed = new AtomicBoolean();
  private ConnectionInfo info;
  private long sequence;

  private Client(Config config) throws IOException, GeneralSecurityException {
    this.config = config;
    transport = new Transport(config);
  }

  public static Client connect(Config config) throws IOException, GeneralSecurityException {
    Client c = new Client(config);
    long deadline = System.nanoTime() + config.timeout().toNanos();
    try {
      c.transport.connect(remaining(deadline));
      var challenge =
          c.roundtrip(
                  "auth_hello",
                  Wire.AuthHello.newBuilder().setTokenId(config.key().tokenId()),
                  "auth_challenge",
                  false,
                  remaining(deadline))
              .getAuthChallenge();
      if (!challenge.getServiceId().equals(config.owner()))
        throw new IOException("owner challenge mismatch");
      byte[] nonce = Key.decode(challenge.getNonce(), 32);
      var welcome =
          c.roundtrip(
                  "auth_proof",
                  Wire.AuthProof.newBuilder()
                      .setAuthId(challenge.getRequestId())
                      .setSignature(
                          Key.b64(
                              config
                                  .key()
                                  .sign(
                                      Key.authBytes(
                                          nonce, config.key().publicKey(), config.owner())))),
                  "auth_welcome",
                  false,
                  remaining(deadline))
              .getAuthWelcome();
      if (!welcome.getServiceId().equals(config.owner())
          || !welcome.getClientId().equals(config.clientId())
          || !welcome.hasRestartCounter()) throw new IOException("owner/client welcome mismatch");
      c.info =
          new ConnectionInfo(config.owner(), welcome.getClientId(), welcome.getRestartCounter(),
              validatedBootId(welcome.hasBootId() ? welcome.getBootId() : null));
      return c;
    } catch (InterruptedException e) {
      c.close();
      Thread.currentThread().interrupt();
      throw new TransportException(e, false);
    } catch (IOException | GeneralSecurityException | RuntimeException e) {
      c.close();
      throw e;
    }
  }

  public ConnectionInfo info() {
    return info;
  }

  @Override
  public void close() {
    if (closed.compareAndSet(false, true)) transport.close();
  }

  private void acquire() {
    if (!busy.compareAndSet(false, true)) throw new BusyException();
  }

  private Wire.Frame exchange(String kind, Message.Builder body, String expected, boolean mutation)
      throws IOException {
    acquire();
    try {
      return roundtrip(kind, body, expected, mutation, config.timeout());
    } finally {
      busy.set(false);
    }
  }

  private Wire.Frame roundtrip(
      String kind, Message.Builder body, String expected, boolean mutation, Duration timeout)
      throws IOException {
    if (closed.get() || transport.isClosed()) {
      close();
      throw new ClosedException();
    }
    if (Thread.currentThread().isInterrupted())
      throw new TransportException(new InterruptedException("interrupted before send"), false);
    if (sequence == Long.MAX_VALUE) throw new IOException("request ID space exhausted");
    String id = "java-" + (++sequence);
    body.setField(body.getDescriptorForType().findFieldByName("id"), id);
    var envelope = Wire.Frame.newBuilder().setVersion(1);
    envelope.setField(Wire.Frame.getDescriptor().findFieldByName(kind), body.build());
    byte[] request = envelope.build().toByteArray();
    try {
      var response = Wire.Frame.parseFrom(transport.exchange(request, timeout));
      var field = response.getOneofFieldDescriptor(Wire.Frame.getDescriptor().getOneofs().get(0));
      if (response.getVersion() != 1
          || field == null
          || !field.getName().equals(expected) && !field.getName().equals("error"))
        throw new IOException("invalid response envelope");
      Message reply = (Message) response.getField(field);
      var requestId = reply.getDescriptorForType().findFieldByName("request_id");
      boolean earlyRateLimit =
          response.hasError()
              && response.getError().getCode().equals("rate_limited")
              && !response.getError().hasRequestId()
              && response.getError().hasRetryAfterMs();
      if (!earlyRateLimit
          && (requestId == null
              || !reply.hasField(requestId)
              || !id.equals(reply.getField(requestId))))
        throw new IOException("response request ID mismatch");
      if (response.hasError()) throw new BrokerException(response.getError());
      if (response.hasMessages()) {
        for (var item : response.getMessages().getItemsList()) {
          if (item.getMessage().getProducerId().isEmpty())
            throw new IOException("missing producer identity");
        }
      }
      if (response.hasMessageStatus()
          && response.getMessageStatus().getMessage().getProducerId().isEmpty())
        throw new IOException("missing producer identity");
      if (kind.equals("publish")
          && (!response.getResult().hasMessageId() || !response.getResult().hasExpires()))
        throw new IOException("incomplete publish result");
      if (kind.equals("get_message")
          && !response
              .getMessageStatus()
              .getMessage()
              .getId()
              .equals(body.getField(body.getDescriptorForType().findFieldByName("message_id"))))
        throw new IOException("response message ID mismatch");
      if (body instanceof Wire.Poll.Builder poll) {
        var page = response.getMessages();
        if (!cursor(page.getNextCursor()) || page.getItemsCount() > poll.getLimit())
          throw new IOException("invalid poll page");
        for (var item : response.getMessages().getItemsList()) {
          if (!poll.getTopic().equals(item.getMessage().getTopic()))
            throw new IOException("poll response topic mismatch");
        }
      } else if (body instanceof Wire.ListMessages.Builder list && list.hasTopic()) {
        for (var item : response.getMessagesPage().getItemsList()) {
          if (!list.getTopic().equals(item.getTopic()))
            throw new IOException("producer page topic mismatch");
        }
      } else if (body instanceof Wire.GetPermissions.Builder permissions
          && !permissions.getTopic().equals(response.getPermissions().getTopic())) {
        throw new IOException("permissions response topic mismatch");
      }
      return response;
    } catch (BrokerException e) {
      throw e;
    } catch (InterruptedException e) {
      close();
      Thread.currentThread().interrupt();
      throw new TransportException(e, mutation);
    } catch (IOException | RuntimeException e) {
      close();
      throw new TransportException(e, mutation);
    }
  }

  public static Wire.Payload bytes(byte[] data, String contentType) {
    var builder = Wire.Payload.newBuilder().setData(ByteString.copyFrom(data));
    if (contentType != null) builder.setContentType(contentType);
    var payload = builder.build();
    validate(payload);
    return payload;
  }

  public static Wire.Payload event(Wire.Event event) {
    var payload = Wire.Payload.newBuilder().setEvent(event).build();
    validate(payload);
    return payload;
  }

  public Wire.Result publish(String topic, Wire.Payload payload, Instant expires)
      throws IOException {
    name(topic, "topic");
    validate(payload);
    var request = Wire.Publish.newBuilder().setTopic(topic).setPayload(payload);
    if (expires != null)
      request.setExpires(
          Wire.Expires.newBuilder().setTime(DateTimeFormatter.ISO_INSTANT.format(expires)));
    return exchange("publish", request, "result", true).getResult();
  }

  public Wire.Messages poll(String topic, String after, int limit) throws IOException {
    name(topic, "topic");
    limit(limit);
    if (after != null && !cursor(after)) throw new IllegalArgumentException("invalid poll cursor");
    var request = Wire.Poll.newBuilder().setTopic(topic).setLimit(limit);
    if (after != null) request.setAfter(after);
    return exchange("poll", request, "messages", false).getMessages();
  }

  /** Unsigned u64 IDs use Java long bit patterns, including negative values. */
  public Wire.MessagesPage listMessages(String topic, String cursor, int limit) throws IOException {
    limit(limit);
    var request = Wire.ListMessages.newBuilder().setLimit(limit);
    if (topic != null) {
      name(topic, "topic");
      request.setTopic(topic);
    }
    if (cursor != null) {
      text(cursor, 4096, "producer cursor");
      request.setCursor(cursor);
    }
    return exchange("list_messages", request, "messages_page", false).getMessagesPage();
  }

  public Wire.MessageStatus getMessage(String id) throws IOException {
    name(id, "message ID");
    return exchange(
            "get_message", Wire.GetMessage.newBuilder().setMessageId(id), "message_status", false)
        .getMessageStatus();
  }

  public Wire.Permissions getPermissions(String topic) throws IOException {
    name(topic, "topic");
    return exchange(
            "get_permissions",
            Wire.GetPermissions.newBuilder().setTopic(topic),
            "permissions",
            false)
        .getPermissions();
  }

  /** Definition is JSON bytes; use the application's JSON parser if needed. */
  public Wire.Catalog getCatalog(String name, int version) throws IOException {
    name(name, "catalog");
    if (version <= 0) throw new IllegalArgumentException("invalid catalog version");
    return exchange(
            "get_catalog",
            Wire.GetCatalog.newBuilder().setName(name).setCatalogVersion(version),
            "catalog",
            false)
        .getCatalog();
  }

  /** Save newKey first; success closes this session. Retain both keys if the outcome is unknown. */
  public Wire.RotationComplete rotate(Key newKey) throws IOException, GeneralSecurityException {
    if (newKey == null) throw new IllegalArgumentException("missing new key");
    acquire();
    long deadline = System.nanoTime() + config.timeout().toNanos();
    try {
      var offer =
          roundtrip(
                  "rotation_start",
                  Wire.RotationStart.newBuilder(),
                  "rotation_offer",
                  true,
                  remaining(deadline))
              .getRotationOffer();
      byte[] secret = new byte[32];
      new SecureRandom().nextBytes(secret);
      var ready =
          roundtrip(
                  "rotation_confirm",
                  Wire.RotationConfirm.newBuilder()
                      .setRotationId(offer.getRotationId())
                      .setSharedSecret(Key.b64(secret)),
                  "rotation_ready",
                  true,
                  remaining(deadline))
              .getRotationReady();
      byte[] aad =
          rotationAAD(
              info.owner(),
              info.clientId(),
              offer.getRotationId(),
              config.key().tokenId(),
              Instant.parse(ready.getNewValidUntil()).toEpochMilli());
      if (!ready.getRotationId().equals(offer.getRotationId())
          || !ready.getExpiresAt().equals(offer.getExpiresAt())
          || !ready.getNewValidUntil().equals(offer.getNewValidUntil())
          || !Arrays.equals(aad, Key.decode(ready.getAad(), -1)))
        throw new IOException("rotation binding mismatch; retain both keys");
      byte[] nonce = Key.decode(ready.getNonce(), 12),
          challenge = Key.decode(ready.getChallenge(), 32);
      var cipher = Cipher.getInstance("AES/GCM/NoPadding");
      cipher.init(
          Cipher.ENCRYPT_MODE, new SecretKeySpec(secret, "AES"), new GCMParameterSpec(128, nonce));
      cipher.updateAAD(aad);
      byte[] encrypted = cipher.doFinal(newKey.publicKey());
      byte[] proof = newKey.sign(rotationProof(aad, newKey.publicKey(), challenge));
      var complete =
          roundtrip(
                  "rotation_commit",
                  Wire.RotationCommit.newBuilder()
                      .setRotationId(offer.getRotationId())
                      .setEncryptedPublicKey(Key.b64(encrypted))
                      .setSignature(Key.b64(proof)),
                  "rotation_complete",
                  true,
                  remaining(deadline))
              .getRotationComplete();
      close();
      if (!complete.getRotationId().equals(offer.getRotationId())
          || !complete.getClientId().equals(info.clientId())
          || !complete.getTokenId().equals(newKey.tokenId())
          || !complete.getValidUntil().equals(ready.getNewValidUntil()))
        throw new TransportException(
            new IOException("rotation commit unconfirmed; retain both keys"), true);
      return complete;
    } catch (BrokerException e) {
      throw e;
    } catch (IOException | GeneralSecurityException | RuntimeException e) {
      close();
      throw e;
    } finally {
      busy.set(false);
    }
  }

  static byte[] rotationAAD(String owner, String client, String rotation, String old, long until) {
    return ("[\"mynofy-rotation-v1\","
            + quote(owner)
            + ","
            + quote(client)
            + ","
            + quote(rotation)
            + ","
            + quote(old)
            + ","
            + until
            + "]")
        .getBytes(StandardCharsets.UTF_8);
  }

  static byte[] rotationProof(byte[] aad, byte[] publicKey, byte[] challenge) {
    return Key.join(
        "mynofy-rotation-proof-v1\0".getBytes(StandardCharsets.UTF_8), aad, publicKey, challenge);
  }

  private static String quote(String value) {
    StringBuilder out = new StringBuilder("\"");
    for (int i = 0; i < value.length(); i++) {
      char c = value.charAt(i);
      switch (c) {
        case '"' -> out.append("\\\"");
        case '\\' -> out.append("\\\\");
        case '\b' -> out.append("\\b");
        case '\f' -> out.append("\\f");
        case '\n' -> out.append("\\n");
        case '\r' -> out.append("\\r");
        case '\t' -> out.append("\\t");
        default -> {
          if (c < 32) out.append(String.format("\\u%04x", (int) c));
          else out.append(c);
        }
      }
    }
    return out.append('"').toString();
  }

  private static Duration remaining(long deadline) throws java.net.SocketTimeoutException {
    long value = deadline - System.nanoTime();
    if (value <= 0) throw new java.net.SocketTimeoutException("mynofy operation timed out");
    return Duration.ofNanos(value);
  }

  private static boolean cursor(String value) {
    return value.matches("[0-9A-Za-z]{8}");
  }

  private static void limit(int limit) {
    if (limit < 1 || limit > 100) throw new IllegalArgumentException("limit must be 1..100");
  }

  private static void name(String value, String what) {
    text(value, 256, what);
    if (value.isEmpty()) throw new IllegalArgumentException("empty " + what);
  }

  private static void text(String value, int max, String what) {
    if (value == null || value.getBytes(StandardCharsets.UTF_8).length > max)
      throw new IllegalArgumentException("invalid " + what);
    for (int i = 0; i < value.length(); i++) {
      char c = value.charAt(i);
      if (Character.isHighSurrogate(c)) {
        if (++i >= value.length() || !Character.isLowSurrogate(value.charAt(i)))
          throw new IllegalArgumentException("invalid Unicode " + what);
      } else if (Character.isLowSurrogate(c))
        throw new IllegalArgumentException("invalid Unicode " + what);
    }
  }

  static void validate(Wire.Payload payload) {
    if (payload == null || !payload.isInitialized())
      throw new IllegalArgumentException("missing payload");
    if (payload.hasContentType()) {
      name(payload.getContentType(), "content type");
      text(payload.getContentType(), 128, "content type");
    }
    switch (payload.getBodyCase()) {
      case DATA -> {
        if (payload.getData().size() > 4096)
          throw new IllegalArgumentException("payload exceeds 4096 bytes");
      }
      case EVENT -> {
        if (payload.hasContentType())
          throw new IllegalArgumentException("event payloads must not have a content type");
        var event = payload.getEvent();
        if (event.getCatalog() != 1
            || event.getCatalogVersion() != 1
            || !CatalogIds.category(event.getCategory())
            || !CatalogIds.action(event.getAction())
            || !CatalogIds.status(event.getStatus())
            || event.getParametersCount() > 3)
          throw new IllegalArgumentException("invalid catalog event");
        for (var p : event.getParametersList()) {
          if (p.getValueCase() == Wire.Parameter.ValueCase.VALUE_NOT_SET
              || p.hasF32() && !Float.isFinite(p.getF32())
              || p.hasF64() && !Double.isFinite(p.getF64()))
            throw new IllegalArgumentException("invalid event parameter");
        }
      }
      default -> throw new IllegalArgumentException("missing payload body");
    }
  }
}
