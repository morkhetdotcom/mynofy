package dev.mynofy;

import static org.junit.jupiter.api.Assertions.*;

import dev.mynofy.protocol.Wire;
import java.io.*;
import java.net.*;
import java.nio.ByteBuffer;
import java.nio.file.*;
import java.security.*;
import java.security.cert.CertificateFactory;
import java.security.spec.PKCS8EncodedKeySpec;
import java.time.Duration;
import java.util.Base64;
import java.util.List;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import javax.net.ssl.*;
import org.java_websocket.WebSocket;
import org.java_websocket.WebSocketAdapter;
import org.java_websocket.WebSocketImpl;
import org.java_websocket.drafts.Draft_6455;
import org.java_websocket.handshake.Handshakedata;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

@Timeout(30)
class FailureTest {
  static final class Peer extends WebSocketAdapter implements AutoCloseable {
    final SSLServerSocket server;
    final Thread worker;
    volatile SSLSocket socket;
    final AtomicReference<Throwable> unexpected = new AtomicReference<>();
    final CompletableFuture<Wire.Frame> received = new CompletableFuture<>();
    final AtomicInteger publishes = new AtomicInteger();
    final AtomicInteger commands = new AtomicInteger();
    final Fixture fixture;
    final String mode;

    Peer(Fixture fixture, String mode) throws Exception {
      this.fixture = fixture;
      this.mode = mode;
      var store = KeyStore.getInstance(KeyStore.getDefaultType());
      store.load(null, null);
      java.security.cert.Certificate cert;
      try (var input = Files.newInputStream(fixture.dir.resolve("ca.pem"))) {
        cert = CertificateFactory.getInstance("X.509").generateCertificate(input);
      }
      String pem =
          Files.readString(fixture.dir.resolve("s.key"))
              .replace("-----BEGIN PRIVATE KEY-----", "")
              .replace("-----END PRIVATE KEY-----", "")
              .replaceAll("\\s", "");
      var key =
          KeyFactory.getInstance("EC")
              .generatePrivate(new PKCS8EncodedKeySpec(Base64.getDecoder().decode(pem)));
      store.setKeyEntry("server", key, new char[0], new java.security.cert.Certificate[] {cert});
      var km = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
      km.init(store, new char[0]);
      var ssl = SSLContext.getInstance("TLSv1.3");
      ssl.init(km.getKeyManagers(), null, null);
      // Blocking JSSE sockets avoid the library's server-only TLS-engine path.
      // Framing and HTTP upgrade still use its unmodified WebSocket codec.
      server =
          (SSLServerSocket)
              ssl.getServerSocketFactory()
                  .createServerSocket(0, 1, InetAddress.getByName("127.0.0.1"));
      server.setEnabledProtocols(new String[] {"TLSv1.3"});
      worker =
          new Thread(
              () -> {
                try {
                  socket = (SSLSocket) server.accept();
                  socket.setSoTimeout(5000);
                  socket.startHandshake();
                  var engine =
                      new WebSocketImpl(
                          this, List.of(new Draft_6455(List.of(), Transport.MAX_FRAME)));
                  byte[] bytes = new byte[8192];
                  int count;
                  while ((count = socket.getInputStream().read(bytes)) >= 0)
                    engine.decode(ByteBuffer.wrap(bytes, 0, count));
                } catch (IOException expectedDisconnect) {
                } catch (Throwable error) {
                  unexpected.set(error);
                } finally {
                  closeSocket();
                }
              },
              "mynofy-fault-peer");
      worker.setDaemon(true);
      worker.start();
    }

    Client.Config config(Duration timeout) {
      return new Client.Config(
          URI.create("wss://localhost:" + server.getLocalPort()),
          null,
          "local",
          fixture.key,
          fixture.config.caFile(),
          null,
          timeout);
    }

    private void closeSocket() {
      if (socket != null)
        try {
          socket.close();
        } catch (IOException ignored) {
        }
    }

    @Override
    public void onWebsocketOpen(WebSocket conn, Handshakedata handshake) {}

    @Override
    public void onWebsocketClose(WebSocket conn, int code, String reason, boolean remote) {
      closeSocket();
    }

    @Override
    public void onWebsocketClosing(WebSocket conn, int code, String reason, boolean remote) {}

    @Override
    public void onWebsocketCloseInitiated(WebSocket conn, int code, String reason) {}

    @Override
    public void onWebsocketError(WebSocket conn, Exception error) {
      unexpected.compareAndSet(null, error);
      closeSocket();
    }

    @Override
    public InetSocketAddress getLocalSocketAddress(WebSocket conn) {
      return (InetSocketAddress) socket.getLocalSocketAddress();
    }

    @Override
    public InetSocketAddress getRemoteSocketAddress(WebSocket conn) {
      return (InetSocketAddress) socket.getRemoteSocketAddress();
    }

    @Override
    public void onWriteDemand(WebSocket conn) {
      try {
        ByteBuffer frame;
        while ((frame = ((WebSocketImpl) conn).outQueue.poll()) != null) {
          byte[] bytes = new byte[frame.remaining()];
          frame.get(bytes);
          socket.getOutputStream().write(bytes);
        }
      } catch (IOException expectedDisconnect) {
        closeSocket();
      }
    }

    @Override
    public void onWebsocketMessage(WebSocket conn, String message) {
      conn.closeConnection(1002, "binary required");
    }

    @Override
    public void onWebsocketMessage(WebSocket conn, ByteBuffer data) {
      try {
        byte[] bytes = new byte[data.remaining()];
        data.get(bytes);
        var frame = Wire.Frame.parseFrom(bytes);
        if (frame.hasAuthHello()) {
          if (mode.equals("auth-wait")) return;
          send(
              conn,
              Wire.Frame.newBuilder()
                  .setVersion(1)
                  .setAuthChallenge(
                      Wire.AuthChallenge.newBuilder()
                          .setRequestId(frame.getAuthHello().getId())
                          .setServiceId("local")
                          .setNonce(Key.b64(new byte[32])))
                  .build());
          return;
        }
        if (frame.hasAuthProof()) {
          var welcome =
              Wire.Frame.newBuilder()
                  .setVersion(1)
                  .setAuthWelcome(
                      Wire.AuthWelcome.newBuilder()
                          .setRequestId(frame.getAuthProof().getId())
                          .setServiceId("local")
                          .setClientId(fixture.key.clientId())
                          .setRestartCounter(1));
          if (mode.equals("welcome-owner")) welcome.getAuthWelcomeBuilder().setServiceId("wrong");
          if (mode.equals("welcome-client")) welcome.getAuthWelcomeBuilder().setClientId("wrong");
          if (mode.equals("welcome-counter")) welcome.getAuthWelcomeBuilder().clearRestartCounter();
          send(conn, welcome.build());
          return;
        }
        received.complete(frame);
        commands.incrementAndGet();
        if (mode.startsWith("semantic:")) {
          var response =
              Wire.Frame.parseFrom(Base64.getDecoder().decode(mode.substring(9))).toBuilder();
          if (frame.hasPublish())
            response.getResultBuilder().setRequestId(frame.getPublish().getId());
          else response.getMessageStatusBuilder().setRequestId(frame.getGetMessage().getId());
          send(conn, response.build());
          return;
        }
        if (frame.hasPublish()) publishes.incrementAndGet();
        if (mode.equals("rate-limit")) {
          send(
              conn,
              Wire.Frame.newBuilder()
                  .setVersion(1)
                  .setError(
                      Wire.Error.newBuilder()
                          .setCode("rate_limited")
                          .setDetail("wait")
                          .setRetryAfterMs(123))
                  .build());
          return;
        }
        if (mode.equals("wait")) return;
        if (mode.equals("lost")) {
          conn.closeConnection(1001, "lost result");
          return;
        }
        String id = frame.hasPublish() ? frame.getPublish().getId() : "unexpected";
        var result =
            Wire.Frame.newBuilder()
                .setVersion(1)
                .setResult(Wire.Result.newBuilder().setRequestId(id));
        switch (mode) {
          case "request-id" -> result.getResultBuilder().setRequestId("wrong");
          case "version" -> result.setVersion(2);
          case "required" -> result.clearVersion();
          case "text" -> {
            conn.send("{}");
            return;
          }
          case "malformed" -> {
            conn.send(new byte[] {(byte) 255});
            return;
          }
          case "oversized" -> {
            conn.send(new byte[Transport.MAX_FRAME + 1]);
            return;
          }
        }
        conn.send(result.buildPartial().toByteArray());
      } catch (Exception e) {
        unexpected.compareAndSet(null, e);
        conn.closeConnection(1011, "test peer failure");
      }
    }

    void send(WebSocket conn, Wire.Frame frame) {
      conn.send(frame.toByteArray());
    }

    @Override
    public void close() throws Exception {
      server.close();
      closeSocket();
      worker.join(2000);
      assertFalse(worker.isAlive(), "fault peer leaked");
      assertNull(unexpected.get(), "fault peer failed");
    }
  }

  @Test
  @Timeout(20)
  void sharedReplySemantics() throws Exception {
    var vectors =
        com.google.gson.JsonParser.parseString(
                Files.readString(ConformanceTest.ROOT.resolve("protocol/conformance/sdk-v1.json")))
            .getAsJsonObject();
    var cases = vectors.getAsJsonArray("reply_validation");
    assertFalse(cases.isEmpty());
    try (var fixture = new Fixture("wss")) {
      for (var element : cases) {
        var item = element.getAsJsonObject();
        boolean valid = item.get("valid").getAsBoolean();
        boolean mutation = item.get("mutation").getAsBoolean();
        try (var peer = new Peer(fixture, "semantic:" + item.get("response_base64").getAsString());
            var client = Client.connect(peer.config(Duration.ofSeconds(2)))) {
          org.junit.jupiter.api.function.Executable operation =
              mutation
                  ? () -> client.publish(Fixture.TOPIC, Client.bytes(new byte[] {1}, null), null)
                  : () -> client.getMessage("00000001");
          if (valid) {
            assertDoesNotThrow(operation);
            assertDoesNotThrow(operation);
          } else {
            var error =
                assertThrows(
                    Client.TransportException.class, operation, item.get("name").getAsString());
            assertEquals(mutation, error.outcomeUnknown());
            assertThrows(Client.ClosedException.class, operation);
          }
          assertEquals(valid ? 2 : 1, peer.commands.get());
        }
      }
    }
  }

  @Test
  void uncorrelatedRateLimitPreservesConnection() throws Exception {
    try (var f = new Fixture("wss");
        var peer = new Peer(f, "rate-limit");
        var c = Client.connect(peer.config(Duration.ofSeconds(2)))) {
      for (int i = 0; i < 2; i++) {
        var error =
            assertThrows(
                Client.BrokerException.class,
                () -> c.publish(Fixture.TOPIC, Client.bytes(new byte[1], null), null));
        assertEquals("rate_limited", error.error().getCode());
        assertEquals(123L, error.error().getRetryAfterMs());
      }
      assertEquals(2, peer.publishes.get());
    }
  }

  @Test
  void connectionAuthenticationDeadline() throws Exception {
    try (var f = new Fixture("wss");
        var peer = new Peer(f, "auth-wait")) {
      long start = System.nanoTime();
      var error =
          assertThrows(
              Client.TransportException.class,
              () -> Client.connect(peer.config(Duration.ofMillis(300))));
      assertFalse(error.outcomeUnknown());
      assertInstanceOf(SocketTimeoutException.class, error.getCause());
      assertTrue(System.nanoTime() - start < TimeUnit.SECONDS.toNanos(2));
    }
  }

  @Test
  void lostPublishIsNeverRetried() throws Exception {
    try (var f = new Fixture("wss");
        var peer = new Peer(f, "lost");
        var c = Client.connect(peer.config(Duration.ofSeconds(2)))) {
      var error =
          assertThrows(
              Client.TransportException.class,
              () -> c.publish(Fixture.TOPIC, Client.bytes(new byte[1], null), null));
      assertTrue(error.outcomeUnknown());
      assertThrows(
          Client.ClosedException.class,
          () -> c.publish(Fixture.TOPIC, Client.bytes(new byte[1], null), null));
      assertEquals(1, peer.publishes.get());
    }
  }

  @Test
  void interruptTimeoutCloseAndBusy() throws Exception {
    try (var f = new Fixture("wss")) {
      for (String mode : List.of("interrupt", "timeout", "close")) {
        try (var peer = new Peer(f, "wait");
            var c = Client.connect(peer.config(Duration.ofSeconds(1)))) {
          var done = new CompletableFuture<Throwable>();
          Thread worker =
              new Thread(
                  () -> {
                    try {
                      c.publish(Fixture.TOPIC, Client.bytes(new byte[1], null), null);
                      done.complete(null);
                    } catch (Throwable e) {
                      done.complete(e);
                    }
                  });
          worker.start();
          peer.received.get(2, TimeUnit.SECONDS);
          assertThrows(Client.BusyException.class, () -> c.poll(Fixture.TOPIC, null, 20));
          if (mode.equals("interrupt")) worker.interrupt();
          if (mode.equals("close")) c.close();
          Throwable result = done.get(3, TimeUnit.SECONDS);
          worker.join(1000);
          assertInstanceOf(Client.TransportException.class, result);
          assertTrue(((Client.TransportException) result).outcomeUnknown());
          if (mode.equals("interrupt")) {
            assertInstanceOf(InterruptedException.class, result.getCause());
            assertTrue(worker.isInterrupted());
          }
          if (mode.equals("timeout"))
            assertInstanceOf(SocketTimeoutException.class, result.getCause());
          assertEquals(1, peer.publishes.get());
        }
      }
    }
  }

  @Test
  void invalidFramesDiscardTheConnection() throws Exception {
    try (var f = new Fixture("wss")) {
      for (String mode :
          List.of("request-id", "version", "required", "text", "malformed", "oversized")) {
        try (var peer = new Peer(f, mode);
            var c = Client.connect(peer.config(Duration.ofSeconds(2)))) {
          var error =
              assertThrows(
                  Client.TransportException.class,
                  () -> c.publish(Fixture.TOPIC, Client.bytes(new byte[1], null), null),
                  mode);
          assertTrue(error.outcomeUnknown(), mode);
          assertThrows(Client.ClosedException.class, () -> c.poll(Fixture.TOPIC, null, 20));
        }
      }
    }
  }

  @Test
  void invalidWelcomeIsRejected() throws Exception {
    try (var f = new Fixture("wss")) {
      for (String mode : List.of("welcome-owner", "welcome-client", "welcome-counter")) {
        try (var peer = new Peer(f, mode)) {
          assertThrows(
              IOException.class,
              () -> {
                try (var c = Client.connect(peer.config(Duration.ofSeconds(2)))) {
                  fail("invalid welcome accepted");
                }
              });
        }
      }
    }
  }
}
