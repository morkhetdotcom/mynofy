package mynofy

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"errors"
)

// Key holds an Ed25519 private key. Persistence remains the caller's responsibility.
type Key struct{ private ed25519.PrivateKey }

var pkcs8v2 = []byte{0x30, 0x51, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20}

func GenerateKey() (*Key, error) {
	_, key, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, err
	}
	return &Key{private: key}, nil
}

// ParseKey accepts standard PKCS#8 v1 and the exact RFC5958 v2 form emitted by Rust.
func ParseKey(der []byte) (*Key, error) {
	if len(der) == 83 && bytes.Equal(der[:16], pkcs8v2) && bytes.Equal(der[48:51], []byte{0x81, 0x21, 0}) {
		key := ed25519.NewKeyFromSeed(der[16:48])
		if subtle.ConstantTimeCompare(key.Public().(ed25519.PublicKey), der[51:]) != 1 {
			return nil, errors.New("PKCS#8 public key mismatch")
		}
		return &Key{private: key}, nil
	}
	value, err := x509.ParsePKCS8PrivateKey(der)
	if err != nil {
		return nil, errors.New("invalid Ed25519 PKCS#8 key")
	}
	key, ok := value.(ed25519.PrivateKey)
	if !ok {
		return nil, errors.New("expected Ed25519 key")
	}
	canonical, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil || !bytes.Equal(canonical, der) {
		return nil, errors.New("expected canonical PKCS#8 v1 or exact Rust v2 key")
	}
	return &Key{private: key}, nil
}
func (k *Key) PublicKey() []byte {
	return append([]byte(nil), k.private.Public().(ed25519.PublicKey)...)
}
func (k *Key) TokenID() string  { sum := sha256.Sum256(k.PublicKey()); return "ed25519:" + b64(sum[:]) }
func (k *Key) ClientID() string { return "client:" + k.TokenID() }

// PrivateBytes exports Rust/Python-compatible PKCS#8 v2; store with private permissions.
func (k *Key) PrivateBytes() []byte {
	out := append([]byte(nil), pkcs8v2...)
	out = append(out, k.private.Seed()...)
	out = append(out, 0x81, 0x21, 0)
	return append(out, k.PublicKey()...)
}
func b64(data []byte) string { return base64.StdEncoding.EncodeToString(data) }
func unb64(value string, size int) ([]byte, error) {
	out, err := base64.StdEncoding.Strict().DecodeString(value)
	if err != nil || b64(out) != value || size >= 0 && len(out) != size {
		return nil, errors.New("invalid canonical Base64")
	}
	return out, nil
}
func authBytes(nonce, public []byte, owner string) []byte {
	out := append([]byte("mynofy-client-auth-v1\x00"), nonce...)
	out = append(out, public...)
	out = binary.BigEndian.AppendUint32(out, uint32(len(owner)))
	return append(out, []byte(owner)...)
}
