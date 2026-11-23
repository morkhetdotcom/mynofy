package mynofy

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/json"
	"errors"
	"time"

	"google.golang.org/protobuf/proto"
	pb "mynofy.local/sdk/go/protocol"
)

func rotationAAD(owner, client, rid, old string, until int64) ([]byte, error) {
	var out bytes.Buffer
	encoder := json.NewEncoder(&out)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode([]any{"mynofy-rotation-v1", owner, client, rid, old, until}); err != nil {
		return nil, err
	}
	// Go escapes U+2028/U+2029 even with EscapeHTML disabled; serde_json
	// keeps those UTF-8 characters literal. Preserve escaped backslashes.
	encoded := bytes.TrimSuffix(out.Bytes(), []byte{'\n'})
	var canonical bytes.Buffer
	for i := 0; i < len(encoded); {
		if encoded[i] == '\\' && i+1 < len(encoded) {
			if i+6 <= len(encoded) && (string(encoded[i:i+6]) == `\u2028` || string(encoded[i:i+6]) == `\u2029`) {
				canonical.WriteRune(rune(0x2028) + rune(encoded[i+5]-'8'))
				i += 6
				continue
			}
			canonical.Write(encoded[i : i+2])
			i += 2
			continue
		}
		canonical.WriteByte(encoded[i])
		i++
	}
	return canonical.Bytes(), nil
}
func rotationProof(aad, public, challenge []byte) []byte {
	out := append([]byte("mynofy-rotation-proof-v1\x00"), aad...)
	out = append(out, public...)
	return append(out, challenge...)
}

// Rotate requires the caller to persist newKey first. Retain both keys if the commit outcome is unknown.
// Success closes the old session; explicitly reconnect with newKey and the original ClientID.
func (c *Client) Rotate(ctx context.Context, newKey *Key) (*pb.RotationComplete, error) {
	if newKey == nil || len(newKey.private) != ed25519.PrivateKeySize {
		return nil, errors.New("invalid new key")
	}
	if !c.busy.CompareAndSwap(false, true) {
		return nil, ErrBusy
	}
	defer c.busy.Store(false)
	frame, err := c.roundtrip(ctx, "rotation_start", new(pb.RotationStart), "rotation_offer", true)
	if err != nil {
		return nil, err
	}
	offer := frame.GetRotationOffer()
	secret := make([]byte, 32)
	if _, err = rand.Read(secret); err != nil {
		return nil, err
	}
	frame, err = c.roundtrip(ctx, "rotation_confirm", &pb.RotationConfirm{RotationId: offer.RotationId, SharedSecret: proto.String(b64(secret))}, "rotation_ready", true)
	if err != nil {
		return nil, err
	}
	ready := frame.GetRotationReady()
	mismatch := func() (*pb.RotationComplete, error) {
		c.Close()
		return nil, errors.New("rotation binding mismatch; retain both keys")
	}
	if ready.GetRotationId() != offer.GetRotationId() || ready.GetExpiresAt() != offer.GetExpiresAt() || ready.GetNewValidUntil() != offer.GetNewValidUntil() {
		return mismatch()
	}
	until, err := time.Parse(time.RFC3339Nano, ready.GetNewValidUntil())
	if err != nil {
		return mismatch()
	}
	aad, err := rotationAAD(c.info.Owner, c.info.ClientID, offer.GetRotationId(), c.key.TokenID(), until.UnixMilli())
	if err != nil {
		return mismatch()
	}
	remoteAAD, err := unb64(ready.GetAad(), -1)
	if err != nil || !bytes.Equal(aad, remoteAAD) {
		return mismatch()
	}
	nonce, err := unb64(ready.GetNonce(), 12)
	if err != nil {
		return mismatch()
	}
	challenge, err := unb64(ready.GetChallenge(), 32)
	if err != nil {
		return mismatch()
	}
	block, err := aes.NewCipher(secret)
	if err != nil {
		return nil, err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	encrypted := gcm.Seal(nil, nonce, newKey.PublicKey(), aad)
	proof := ed25519.Sign(newKey.private, rotationProof(aad, newKey.PublicKey(), challenge))
	frame, err = c.roundtrip(ctx, "rotation_commit", &pb.RotationCommit{RotationId: offer.RotationId, EncryptedPublicKey: proto.String(b64(encrypted)), Signature: proto.String(b64(proof))}, "rotation_complete", true)
	if err != nil {
		return nil, err
	}
	complete := frame.GetRotationComplete()
	c.Close()
	if complete.GetRotationId() != offer.GetRotationId() || complete.GetClientId() != c.info.ClientID || complete.GetTokenId() != newKey.TokenID() || complete.GetValidUntil() != ready.GetNewValidUntil() {
		return nil, &TransportError{Err: errors.New("rotation commit not confirmed; retain both keys"), OutcomeUnknown: true}
	}
	return complete, nil
}
