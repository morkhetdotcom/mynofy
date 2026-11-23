package dev.mynofy;

import static org.junit.jupiter.api.Assertions.*;

import com.google.gson.*;
import com.google.protobuf.util.JsonFormat;
import dev.mynofy.protocol.Wire;
import java.nio.file.*;
import java.security.*;
import java.util.Arrays;
import org.junit.jupiter.api.Test;

class ConformanceTest {
  static final Path ROOT = Path.of("../..").toAbsolutePath().normalize();

  @Test
  void sharedWireAndCryptoVectors() throws Exception {
    JsonObject vectors =
        JsonParser.parseString(Files.readString(ROOT.resolve("protocol/conformance/sdk-v1.json")))
            .getAsJsonObject();
    for (var element : vectors.getAsJsonArray("frames")) {
      var item = element.getAsJsonObject();
      var shape = item.getAsJsonObject("json").deepCopy();
      String kind = shape.remove("type").getAsString();
      var version = shape.remove("version");
      JsonObject envelope = new JsonObject();
      envelope.add("version", version);
      envelope.add(kind, convert(shape));
      var frame = Wire.Frame.newBuilder();
      JsonFormat.parser().merge(envelope.toString(), frame);
      assertEquals(
          item.get("protobuf_base64").getAsString(), Key.b64(frame.build().toByteArray()), kind);
    }
    for (var element : vectors.getAsJsonArray("auth")) {
      var item = element.getAsJsonObject();
      byte[] pub = Key.decode(item.get("public_key").getAsString(), 32);
      assertEquals(
          item.get("token_id").getAsString(),
          "ed25519:" + Key.b64(MessageDigest.getInstance("SHA-256").digest(pub)));
      byte[] data =
          Key.authBytes(
              Key.decode(item.get("nonce").getAsString(), 32),
              pub,
              item.get("owner").getAsString());
      assertEquals(item.get("transcript_base64").getAsString(), Key.b64(data));
    }
    var r = vectors.getAsJsonObject("rotation");
    byte[] aad =
        Client.rotationAAD(
            r.get("owner").getAsString(),
            r.get("client_id").getAsString(),
            r.get("rotation_id").getAsString(),
            r.get("old_token").getAsString(),
            r.get("until_ms").getAsLong());
    assertEquals(r.get("aad_base64").getAsString(), Key.b64(aad));
    assertEquals(
        r.get("proof_base64").getAsString(),
        Key.b64(
            Client.rotationProof(
                aad,
                Key.decode(r.get("public_key").getAsString(), 32),
                Key.decode(r.get("challenge").getAsString(), 32))));
  }

  @Test
  void sharedPayloadValidation() throws Exception {
    var vectors =
        JsonParser.parseString(Files.readString(ROOT.resolve("protocol/conformance/sdk-v1.json")))
            .getAsJsonObject();
    var cases = vectors.getAsJsonArray("payload_validation");
    assertFalse(cases.isEmpty());
    for (var element : cases) {
      var item = element.getAsJsonObject();
      var builder = Wire.Payload.newBuilder();
      JsonFormat.parser().merge(item.get("payload").toString(), builder);
      var payload = builder.build();
      if (item.get("valid").getAsBoolean())
        assertDoesNotThrow(() -> Client.validate(payload), item.get("name").getAsString());
      else
        assertThrows(
            IllegalArgumentException.class,
            () -> Client.validate(payload),
            item.get("name").getAsString());
    }
  }

  private JsonElement convert(JsonElement element) {
    if (element.isJsonArray()) {
      var result = new JsonArray();
      for (var v : element.getAsJsonArray()) result.add(convert(v));
      return result;
    }
    if (!element.isJsonObject()) return element;
    var result = element.getAsJsonObject().deepCopy();
    if (result.has("kind")) {
      String kind = result.remove("kind").getAsString();
      if (kind.equals("event")) {
        var wrapped = new JsonObject();
        wrapped.add("event", convert(result));
        return wrapped;
      }
    }
    for (String key : result.keySet().toArray(String[]::new))
      result.add(key, convert(result.get(key)));
    return result;
  }

  @Test
  void keyFormatsAndDefensiveCopies() throws Exception {
    Key key = Key.generate();
    assertArrayEquals(key.publicKey(), Key.parse(key.privateBytes()).publicKey());
    var der = key.privateBytes();
    byte[] v1 = Arrays.copyOf(der, 48);
    v1[1] = 0x2e;
    v1[4] = 0;
    assertArrayEquals(key.publicKey(), Key.parse(v1, key.publicKey()).publicKey());
    assertThrows(GeneralSecurityException.class, () -> Key.parse(v1));
    der[82] ^= 1;
    assertThrows(GeneralSecurityException.class, () -> Key.parse(der));
    assertThrows(
        GeneralSecurityException.class, () -> Key.parse(Arrays.copyOf(key.privateBytes(), 84)));
    assertThrows(GeneralSecurityException.class, () -> Key.parse(v1, Key.generate().publicKey()));
    byte[] exported = key.publicKey();
    exported[0] ^= 1;
    assertFalse(Arrays.equals(exported, key.publicKey()));
  }

  @Test
  void unicodeRotationTranscript() {
    byte[] aad = Client.rotationAAD("owner\u2028\u2029<>&\\u2028", "client", "rid", "old", 1);
    assertEquals(
        "[\"mynofy-rotation-v1\",\"owner\u2028\u2029<>&\\\\u2028\",\"client\",\"rid\",\"old\",1]",
        new String(aad, java.nio.charset.StandardCharsets.UTF_8));
  }

  @Test
  void validationPreservesUnsignedValues() {
    assertThrows(IllegalArgumentException.class, () -> Client.bytes(new byte[4097], null));
    assertThrows(IllegalArgumentException.class, () -> Client.bytes(new byte[1], ""));
    var event =
        Wire.Event.newBuilder()
            .setCatalog(1)
            .setCatalogVersion(1)
            .setCategory(12)
            .setAction(42)
            .setStatus(2)
            .addParameters(Wire.Parameter.newBuilder().setU64(-1L))
            .addParameters(Wire.Parameter.newBuilder().setI64(Long.MIN_VALUE))
            .build();
    assertEquals(
        "18446744073709551615",
        Long.toUnsignedString(Client.event(event).getEvent().getParameters(0).getU64()));
    assertThrows(
        IllegalArgumentException.class,
        () -> Client.event(event.toBuilder().setCategory(999).build()));
    assertThrows(
        IllegalArgumentException.class,
        () ->
            Client.event(
                event.toBuilder()
                    .addParameters(Wire.Parameter.newBuilder().setF64(Double.NaN))
                    .build()));
    assertThrows(
        IllegalArgumentException.class,
        () -> Client.Config.wss(java.net.URI.create("ws://localhost"), "local", null, null));
  }
}
