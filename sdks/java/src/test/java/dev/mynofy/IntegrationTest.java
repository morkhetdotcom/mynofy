package dev.mynofy;

import static org.junit.jupiter.api.Assertions.*;

import dev.mynofy.protocol.Wire;
import java.io.IOException;
import java.net.URI;
import java.nio.file.*;
import java.nio.file.attribute.PosixFilePermissions;
import java.time.Instant;
import java.util.List;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

@Timeout(30)
class IntegrationTest {
  @Test
  void unixRoundtrip() throws Exception {
    roundtrip("unix");
  }

  @Test
  void wssRoundtrip() throws Exception {
    roundtrip("wss");
  }

  private void roundtrip(String mode) throws Exception {
    try (var f = new Fixture(mode);
        var c = Client.connect(f.config)) {
      assertEquals(f.key.clientId(), c.info().clientId());
      assertEquals("local", c.info().owner());
      assertTrue(
          c.getCatalog("mynofy", 1).getDefinition().toStringUtf8().contains("\"catalog_id\""));
      assertTrue(c.getPermissions(Fixture.TOPIC).getActionsList().contains("publish"));
      byte[] bytes = {0, (byte) 255, 1};
      var first = c.publish(Fixture.TOPIC, Client.bytes(bytes, "application/octet-stream"), null);
      var event =
          Wire.Event.newBuilder()
              .setCatalog(1)
              .setCatalogVersion(1)
              .setCategory(12)
              .setAction(42)
              .setStatus(2)
              .addParameters(Wire.Parameter.newBuilder().setU64(-1L))
              .addParameters(Wire.Parameter.newBuilder().setI64(Long.MIN_VALUE))
              .addParameters(Wire.Parameter.newBuilder().setBool(false))
              .build();
      var second = c.publish(Fixture.TOPIC, Client.event(event), Instant.now().plusSeconds(60));
      var page = c.poll(Fixture.TOPIC, null, 1);
      assertEquals(1, page.getItemsCount());
      assertEquals(c.info().clientId(), page.getItems(0).getMessage().getProducerId());
      assertTrue(page.getHasMore());
      assertArrayEquals(bytes, page.getItems(0).getMessage().getPayload().getData().toByteArray());
      var next = c.poll(Fixture.TOPIC, page.getNextCursor(), 20);
      assertEquals(1, next.getItemsCount());
      assertEquals(event, next.getItems(0).getMessage().getPayload().getEvent());
      assertEquals(first.getMessageId(), c.getMessage(first.getMessageId()).getMessage().getId());
      assertEquals(second.getMessageId(), c.getMessage(second.getMessageId()).getMessage().getId());
      var own = c.listMessages(Fixture.TOPIC, null, 1);
      assertEquals(1, own.getItemsCount());
      assertTrue(own.hasNextCursor());
      assertEquals(1, c.listMessages(Fixture.TOPIC, own.getNextCursor(), 1).getItemsCount());
      assertEquals(0, c.poll(Fixture.TOPIC, next.getNextCursor(), 20).getItemsCount());
      var deniedCfg =
          new Client.Config(
              f.config.url(), f.config.socket(), "local", f.other, f.config.caFile(), null, null);
      try (var denied = Client.connect(deniedCfg)) {
        var err =
            assertThrows(
                Client.BrokerException.class,
                () -> denied.publish(Fixture.TOPIC, Client.bytes(new byte[1], null), null));
        assertEquals("forbidden", err.error().getCode());
        assertNotNull(denied.getCatalog("mynofy", 1));
      }
      assertThrows(IllegalArgumentException.class, () -> c.publish(Fixture.TOPIC, null, null));
      assertThrows(IllegalArgumentException.class, () -> c.poll(Fixture.TOPIC, "bad", 20));
      assertNotNull(c.publish(Fixture.TOPIC, Client.bytes(new byte[4096], null), null));
    }
  }

  @Test
  void authenticationAndUnixPathValidation() throws Exception {
    try (var f = new Fixture("unix")) {
      for (String mode : List.of("owner", "identity", "key")) {
        var cfg =
            new Client.Config(
                null,
                f.socket,
                mode.equals("owner") ? "wrong" : "local",
                mode.equals("key") ? Key.generate() : f.key,
                null,
                mode.equals("identity") ? "client:wrong" : null,
                null);
        assertThrows(
            IOException.class,
            () -> {
              try (var c = Client.connect(cfg)) {
                fail("accepted wrong " + mode);
              }
            });
      }
      Files.setPosixFilePermissions(f.socket, PosixFilePermissions.fromString("rw-rw-rw-"));
      assertThrows(
          IOException.class,
          () -> {
            try (var c = Client.connect(f.config)) {
              fail("unsafe mode accepted");
            }
          });
      Files.setPosixFilePermissions(f.socket, PosixFilePermissions.fromString("rw-------"));
      Path link = f.dir.resolve("link.sock");
      Files.createSymbolicLink(link, f.socket);
      assertThrows(
          IOException.class,
          () -> {
            try (var c = Client.connect(Client.Config.unix(link, "local", f.key))) {
              fail("symlink accepted");
            }
          });
    }
  }

  @Test
  void tlsValidation() throws Exception {
    try (var f = new Fixture("wss")) {
      for (String mode : List.of("hostname", "ca")) {
        var cfg =
            new Client.Config(
                mode.equals("hostname")
                    ? URI.create(f.config.url().toString().replace("localhost", "127.0.0.1"))
                    : f.config.url(),
                null,
                "local",
                f.key,
                mode.equals("ca") ? null : f.config.caFile(),
                null,
                null);
        assertThrows(
            IOException.class,
            () -> {
              try (var c = Client.connect(cfg)) {
                fail("accepted wrong " + mode);
              }
            });
      }
      assertThrows(
          IllegalArgumentException.class,
          () -> Client.Config.wss(URI.create("ws://localhost:8080"), "local", f.key, null));
    }
  }

  @Test
  void rotationAndCrossLanguageKeys() throws Exception {
    try (var f = new Fixture("unix");
        var c = Client.connect(f.config)) {
      var sent =
          c.publish(
              Fixture.TOPIC,
              Client.bytes(
                  "before rotation".getBytes(java.nio.charset.StandardCharsets.UTF_8), null),
              null);
      Key fresh = Key.generate();
      Path path = f.dir.resolve("new.pk8");
      Files.write(path, fresh.privateBytes(), StandardOpenOption.CREATE_NEW);
      Files.setPosixFilePermissions(path, PosixFilePermissions.fromString("rw-------"));
      var complete = c.rotate(fresh);
      assertEquals(f.key.clientId(), complete.getClientId());
      assertThrows(
          IOException.class,
          () -> {
            try (var old = Client.connect(f.config)) {
              fail("old key accepted");
            }
          });
      var cfg = new Client.Config(null, f.socket, "local", fresh, null, f.key.clientId(), null);
      try (var rotated = Client.connect(cfg)) {
        assertEquals(
            "before rotation",
            rotated
                .getMessage(sent.getMessageId())
                .getMessage()
                .getPayload()
                .getData()
                .toStringUtf8());
      }
      String output =
          Fixture.run(
              List.of(
                  f.binary.toString(),
                  "client",
                  "--socket",
                  f.socket.toString(),
                  "--owner",
                  "local",
                  "--key",
                  path.toString(),
                  "--client-id",
                  f.key.clientId()),
              "{\"version\":1,\"type\":\"get_catalog\",\"id\":\"rust\",\"name\":\"mynofy\",\"catalog_version\":1}\n");
      assertTrue(output.contains("\"type\":\"catalog\""));
      String python = System.getenv("MYNOFY_PYTHON");
      if (python != null) {
        String script =
            """
import asyncio,sys
from pathlib import Path
from mynofy import Client,Key
async def main():
    async with await Client.connect_unix(sys.argv[1], owner='local',key=Key(Path(sys.argv[2]).read_bytes()),client_id=sys.argv[3]) as c:
        assert (await c.get_message(sys.argv[4])).message.payload.data == b'before rotation'
asyncio.run(main())
""";
        Fixture.run(
            List.of(
                python,
                "-c",
                script,
                f.socket.toString(),
                path.toString(),
                f.key.clientId(),
                sent.getMessageId()),
            null);
      }
    }
  }
}
