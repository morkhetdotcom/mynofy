package dev.mynofy;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.*;
import java.security.interfaces.EdECPrivateKey;
import java.security.spec.*;
import java.util.Arrays;
import java.util.Base64;
import java.util.HexFormat;

/** Ed25519 keypair. File persistence remains the application's responsibility. */
public final class Key {
  private static final byte[] V1 = HexFormat.of().parseHex("302e020100300506032b657004220420");
  private static final byte[] V2 = HexFormat.of().parseHex("3051020101300506032b657004220420");
  private static final byte[] SPKI = HexFormat.of().parseHex("302a300506032b6570032100");
  private final PrivateKey privateKey;
  private final byte[] seed, publicKey;

  private Key(byte[] seed, byte[] publicKey) throws GeneralSecurityException {
    this.seed = seed.clone();
    this.publicKey = publicKey.clone();
    var factory = KeyFactory.getInstance("Ed25519");
    privateKey = factory.generatePrivate(new PKCS8EncodedKeySpec(join(V1, seed)));
    var pub = factory.generatePublic(new X509EncodedKeySpec(join(SPKI, publicKey)));
    byte[] challenge = new byte[32];
    new SecureRandom().nextBytes(challenge);
    var verifier = Signature.getInstance("Ed25519");
    verifier.initVerify(pub);
    verifier.update(challenge);
    if (!verifier.verify(sign(challenge)))
      throw new InvalidKeyException("public/private key mismatch");
  }

  public static Key generate() throws GeneralSecurityException {
    var pair = KeyPairGenerator.getInstance("Ed25519").generateKeyPair();
    if (!(pair.getPrivate() instanceof EdECPrivateKey privateKey))
      throw new InvalidKeyException("provider does not expose an Ed25519 seed");
    byte[] seed =
        privateKey.getBytes().orElseThrow(() -> new InvalidKeyException("seed is not exportable"));
    byte[] publicDer = pair.getPublic().getEncoded();
    if (seed.length != 32
        || publicDer == null
        || publicDer.length != 44
        || !Arrays.equals(Arrays.copyOf(publicDer, 12), SPKI))
      throw new InvalidKeyException("unexpected provider Ed25519 representation");
    return new Key(seed, Arrays.copyOfRange(publicDer, 12, 44));
  }

  /** Import the exact Rust/Python/Go PKCS#8-v2 representation, checking key binding. */
  public static Key parse(byte[] der) throws GeneralSecurityException {
    if (der.length != 83
        || !Arrays.equals(Arrays.copyOf(der, 16), V2)
        || der[48] != (byte) 0x81
        || der[49] != 0x21
        || der[50] != 0)
      throw new InvalidKeySpecException(
          "expected exact PKCS#8 v2; v1 needs an explicit public key");
    return new Key(Arrays.copyOfRange(der, 16, 48), Arrays.copyOfRange(der, 51, 83));
  }

  /** Standard v1 lacks a public key; supply its raw 32-byte Ed25519 public key. */
  public static Key parse(byte[] der, byte[] publicKey) throws GeneralSecurityException {
    if (publicKey.length != 32) throw new InvalidKeySpecException("public key must be 32 bytes");
    if (der.length != 48 || !Arrays.equals(Arrays.copyOf(der, 16), V1))
      throw new InvalidKeySpecException("expected canonical PKCS#8 v1");
    return new Key(Arrays.copyOfRange(der, 16, 48), publicKey);
  }

  public byte[] publicKey() {
    return publicKey.clone();
  }

  public String tokenId() {
    try {
      return "ed25519:" + b64(MessageDigest.getInstance("SHA-256").digest(publicKey));
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException(e);
    }
  }

  /** Initial identity; after rotation retain the original identity from ConnectionInfo. */
  public String clientId() {
    return "client:" + tokenId();
  }

  public byte[] privateBytes() {
    return join(V2, seed, new byte[] {(byte) 0x81, 0x21, 0}, publicKey);
  }

  byte[] sign(byte[] data) throws GeneralSecurityException {
    var signer = Signature.getInstance("Ed25519");
    signer.initSign(privateKey);
    signer.update(data);
    return signer.sign();
  }

  static byte[] join(byte[]... pieces) {
    int length = 0;
    for (byte[] p : pieces) length = Math.addExact(length, p.length);
    var out = ByteBuffer.allocate(length);
    for (byte[] p : pieces) out.put(p);
    return out.array();
  }

  static String b64(byte[] data) {
    return Base64.getEncoder().encodeToString(data);
  }

  static byte[] decode(String value, int size) {
    byte[] out = Base64.getDecoder().decode(value);
    if (!b64(out).equals(value) || size >= 0 && out.length != size)
      throw new IllegalArgumentException("invalid canonical Base64");
    return out;
  }

  static byte[] authBytes(byte[] nonce, byte[] publicKey, String owner) {
    byte[] utf8 = owner.getBytes(StandardCharsets.UTF_8);
    return join(
        "mynofy-client-auth-v1\0".getBytes(StandardCharsets.UTF_8),
        nonce,
        publicKey,
        ByteBuffer.allocate(4).putInt(utf8.length).array(),
        utf8);
  }
}
