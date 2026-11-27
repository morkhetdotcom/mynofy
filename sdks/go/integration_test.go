package mynofy

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"errors"
	"io"
	"math"
	"math/big"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"google.golang.org/protobuf/proto"
	pb "mynofy.local/sdk/go/protocol"
)

const topic = "local/events"

type fixture struct {
	dir, binary string
	key, other  *Key
	cfg         Config
}

func rustBinary(t *testing.T) string {
	t.Helper()
	if value := os.Getenv("MYNOFY_BIN"); value != "" {
		return value
	}
	return filepath.Join(repoRoot(t), "target/debug/mynofy")
}
func startFixture(t *testing.T, mode string) *fixture {
	t.Helper()
	if os.Getuid() == 0 {
		t.Fatal("run real service integration tests under a normal UID")
	}
	f := &fixture{dir: t.TempDir(), binary: rustBinary(t)}
	if err := os.Chmod(f.dir, 0700); err != nil {
		t.Fatal(err)
	}
	keypath := filepath.Join(f.dir, "rust.pk8")
	if data, err := exec.Command(f.binary, "keygen", "--out", keypath).CombinedOutput(); err != nil {
		t.Fatalf("Rust keygen: %v %s", err, data)
	}
	keybytes, err := os.ReadFile(keypath)
	if err != nil {
		t.Fatal(err)
	}
	f.key, err = ParseKey(keybytes)
	if err != nil {
		t.Fatal(err)
	}
	f.other, err = GenerateKey()
	if err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), "config/scenarios/unix.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cfg map[string]any
	if err = json.Unmarshal(raw, &cfg); err != nil {
		t.Fatal(err)
	}
	socket := filepath.Join(f.dir, "m.sock")
	cfg["service"] = map[string]any{"id": "local", "unix_socket": socket}
	f.cfg = Config{Owner: "local", Key: f.key, Socket: socket, Timeout: 2 * time.Second}
	var address string
	if mode == "wss" {
		listener, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		address = listener.Addr().String()
		listener.Close()
		_, port, _ := net.SplitHostPort(address)
		cert, key := certificate(t)
		ca := filepath.Join(f.dir, "ca.pem")
		keyfile := filepath.Join(f.dir, "server.key")
		if err = os.WriteFile(ca, cert, 0600); err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(keyfile, key, 0600); err != nil {
			t.Fatal(err)
		}
		cfg["service"] = map[string]any{"id": "local", "listen": address, "tls": map[string]any{"certificate_file": ca, "private_key_file": keyfile}}
		f.cfg = Config{Owner: "local", Key: f.key, URL: "wss://localhost:" + port, CAFile: ca, Timeout: 2 * time.Second}
	}
	clients := []any{}
	for _, key := range []*Key{f.key, f.other} {
		clients = append(clients, map[string]any{"client_id": key.ClientID(), "keys": []any{map[string]any{"algorithm": "ed25519", "public_key": b64(key.PublicKey()), "valid_until": "2035-01-01T00:00:00Z"}}})
	}
	cfg["clients"] = clients
	topics := cfg["topics"].([]any)
	topics[0].(map[string]any)["acl"] = []any{map[string]any{"client_id": f.key.ClientID(), "allow": []string{"publish", "subscribe", "inspect_own"}}}
	limits := cfg["limits"].(map[string]any)
	limits["client_burst"] = 1000
	limits["client_requests_per_second"] = 1000
	cfg["security_storage"] = map[string]any{"directory": "security"}
	data, err := json.Marshal(cfg)
	if err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(f.dir, "config.json")
	if err = os.WriteFile(file, data, 0600); err != nil {
		t.Fatal(err)
	}
	if output, err := exec.Command(f.binary, "security-init", "--config", file).CombinedOutput(); err != nil {
		t.Fatalf("initialize security: %v %s", err, output)
	}
	cmd := exec.Command(f.binary, "serve", "--config", file)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	cmd.Stdout = io.Discard
	if err = cmd.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	var waitErr error
	go func() { waitErr = cmd.Wait(); close(done) }()
	t.Cleanup(func() {
		cmd.Process.Signal(syscall.SIGTERM)
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			cmd.Process.Kill()
			<-done
		}
	})
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		select {
		case <-done:
			t.Fatalf("service stopped: %v %s", waitErr, stderr.String())
		default:
		}
		if mode == "unix" {
			if _, err = os.Stat(socket); err == nil {
				return f
			}
		} else {
			conn, err := net.DialTimeout("tcp", address, 50*time.Millisecond)
			if err == nil {
				conn.Close()
				return f
			}
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("startup deadline")
	return nil
}
func certificate(t *testing.T) ([]byte, []byte) {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	template := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "localhost"}, DNSNames: []string{"localhost"}, NotBefore: time.Now().Add(-time.Minute), NotAfter: time.Now().Add(time.Hour), IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	private, err := x509.MarshalPKCS8PrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: private})
}
func mustDial(t *testing.T, cfg Config) *Client {
	t.Helper()
	client, err := Dial(context.Background(), cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { client.Close() })
	return client
}
func TestNativeRoundtrip(t *testing.T) {
	for _, mode := range []string{"unix", "wss"} {
		t.Run(mode, func(t *testing.T) {
			f := startFixture(t, mode)
			ctx := context.Background()
			c := mustDial(t, f.cfg)
			if c.Info().BootID == "" {
				t.Fatal("missing boot identity")
			}
			if c.Info().ClientID != f.key.ClientID() {
				t.Fatal("identity mismatch")
			}
			catalog, err := c.GetCatalog(ctx, "mynofy", 1)
			if err != nil || !bytes.Contains(catalog, []byte(`"catalog_id"`)) {
				t.Fatal("catalog", err)
			}
			rights, err := c.GetPermissions(ctx, topic)
			if err != nil || !strings.Contains(strings.Join(rights.Actions, ","), "publish") {
				t.Fatal("permissions", err)
			}
			opaque := []byte{0, 255, 'h', 'i'}
			first, err := c.Publish(ctx, topic, Bytes(opaque, "application/octet-stream"), nil)
			if err != nil {
				t.Fatal(err)
			}
			event := &pb.Event{Catalog: proto.Uint32(1), CatalogVersion: proto.Uint32(1), Category: proto.Uint32(12), Action: proto.Uint32(42), Status: proto.Uint32(2), Parameters: []*pb.Parameter{{Value: &pb.Parameter_U64{U64: math.MaxUint64}}, {Value: &pb.Parameter_I64{I64: math.MinInt64}}, {Value: &pb.Parameter_Bool{Bool: false}}}}
			second, err := c.Publish(ctx, topic, Event(event), nil)
			if err != nil {
				t.Fatal(err)
			}
			page, err := c.Poll(ctx, topic, "", 1)
			if err != nil || len(page.Items) != 1 || !page.GetHasMore() {
				t.Fatal("first page", err)
			}
			if page.Items[0].Message.GetProducerId() != c.Info().ClientID {
				t.Fatal("producer mismatch")
			}
			if !bytes.Equal(page.Items[0].Message.Payload.GetData(), opaque) {
				t.Fatal("opaque data changed")
			}

			next, err := c.Poll(ctx, topic, page.GetNextCursor(), 20)
			if err != nil || len(next.Items) != 1 || !proto.Equal(next.Items[0].Message.Payload.GetEvent(), event) {
				t.Fatal("event changed", err)
			}

			status, err := c.GetMessage(ctx, first.GetMessageId())
			if err != nil || status.Message.GetId() != first.GetMessageId() {
				t.Fatal("first message", err)
			}
			status, err = c.GetMessage(ctx, second.GetMessageId())
			if err != nil || status.Message.GetId() != second.GetMessageId() {
				t.Fatal("second message", err)
			}
			own, err := c.ListMessages(ctx, ListOptions{Topic: topic, Limit: 1})
			if err != nil || len(own.Items) != 1 || own.NextCursor == nil {
				t.Fatal("own page", err)
			}
			own, err = c.ListMessages(ctx, ListOptions{Topic: topic, Limit: 1, Cursor: own.GetNextCursor()})
			if err != nil || len(own.Items) != 1 {
				t.Fatal("own continuation", err)
			}
			empty, err := c.Poll(ctx, topic, next.GetNextCursor(), 20)
			if err != nil || len(empty.Items) != 0 {
				t.Fatal("cursor", err)
			}
			cfg := f.cfg
			cfg.Key = f.other
			denied := mustDial(t, cfg)
			_, err = denied.Publish(ctx, topic, Bytes([]byte("forbidden"), ""), nil)
			var refusal *BrokerError
			if !errors.As(err, &refusal) || refusal.Code != "forbidden" {
				t.Fatal("ACL not enforced", err)
			}
			if _, err = denied.GetCatalog(ctx, "mynofy", 1); err != nil {
				t.Fatal(err)
			}
		})
	}
}
func TestValidationAndAuthentication(t *testing.T) {
	f := startFixture(t, "unix")
	c := mustDial(t, f.cfg)
	ctx := context.Background()
	for _, payload := range []*pb.Payload{nil, Bytes(make([]byte, 4097), ""), Event(&pb.Event{Catalog: proto.Uint32(1), CatalogVersion: proto.Uint32(1), Category: proto.Uint32(999), Action: proto.Uint32(42), Status: proto.Uint32(2)})} {
		if _, err := c.Publish(ctx, topic, payload, nil); err == nil {
			t.Fatal("accepted invalid payload")
		}
	}
	if _, err := c.Poll(ctx, topic, "not-a-cursor", 20); err == nil {
		t.Fatal("accepted malformed cursor")
	}
	page, err := c.Poll(ctx, topic, "", 20)
	if err != nil || len(page.Items) != 0 {
		t.Fatal("validation had side effects", err)
	}
	until := time.Now().Add(30 * time.Second)
	if _, err = c.Publish(ctx, topic, Bytes(make([]byte, 4096), ""), &until); err != nil {
		t.Fatal(err)
	}
	for _, mode := range []string{"owner", "identity", "key"} {
		cfg := f.cfg
		switch mode {
		case "owner":
			cfg.Owner = "wrong"
		case "identity":
			cfg.ClientID = "client:wrong"
		case "key":
			cfg.Key, _ = GenerateKey()
		}
		client, err := Dial(ctx, cfg)
		if err == nil {
			client.Close()
			t.Fatalf("accepted wrong %s", mode)
		}
	}
}
func TestTLSVerification(t *testing.T) {
	f := startFixture(t, "wss")
	for _, mode := range []string{"hostname", "ca", "plaintext"} {
		cfg := f.cfg
		switch mode {
		case "hostname":
			cfg.URL = strings.ReplaceAll(cfg.URL, "localhost", "127.0.0.1")
		case "ca":
			cfg.CAFile = ""
		case "plaintext":
			cfg.URL = strings.ReplaceAll(cfg.URL, "wss:", "ws:")
		}
		if c, err := Dial(context.Background(), cfg); err == nil {
			c.Close()
			t.Fatalf("accepted %s", mode)
		}
	}
}
func TestRotationAndCrossSDKKeys(t *testing.T) {
	f := startFixture(t, "unix")
	ctx := context.Background()
	c := mustDial(t, f.cfg)
	published, err := c.Publish(ctx, topic, Bytes([]byte("before rotation"), ""), nil)
	if err != nil {
		t.Fatal(err)
	}
	fresh, err := GenerateKey()
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(f.dir, "new.pk8")
	if err = os.WriteFile(path, fresh.PrivateBytes(), 0600); err != nil {
		t.Fatal(err)
	}
	complete, err := c.Rotate(ctx, fresh)
	if err != nil || complete.GetClientId() != f.key.ClientID() {
		t.Fatal("rotation", err)
	}
	if old, err := Dial(ctx, f.cfg); err == nil {
		old.Close()
		t.Fatal("old key accepted")
	}
	cfg := f.cfg
	cfg.Key = fresh
	cfg.ClientID = f.key.ClientID()
	rotated := mustDial(t, cfg)
	status, err := rotated.GetMessage(ctx, published.GetMessageId())
	if err != nil || !bytes.Equal(status.Message.Payload.GetData(), []byte("before rotation")) {
		t.Fatal("ownership lost", err)
	}
	cmd := exec.Command(f.binary, "client", "--socket", cfg.Socket, "--owner", "local", "--key", path, "--client-id", cfg.ClientID)
	cmd.Stdin = strings.NewReader("{\"version\":1,\"type\":\"get_catalog\",\"id\":\"rust\",\"name\":\"mynofy\",\"catalog_version\":1}\n")
	if data, err := cmd.CombinedOutput(); err != nil || !bytes.Contains(data, []byte(`"type":"catalog"`)) {
		t.Fatalf("Rust key interoperability: %v %s", err, data)
	}
	if python := os.Getenv("MYNOFY_PYTHON"); python != "" {
		script := `import asyncio,sys
from pathlib import Path
from mynofy import Client,Key
async def main():
 async with await Client.connect_unix(sys.argv[1],owner="local",key=Key(Path(sys.argv[2]).read_bytes()),client_id=sys.argv[3]) as c:
  assert (await c.get_message(sys.argv[4])).message.payload.data == b"before rotation"
asyncio.run(main())`
		cmd = exec.Command(python, "-c", script, cfg.Socket, path, cfg.ClientID, published.GetMessageId())
		if data, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("Python interoperability: %v %s", err, data)
		}
	}
}
