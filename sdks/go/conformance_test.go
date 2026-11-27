package mynofy

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/sha256"
	"crypto/x509"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	pb "mynofy.local/sdk/go/protocol"
)

func repoRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	return root
}
func convertJSON(t *testing.T, raw json.RawMessage) any {
	t.Helper()
	var value any
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.UseNumber()
	if err := decoder.Decode(&value); err != nil {
		t.Fatal(err)
	}
	var visit func(any) any
	visit = func(value any) any {
		switch v := value.(type) {
		case []any:
			for i := range v {
				v[i] = visit(v[i])
			}
		case map[string]any:
			if kind, ok := v["kind"]; ok {
				delete(v, "kind")
				if kind == "event" {
					return map[string]any{"event": visit(v)}
				}
			}
			for k, x := range v {
				v[k] = visit(x)
			}
		}
		return value
	}
	return visit(value)
}
func TestSharedConformance(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), "protocol/conformance/sdk-v1.json"))
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		Frames []struct {
			JSON json.RawMessage `json:"json"`
			Wire string          `json:"protobuf_base64"`
		} `json:"frames"`
		Auth []struct {
			Owner      string `json:"owner"`
			Public     string `json:"public_key"`
			Nonce      string `json:"nonce"`
			Transcript string `json:"transcript_base64"`
			Token      string `json:"token_id"`
		} `json:"auth"`
		Rotation struct {
			Owner     string `json:"owner"`
			Client    string `json:"client_id"`
			RID       string `json:"rotation_id"`
			Old       string `json:"old_token"`
			Until     int64  `json:"until_ms"`
			Public    string `json:"public_key"`
			Challenge string `json:"challenge"`
			AAD       string `json:"aad_base64"`
			Proof     string `json:"proof_base64"`
		} `json:"rotation"`
	}
	if err = json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	for _, item := range v.Frames {
		shape := convertJSON(t, item.JSON).(map[string]any)
		kind := shape["type"].(string)
		delete(shape, "type")
		version := shape["version"]
		delete(shape, "version")
		input, err := json.Marshal(map[string]any{"version": version, kind: shape})
		if err != nil {
			t.Fatal(err)
		}
		frame := new(pb.Frame)
		if err = protojson.Unmarshal(input, frame); err != nil {
			t.Fatalf("%s: %v", kind, err)
		}
		encoded, err := proto.Marshal(frame)
		if err != nil {
			t.Fatal(err)
		}
		if b64(encoded) != item.Wire {
			t.Errorf("wire mismatch for %s", kind)
		}
	}
	for _, item := range v.Auth {
		nonce, err := unb64(item.Nonce, 32)
		if err != nil {
			t.Fatal(err)
		}
		public, err := unb64(item.Public, 32)
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(public)
		if "ed25519:"+b64(digest[:]) != item.Token {
			t.Fatal("token ID mismatch")
		}
		if b64(authBytes(nonce, public, item.Owner)) != item.Transcript {
			t.Fatal("auth transcript mismatch")
		}
	}
	r := v.Rotation
	aad, err := rotationAAD(r.Owner, r.Client, r.RID, r.Old, r.Until)
	if err != nil {
		t.Fatal(err)
	}
	if b64(aad) != r.AAD {
		t.Fatal("rotation AAD mismatch")
	}
	public, _ := unb64(r.Public, 32)
	challenge, _ := unb64(r.Challenge, 32)
	if b64(rotationProof(aad, public, challenge)) != r.Proof {
		t.Fatal("rotation proof mismatch")
	}
}
func TestKeyFormats(t *testing.T) {
	key, err := GenerateKey()
	if err != nil {
		t.Fatal(err)
	}
	roundtrip, err := ParseKey(key.PrivateBytes())
	if err != nil || !bytes.Equal(roundtrip.PublicKey(), key.PublicKey()) {
		t.Fatal("v2 roundtrip", err)
	}
	standard, err := x509.MarshalPKCS8PrivateKey(key.private)
	if err != nil {
		t.Fatal(err)
	}
	roundtrip, err = ParseKey(standard)
	if err != nil || !bytes.Equal(roundtrip.PublicKey(), key.PublicKey()) {
		t.Fatal("v1 roundtrip", err)
	}
	corrupt := key.PrivateBytes()
	corrupt[len(corrupt)-1] ^= 1
	if _, err = ParseKey(corrupt); err == nil {
		t.Fatal("accepted inconsistent public key")
	}
	if _, err = ParseKey(append(key.PrivateBytes(), 0)); err == nil {
		t.Fatal("accepted trailing data")
	}
	if len(key.private) != ed25519.PrivateKeySize {
		t.Fatal("not Ed25519")
	}
}

func TestRotationUnicodeAAD(t *testing.T) {
	aad, err := rotationAAD("owner\u2028\u2029<>&\\u2028", "client", "rid", "old", 1)
	expected := "[\"mynofy-rotation-v1\",\"owner\u2028\u2029<>&\\\\u2028\",\"client\",\"rid\",\"old\",1]"
	if err != nil || string(aad) != expected {
		t.Fatalf("noncanonical Unicode AAD: %q %v", aad, err)
	}
}

func TestSharedPayloadValidation(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), "protocol/conformance/sdk-v1.json"))
	if err != nil {
		t.Fatal(err)
	}
	var vectors struct {
		Cases []struct {
			Name    string          `json:"name"`
			Valid   bool            `json:"valid"`
			Payload json.RawMessage `json:"payload"`
		} `json:"payload_validation"`
	}
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatal(err)
	}
	if len(vectors.Cases) == 0 {
		t.Fatal("missing payload vectors")
	}
	for _, item := range vectors.Cases {
		t.Run(item.Name, func(t *testing.T) {
			var payload pb.Payload
			if err := protojson.Unmarshal(item.Payload, &payload); err != nil {
				t.Fatal(err)
			}
			if err := validatePayload(&payload); (err == nil) != item.Valid {
				t.Fatalf("valid=%v, error=%v", item.Valid, err)
			}
			if !item.Valid {
				if _, err := (&Client{}).Publish(context.Background(), "local/events", &payload, nil); err == nil {
					t.Fatal("invalid publish reached transport")
				}
			}
		})
	}
}

func TestBootIdentitySharedVectors(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), "protocol/conformance/sdk-v1.json"))
	if err != nil {
		t.Fatal(err)
	}
	var vectors struct {
		BootIdentity []struct {
			Name  string
			Value *string
			Valid bool
		} `json:"boot_identity"`
	}
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatal(err)
	}
	for _, row := range vectors.BootIdentity {
		t.Run(row.Name, func(t *testing.T) {
			_, err := validatedBootID(row.Value)
			if (err == nil) != row.Valid {
				t.Fatalf("boot identity: %v", err)
			}
		})
	}
}
