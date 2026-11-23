package dev.mynofy;

import com.sun.security.auth.module.UnixSystem;
import java.io.*;
import java.net.*;
import java.nio.ByteBuffer;
import java.nio.channels.SocketChannel;
import java.nio.file.*;
import java.nio.file.attribute.*;
import java.util.Set;
import jdk.net.ExtendedSocketOptions;

/** Socket facade only; WebSocket framing stays in the upstream library. */
final class UnixSocket extends Socket {
  private final SocketChannel channel = SocketChannel.open(StandardProtocolFamily.UNIX);
  private final Path path;

  UnixSocket(Path configured) throws IOException {
    try {
      Path parent = configured.toAbsolutePath().getParent().toRealPath();
      Set<PosixFilePermission> permissions = Files.getPosixFilePermissions(parent);
      if (!Files.isDirectory(parent)
          || permissions.contains(PosixFilePermission.GROUP_WRITE)
          || permissions.contains(PosixFilePermission.OTHERS_WRITE))
        throw new IOException("unsafe Unix socket parent");
      path = parent.resolve(configured.getFileName());
      var attrs = Files.readAttributes(path, BasicFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
      if (!attrs.isOther()
          || Files.isSymbolicLink(path)
          || !Files.getPosixFilePermissions(path)
              .equals(PosixFilePermissions.fromString("rw-------")))
        throw new IOException("expected Unix socket with mode 0600");
    } catch (Throwable e) {
      channel.close();
      throw e;
    }
  }

  @Override
  public void connect(SocketAddress ignored, int timeout) throws IOException {
    channel.connect(UnixDomainSocketAddress.of(path));
    try {
      var peer = channel.getOption(ExtendedSocketOptions.SO_PEERCRED);
      String uid = Long.toString(new UnixSystem().getUid());
      UserPrincipal current =
          path.getFileSystem().getUserPrincipalLookupService().lookupPrincipalByName(uid);
      if (!peer.user().equals(current)) throw new IOException("Unix peer UID mismatch");
    } catch (IOException | RuntimeException e) {
      close();
      throw e;
    }
  }

  @Override
  public boolean isConnected() {
    return channel.isConnected();
  }

  @Override
  public boolean isClosed() {
    return !channel.isOpen();
  }

  @Override
  public void close() throws IOException {
    channel.close();
  }

  // TCP options do not apply to AF_UNIX.
  @Override
  public void setTcpNoDelay(boolean value) {}

  @Override
  public void setReuseAddress(boolean value) {}

  @Override
  public SocketAddress getLocalSocketAddress() {
    return new InetSocketAddress("127.0.0.1", 0);
  }

  @Override
  public SocketAddress getRemoteSocketAddress() {
    return new InetSocketAddress("127.0.0.1", 0);
  }

  @Override
  public InputStream getInputStream() {
    return new InputStream() {
      @Override
      public int read() throws IOException {
        byte[] b = new byte[1];
        return read(b, 0, 1) < 0 ? -1 : b[0] & 255;
      }

      @Override
      public int read(byte[] b, int offset, int length) throws IOException {
        return channel.read(ByteBuffer.wrap(b, offset, length));
      }
    };
  }

  @Override
  public OutputStream getOutputStream() {
    // Channels.new{Input,Output}Stream share a blocking lock: avoid that duplex deadlock.
    return new OutputStream() {
      @Override
      public void write(int b) throws IOException {
        write(new byte[] {(byte) b}, 0, 1);
      }

      @Override
      public void write(byte[] b, int offset, int length) throws IOException {
        ByteBuffer buffer = ByteBuffer.wrap(b, offset, length);
        while (buffer.hasRemaining()) channel.write(buffer);
      }
    };
  }
}
