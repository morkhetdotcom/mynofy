package dev.mynofy;

import java.io.*;
import java.net.*;
import java.nio.ByteBuffer;
import java.nio.file.*;
import java.security.*;
import java.security.cert.CertificateFactory;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.*;
import java.util.concurrent.atomic.*;
import javax.net.SocketFactory;
import javax.net.ssl.*;
import org.java_websocket.WebSocketImpl;
import org.java_websocket.client.WebSocketClient;
import org.java_websocket.drafts.Draft_6455;
import org.java_websocket.enums.Opcode;
import org.java_websocket.exceptions.InvalidDataException;
import org.java_websocket.exceptions.LimitExceededException;
import org.java_websocket.framing.Framedata;
import org.java_websocket.handshake.Handshakedata;
import org.java_websocket.handshake.ServerHandshake;
import org.java_websocket.exceptions.InvalidHandshakeException;

final class Transport implements AutoCloseable {
  static final int MAX_FRAME = 4 * 1024 * 1024;
  // Draft_6455 processes each decoded frame before retaining it in the message.
  // Keep the limits here until the maintained dependency enforces both itself.
  static final class BoundedDraft extends Draft_6455 {
    static final int MAX_FRAGMENTS = 1024;
    private int fragments;
    private int payloadBytes;

    BoundedDraft() {
      super(List.of(), MAX_FRAME);
    }

    @Override
    public Draft_6455 copyInstance() {
      return new BoundedDraft();
    }

    @Override
    public void reset() {
      super.reset();
      fragments = 0;
      payloadBytes = 0;
    }

    static final int MAX_UPGRADE_HEADERS = 16 * 1024;

    @Override
    public Handshakedata translateHandshake(ByteBuffer bytes) throws InvalidHandshakeException {
      int start = bytes.position();
      int budget = Math.min(bytes.remaining(), MAX_UPGRADE_HEADERS);
      for (int i = 3; i < budget; i++) {
        int end = start + i;
        if (bytes.get(end - 3) == '\r' && bytes.get(end - 2) == '\n'
            && bytes.get(end - 1) == '\r' && bytes.get(end) == '\n') {
          return super.translateHandshake(bytes);
        }
      }
      if (budget == MAX_UPGRADE_HEADERS) {
        throw new InvalidHandshakeException("WebSocket upgrade headers exceed 16 KiB");
      }
      return super.translateHandshake(bytes);
    }

    void account(Opcode opcode, int bytes, boolean fin) throws LimitExceededException {
      if (opcode != Opcode.BINARY && opcode != Opcode.TEXT && opcode != Opcode.CONTINUOUS) return;
      if (bytes > MAX_FRAME - payloadBytes || fragments == MAX_FRAGMENTS) {
        throw new LimitExceededException("WebSocket message exceeds receive limits");
      }
      payloadBytes = fin ? 0 : payloadBytes + bytes;
      fragments = fin ? 0 : fragments + 1;
    }

    @Override
    public void processFrame(WebSocketImpl connection, Framedata frame) throws InvalidDataException {
      account(frame.getOpcode(), frame.getPayloadData().remaining(), frame.isFin());
      super.processFrame(connection, frame);
    }
  }
  private final AtomicBoolean closed = new AtomicBoolean();
  private final AtomicReference<Socket> network = new AtomicReference<>();
  private final AtomicReference<CompletableFuture<byte[]>> pending = new AtomicReference<>();
  private final CompletableFuture<Void> opened = new CompletableFuture<>();
  private final WebSocketClient ws;

  Transport(Client.Config cfg) throws IOException, GeneralSecurityException {
    URI uri = cfg.socket() != null ? URI.create("ws://localhost/") : cfg.url();
    SSLContext tls = cfg.socket() == null ? tlsContext(cfg.caFile()) : null;
    ws =
        new WebSocketClient(uri, new BoundedDraft()) {
          @Override
          public void onOpen(ServerHandshake handshake) {
            opened.complete(null);
          }

          @Override
          public void onMessage(String text) {
            fail(new IOException("expected Protobuf binary response"));
          }

          @Override
          public void onMessage(ByteBuffer bytes) {
            var target = pending.getAndSet(null);
            if (target == null) {
              fail(new IOException("unexpected response"));
              return;
            }
            if (bytes.remaining() > MAX_FRAME) {
              var error = new IOException("oversized response");
              target.completeExceptionally(error);
              fail(error);
              return;
            }
            byte[] copy = new byte[bytes.remaining()];
            bytes.get(copy);
            target.complete(copy);
          }

          @Override
          public void onClose(int code, String reason, boolean remote) {
            fail(new EOFException("WebSocket closed"));
          }

          @Override
          public void onError(Exception error) {
            fail(error);
          }

          @Override
          protected void onSetSSLParameters(SSLParameters params) {
            super.onSetSSLParameters(params);
            params.setProtocols(new String[] {"TLSv1.3"});
          }
        };
    ws.setDaemon(true);
    ws.setConnectionLostTimeout(0); // No unsolicited heartbeat/polling thread.
    ws.setSocketFactory(
        new SocketFactory() {
          @Override
          public Socket createSocket() throws IOException {
            Socket socket =
                cfg.socket() == null ? new Socket(Proxy.NO_PROXY) : new UnixSocket(cfg.socket());
            install(socket);
            socket.connect(
                new InetSocketAddress(
                    uri.getHost(), uri.getPort() < 0 ? (tls == null ? 80 : 443) : uri.getPort()),
                millis(cfg.timeout()));
            if (tls != null) {
              SSLSocket secure =
                  (SSLSocket)
                      tls.getSocketFactory()
                          .createSocket(
                              socket, uri.getHost(), uri.getPort() < 0 ? 443 : uri.getPort(), true);
              install(secure);
              SSLParameters params = secure.getSSLParameters();
              params.setProtocols(new String[] {"TLSv1.3"});
              params.setEndpointIdentificationAlgorithm("HTTPS");
              secure.setSSLParameters(params);
              socket = secure;
            }
            return socket;
          }

          @Override
          public Socket createSocket(String h, int p) throws IOException {
            throw new IOException("unsupported socket factory entry");
          }

          @Override
          public Socket createSocket(String h, int p, InetAddress a, int l) throws IOException {
            throw new IOException("unsupported socket factory entry");
          }

          @Override
          public Socket createSocket(InetAddress h, int p) throws IOException {
            throw new IOException("unsupported socket factory entry");
          }

          @Override
          public Socket createSocket(InetAddress h, int p, InetAddress a, int l)
              throws IOException {
            throw new IOException("unsupported socket factory entry");
          }
        });
  }

  private void install(Socket socket) throws IOException {
    network.set(socket);
    if (closed.get()) {
      socket.close();
      throw new IOException("transport closed");
    }
  }

  boolean isClosed() {
    return closed.get();
  }

  void connect(Duration timeout) throws IOException, InterruptedException {
    ws.connect();
    try {
      await(opened, timeout);
    } catch (IOException | InterruptedException e) {
      close();
      throw e;
    }
  }

  byte[] exchange(byte[] frame, Duration timeout) throws IOException, InterruptedException {
    if (closed.get()) throw new IOException("transport closed");
    var result = new CompletableFuture<byte[]>();
    if (!pending.compareAndSet(null, result)) throw new IllegalStateException("transport busy");
    try {
      if (closed.get()) throw new IOException("transport closed");
      ws.send(frame);
      return await(result, timeout);
    } catch (IOException | InterruptedException | RuntimeException e) {
      close();
      throw e;
    } finally {
      pending.compareAndSet(result, null);
    }
  }

  private static <T> T await(CompletableFuture<T> future, Duration timeout)
      throws IOException, InterruptedException {
    try {
      return future.get(timeout.toNanos(), TimeUnit.NANOSECONDS);
    } catch (TimeoutException e) {
      throw new java.net.SocketTimeoutException("mynofy operation timed out");
    } catch (ExecutionException e) {
      throw new IOException("WebSocket exchange failed", e.getCause());
    }
  }

  private void fail(Exception error) {
    opened.completeExceptionally(error);
    var active = pending.getAndSet(null);
    if (active != null) active.completeExceptionally(error);
    close();
  }

  @Override
  public void close() {
    if (!closed.compareAndSet(false, true)) return;
    // Close the socket BEFORE stopping the writer: its shutdown path may drain queued frames.
    Socket socket = network.get();
    if (socket != null)
      try {
        socket.close();
      } catch (IOException ignored) {
      }
    opened.completeExceptionally(new EOFException("transport closed"));
    var active = pending.getAndSet(null);
    if (active != null) active.completeExceptionally(new EOFException("transport closed"));
    ws.closeConnection(1000, "closed");
  }

  private static int millis(Duration timeout) {
    return (int) Math.min(Integer.MAX_VALUE, Math.max(1, timeout.toMillis()));
  }

  private static SSLContext tlsContext(Path caFile) throws IOException, GeneralSecurityException {
    TrustManager[] managers = null;
    if (caFile != null) {
      var store = KeyStore.getInstance(KeyStore.getDefaultType());
      store.load(null, null);
      try (InputStream input = Files.newInputStream(caFile)) {
        var certs = CertificateFactory.getInstance("X.509").generateCertificates(input);
        if (certs.isEmpty()) throw new IOException("empty CA file");
        int i = 0;
        for (var cert : certs) store.setCertificateEntry("ca-" + i++, cert);
      }
      var factory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
      factory.init(store);
      managers = factory.getTrustManagers();
    }
    var context = SSLContext.getInstance("TLSv1.3");
    context.init(null, managers, null);
    return context;
  }
}
