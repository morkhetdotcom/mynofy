package dev.mynofy;

import com.google.gson.*;
import com.sun.security.auth.module.UnixSystem;
import java.io.*;
import java.net.*;
import java.nio.file.*;
import java.nio.file.attribute.PosixFilePermissions;
import java.time.Duration;
import java.util.*;
import java.util.concurrent.TimeUnit;

final class Fixture implements AutoCloseable {
  static final String TOPIC = "local/events";
  final Path dir, socket, binary;
  final Key key, other;
  final Client.Config config;
  Process process;

  Fixture(String mode) throws Exception {
    if (new UnixSystem().getUid() == 0)
      throw new IllegalStateException("run integration tests under a normal UID");
    binary =
        Path.of(
            System.getenv()
                .getOrDefault(
                    "MYNOFY_BIN", ConformanceTest.ROOT.resolve("target/debug/mynofy").toString()));
    dir =
        Files.createTempDirectory(
            "mn-java-",
            PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")));
    socket = dir.resolve("m.sock");
    try {
      run(List.of(binary.toString(), "keygen", "--out", dir.resolve("rust.pk8").toString()), null);
      key = Key.parse(Files.readAllBytes(dir.resolve("rust.pk8")));
      other = Key.generate();
      var cfg =
          JsonParser.parseString(
                  Files.readString(ConformanceTest.ROOT.resolve("config/scenarios/unix.json")))
              .getAsJsonObject();
      var service = new JsonObject();
      service.addProperty("id", "local");
      Client.Config chosen;
      if (mode.equals("wss")) {
        run(
            List.of(
                "openssl",
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=DNS:localhost",
                "-keyout",
                dir.resolve("s.key").toString(),
                "-out",
                dir.resolve("ca.pem").toString()),
            null);
        int port;
        try (var probe = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))) {
          port = probe.getLocalPort();
        }
        service.addProperty("listen", "127.0.0.1:" + port);
        var tls = new JsonObject();
        tls.addProperty("certificate_file", dir.resolve("ca.pem").toString());
        tls.addProperty("private_key_file", dir.resolve("s.key").toString());
        service.add("tls", tls);
        chosen =
            Client.Config.wss(
                URI.create("wss://localhost:" + port), "local", key, dir.resolve("ca.pem"));
      } else {
        service.addProperty("unix_socket", socket.toString());
        chosen = Client.Config.unix(socket, "local", key);
      }
      config =
          new Client.Config(
              chosen.url(),
              chosen.socket(),
              chosen.owner(),
              chosen.key(),
              chosen.caFile(),
              chosen.clientId(),
              Duration.ofSeconds(3));
      cfg.add("service", service);
      var clients = new JsonArray();
      for (Key k : List.of(key, other)) {
        var client = new JsonObject();
        client.addProperty("client_id", k.clientId());
        var registration = new JsonObject();
        registration.addProperty("algorithm", "ed25519");
        registration.addProperty("public_key", Key.b64(k.publicKey()));
        registration.addProperty("valid_until", "2035-01-01T00:00:00Z");
        var keys = new JsonArray();
        keys.add(registration);
        client.add("keys", keys);
        clients.add(client);
      }
      cfg.add("clients", clients);
      var acl = new JsonObject();
      acl.addProperty("client_id", key.clientId());
      var rights = new JsonArray();
      for (String right : List.of("publish", "subscribe", "inspect_own")) rights.add(right);
      acl.add("allow", rights);
      var entries = new JsonArray();
      entries.add(acl);
      cfg.getAsJsonArray("topics").get(0).getAsJsonObject().add("acl", entries);
      cfg.getAsJsonObject("limits").addProperty("client_burst", 1000);
      cfg.getAsJsonObject("limits").addProperty("client_requests_per_second", 1000);
      Path file = dir.resolve("config.json");
      var security = new com.google.gson.JsonObject();
      security.addProperty("directory", "security");
      cfg.add("security_storage", security);
      Files.writeString(file, cfg.toString());
      var init = new ProcessBuilder(binary.toString(), "security-init", "--config", file.toString())
          .inheritIO().start();
      if (init.waitFor() != 0) throw new IOException("Security store initialization failed");
      process =
          new ProcessBuilder(binary.toString(), "serve", "--config", file.toString())
              .redirectOutput(ProcessBuilder.Redirect.DISCARD)
              .redirectError(dir.resolve("stderr").toFile())
              .start();
      long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
      while (System.nanoTime() < deadline) {
        if (!process.isAlive())
          throw new IOException("service stopped: " + Files.readString(dir.resolve("stderr")));
        if (mode.equals("unix") && Files.exists(socket)) return;
        if (mode.equals("wss"))
          try (var probe = new Socket()) {
            probe.connect(new InetSocketAddress("127.0.0.1", chosen.url().getPort()), 50);
            return;
          } catch (IOException ignored) {
          }
        Thread.sleep(10);
      }
      throw new IOException("service startup timeout");
    } catch (Exception e) {
      close();
      throw e;
    }
  }

  static String run(List<String> command, String stdin) throws Exception {
    Process p = new ProcessBuilder(command).redirectErrorStream(true).start();
    try {
      if (stdin != null)
        p.getOutputStream().write(stdin.getBytes(java.nio.charset.StandardCharsets.UTF_8));
      p.getOutputStream().close();
      // Drain concurrently so full pipes cannot block process termination.
      var bytes = new java.io.ByteArrayOutputStream();
      Thread reader =
          new Thread(
              () -> {
                try {
                  p.getInputStream().transferTo(bytes);
                } catch (IOException ignored) {
                }
              });
      reader.setDaemon(true);
      reader.start();
      if (!p.waitFor(10, TimeUnit.SECONDS)) throw new IOException("subprocess timeout");
      reader.join(1000);
      String output = bytes.toString(java.nio.charset.StandardCharsets.UTF_8);
      if (p.exitValue() != 0) throw new IOException("subprocess failed: " + output);
      return output;
    } finally {
      if (p.isAlive()) p.destroyForcibly();
    }
  }

  @Override
  public void close() throws Exception {
    if (process != null) {
      process.destroy();
      if (!process.waitFor(3, TimeUnit.SECONDS)) {
        process.destroyForcibly();
        process.waitFor(3, TimeUnit.SECONDS);
      }
    }
    if (Files.exists(dir))
      try (var files = Files.walk(dir)) {
        for (Path p : files.sorted(Comparator.reverseOrder()).toList()) Files.deleteIfExists(p);
      }
  }
}
