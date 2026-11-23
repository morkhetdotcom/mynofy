"""Ed25519 identities; all cryptographic operations use pyca/cryptography."""
import base64
import hashlib
import hmac
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode('ascii')


def decode(text: str, length: int | None = None) -> bytes:
    value = base64.b64decode(text, validate=True)
    if b64(value) != text or length is not None and len(value) != length:
        raise ValueError('Invalid canonical Base64 value')
    return value


class Key:
    def __init__(self, der: bytes):
        try:
            key = serialization.load_der_private_key(der, password=None)
        except ValueError:
            # aws-lc emits RFC 5958 OneAsymmetricKey v2 (83 bytes). OpenSSL's
            # PKCS#8 loader may only accept v1. Accept this exact Ed25519 DER
            # structure, then let cryptography derive and verify its public key.
            prefix = bytes.fromhex('3051020101300506032b657004220420')
            if len(der) != 83 or der[:16] != prefix or der[48:51] != b'\x81\x21\x00':
                raise ValueError('Expected an Ed25519 PKCS#8 private key') from None
            key = Ed25519PrivateKey.from_private_bytes(der[16:48])
            if not hmac.compare_digest(key.public_key().public_bytes_raw(), der[51:]):
                raise ValueError('PKCS#8 public key does not match private key') from None
        if not isinstance(key, Ed25519PrivateKey):
            raise ValueError('Expected an Ed25519 PKCS#8 private key')
        self._key = key

    @classmethod
    def generate(cls) -> 'Key':
        key = Ed25519PrivateKey.generate()
        return cls(key.private_bytes(serialization.Encoding.DER,
                                    serialization.PrivateFormat.PKCS8,
                                    serialization.NoEncryption()))

    @property
    def public_key(self) -> bytes:
        return self._key.public_key().public_bytes_raw()

    @property
    def token_id(self) -> str:
        return 'ed25519:' + b64(hashlib.sha256(self.public_key).digest())

    @property
    def client_id(self) -> str:
        return 'client:' + self.token_id

    def private_bytes(self) -> bytes:
        # Export the exact RFC 5958 v2 form used by aws-lc/Rust, including the
        # public key. cryptography performs key derivation; this is DER framing.
        seed = self._key.private_bytes(serialization.Encoding.Raw,
                                      serialization.PrivateFormat.Raw,
                                      serialization.NoEncryption())
        return (bytes.fromhex('3051020101300506032b657004220420') + seed
                + b'\x81\x21\x00' + self.public_key)

    def sign(self, data: bytes) -> bytes:
        return self._key.sign(data)


def auth_bytes(nonce: bytes, public: bytes, owner: str) -> bytes:
    if len(nonce) != 32 or len(public) != 32:
        raise ValueError('Nonce and public key must have 32 bytes')
    encoded = owner.encode('utf-8')
    return b'mynofy-client-auth-v1\0' + nonce + public + len(encoded).to_bytes(4, 'big') + encoded
