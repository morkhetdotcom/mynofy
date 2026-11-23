// Package mynofy implements direct authenticated WSS/Unix clients, without workflow automation.
package mynofy

import (
	"context"
	"crypto/ed25519"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"github.com/coder/websocket"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	pb "mynofy.local/sdk/go/protocol"
)

var ErrBusy = errors.New("one operation per client; operation not queued")
var ErrClosed = errors.New("client closed")

type BrokerError struct {
	Code, Detail string
	RetryAfterMS *uint64
}

func (e *BrokerError) Error() string { return e.Code + ": " + e.Detail }

type TransportError struct {
	Err            error
	OutcomeUnknown bool
}

func (e *TransportError) Error() string { return "mynofy transport: " + e.Err.Error() }
func (e *TransportError) Unwrap() error { return e.Err }

type Config struct {
	URL, Socket, Owner, CAFile, ClientID string
	Key                                  *Key
	Timeout                              time.Duration
}
type ConnectionInfo struct {
	Owner, ClientID string
	RestartCounter  uint32
}
type Client struct {
	ws           *websocket.Conn
	key          *Key
	info         ConnectionInfo
	timeout      time.Duration
	busy, closed atomic.Bool
	sequence     uint64
}

// Dial opens exactly one configured transport. No proxy environment, redirect or insecure TLS fallback.
func Dial(ctx context.Context, cfg Config) (*Client, error) {
	if !validName(cfg.Owner) || cfg.Key == nil || len(cfg.Key.private) != ed25519.PrivateKeySize || (cfg.URL == "") == (cfg.Socket == "") || cfg.Timeout < 0 {
		return nil, errors.New("invalid connection configuration")
	}
	if cfg.Timeout == 0 {
		cfg.Timeout = 10 * time.Second
	}
	ctx, cancel := context.WithTimeout(ctx, cfg.Timeout)
	defer cancel()
	transport := &http.Transport{Proxy: nil}
	uri := cfg.URL
	if cfg.Socket != "" {
		parent, err := filepath.EvalSymlinks(filepath.Dir(cfg.Socket))
		if err != nil {
			return nil, err
		}
		meta, err := os.Stat(parent)
		if err != nil {
			return nil, err
		}
		if !meta.IsDir() || meta.Mode().Perm()&0022 != 0 {
			return nil, errors.New("unsafe Unix socket parent")
		}
		path := filepath.Join(parent, filepath.Base(cfg.Socket))
		meta, err = os.Lstat(path)
		if err != nil {
			return nil, err
		}
		if meta.Mode()&os.ModeSocket == 0 || meta.Mode().Perm() != 0600 {
			return nil, errors.New("expected Unix socket with mode 0600")
		}
		transport.DialContext = func(ctx context.Context, _, _ string) (net.Conn, error) {
			conn, err := (&net.Dialer{}).DialContext(ctx, "unix", path)
			if err != nil {
				return nil, err
			}
			uid, err := peerUID(conn.(*net.UnixConn))
			if err != nil || uid != uint32(os.Getuid()) {
				conn.Close()
				if err != nil {
					return nil, err
				}
				return nil, errors.New("Unix peer UID mismatch")
			}
			return conn, nil
		}
		uri = "ws://localhost/"
	} else {
		u, err := url.Parse(uri)
		if err != nil || u.Scheme != "wss" || u.Hostname() == "" || u.User != nil || u.Fragment != "" {
			return nil, errors.New("expected wss:// URL without credentials or fragment")
		}
		config := &tls.Config{MinVersion: tls.VersionTLS13}
		if cfg.CAFile != "" {
			data, err := os.ReadFile(cfg.CAFile)
			if err != nil {
				return nil, err
			}
			roots := x509.NewCertPool()
			if !roots.AppendCertsFromPEM(data) {
				return nil, errors.New("invalid CA file")
			}
			config.RootCAs = roots
		}
		transport.TLSClientConfig = config
	}
	httpClient := &http.Client{Transport: transport, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("redirect refused") }}
	ws, response, err := websocket.Dial(ctx, uri, &websocket.DialOptions{HTTPClient: httpClient, CompressionMode: websocket.CompressionDisabled})
	if err != nil {
		if response != nil && response.Body != nil {
			response.Body.Close()
		}
		transport.CloseIdleConnections()
		return nil, err
	}
	ws.SetReadLimit(4 * 1024 * 1024)
	c := &Client{ws: ws, key: cfg.Key, timeout: cfg.Timeout, info: ConnectionInfo{Owner: cfg.Owner}}
	expected := cfg.ClientID
	if expected == "" {
		expected = cfg.Key.ClientID()
	}
	if err := c.authenticate(ctx, expected); err != nil {
		c.Close()
		return nil, err
	}
	return c, nil
}
func (c *Client) Info() ConnectionInfo { return c.info }

// Close aborts an active exchange too. Already accepted mutations are not rolled back.
func (c *Client) Close() error {
	if c.closed.Swap(true) {
		return nil
	}
	return c.ws.CloseNow()
}
func (c *Client) authenticate(ctx context.Context, expected string) error {
	frame, err := c.roundtrip(ctx, "auth_hello", &pb.AuthHello{TokenId: proto.String(c.key.TokenID())}, "auth_challenge", false)
	if err != nil {
		return err
	}
	challenge := frame.GetAuthChallenge()
	if challenge.GetServiceId() != c.info.Owner {
		return errors.New("owner challenge mismatch")
	}
	nonce, err := unb64(challenge.GetNonce(), 32)
	if err != nil {
		return err
	}
	signature := ed25519.Sign(c.key.private, authBytes(nonce, c.key.PublicKey(), c.info.Owner))
	frame, err = c.roundtrip(ctx, "auth_proof", &pb.AuthProof{AuthId: challenge.RequestId, Signature: proto.String(b64(signature))}, "auth_welcome", false)
	if err != nil {
		return err
	}
	welcome := frame.GetAuthWelcome()
	if welcome.GetServiceId() != c.info.Owner || welcome.GetClientId() != expected || welcome.RestartCounter == nil {
		return errors.New("owner/client welcome mismatch")
	}
	c.info.ClientID = expected
	c.info.RestartCounter = welcome.GetRestartCounter()
	return nil
}
func frameFor(kind string, body proto.Message) *pb.Frame {
	frame := &pb.Frame{Version: proto.Uint32(1)}
	field := frame.ProtoReflect().Descriptor().Fields().ByName(protoreflect.Name(kind))
	frame.ProtoReflect().Set(field, protoreflect.ValueOfMessage(body.ProtoReflect()))
	return frame
}
func (c *Client) exchange(ctx context.Context, kind string, body proto.Message, expected string, mutation bool) (*pb.Frame, error) {
	if !c.busy.CompareAndSwap(false, true) {
		return nil, ErrBusy
	}
	defer c.busy.Store(false)
	return c.roundtrip(ctx, kind, body, expected, mutation)
}
func (c *Client) roundtrip(ctx context.Context, kind string, body proto.Message, expected string, mutation bool) (*pb.Frame, error) {
	if c.closed.Load() {
		return nil, ErrClosed
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if c.sequence == math.MaxUint64 {
		return nil, errors.New("request ID space exhausted")
	}
	c.sequence++
	id := fmt.Sprintf("go-%d", c.sequence)
	field := body.ProtoReflect().Descriptor().Fields().ByName("id")
	body.ProtoReflect().Set(field, protoreflect.ValueOfString(id))
	encoded, err := proto.Marshal(frameFor(kind, body))
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()
	fail := func(err error) (*pb.Frame, error) {
		c.Close()
		return nil, &TransportError{Err: err, OutcomeUnknown: mutation}
	}
	if err = c.ws.Write(ctx, websocket.MessageBinary, encoded); err != nil {
		return fail(err)
	}
	typ, data, err := c.ws.Read(ctx)
	if err != nil {
		return fail(err)
	}
	if typ != websocket.MessageBinary {
		return fail(errors.New("expected Protobuf binary response"))
	}
	response := new(pb.Frame)
	if err = proto.Unmarshal(data, response); err != nil {
		return fail(err)
	}
	if err = proto.CheckInitialized(response); err != nil {
		return fail(err)
	}
	replyField := response.ProtoReflect().WhichOneof(response.ProtoReflect().Descriptor().Oneofs().ByName("body"))
	if response.GetVersion() != 1 || replyField == nil || string(replyField.Name()) != expected && replyField.Name() != "error" {
		return fail(errors.New("invalid response envelope"))
	}
	reply := response.ProtoReflect().Get(replyField).Message()
	requestID := reply.Descriptor().Fields().ByName("request_id")
	failure := response.GetError()
	earlyRateLimit := failure != nil && failure.GetCode() == "rate_limited" && failure.RequestId == nil && failure.RetryAfterMs != nil
	if !earlyRateLimit && (requestID == nil || !reply.Has(requestID) || reply.Get(requestID).String() != id) {
		return fail(errors.New("response request ID mismatch"))
	}
	if failure := response.GetError(); failure != nil {
		return nil, &BrokerError{Code: failure.GetCode(), Detail: failure.GetDetail(), RetryAfterMS: failure.RetryAfterMs}
	}
	if page := response.GetMessages(); page != nil {
		for _, item := range page.Items {
			if item.GetMessage().GetProducerId() == "" {
				return fail(errors.New("missing producer identity"))
			}
		}
	}
	if status := response.GetMessageStatus(); status != nil && status.GetMessage().GetProducerId() == "" {
		return fail(errors.New("missing producer identity"))
	}
	if kind == "publish" && (response.GetResult().MessageId == nil || response.GetResult().Expires == nil) {
		return fail(errors.New("incomplete publish result"))
	}
	if kind == "get_message" && response.GetMessageStatus().GetMessage().GetId() != body.(*pb.GetMessage).GetMessageId() {
		return fail(errors.New("response message ID mismatch"))
	}
	return response, nil
}
func validName(s string) bool { return len(s) > 0 && len(s) <= 256 && utf8.ValidString(s) }
func validCursor(s string) bool {
	if len(s) != 8 {
		return false
	}
	for _, b := range []byte(s) {
		if !(b >= '0' && b <= '9' || b >= 'A' && b <= 'Z' || b >= 'a' && b <= 'z') {
			return false
		}
	}
	return true
}
func limit(value uint32) (uint32, error) {
	if value == 0 {
		return 20, nil
	}
	if value > 100 {
		return 0, errors.New("limit must be 1..100")
	}
	return value, nil
}
func Bytes(data []byte, contentType string) *pb.Payload {
	p := &pb.Payload{Body: &pb.Payload_Data{Data: data}}
	if contentType != "" {
		p.ContentType = proto.String(contentType)
	}
	return p
}
func Event(event *pb.Event) *pb.Payload { return &pb.Payload{Body: &pb.Payload_Event{Event: event}} }
func validatePayload(p *pb.Payload) error {
	if p == nil {
		return errors.New("missing payload")
	}
	if err := proto.CheckInitialized(p); err != nil {
		return err
	}
	if p.ContentType != nil && (len(p.GetContentType()) == 0 || len(p.GetContentType()) > 128 || !utf8.ValidString(p.GetContentType())) {
		return errors.New("invalid content type")
	}
	switch body := p.Body.(type) {
	case *pb.Payload_Data:
		if len(body.Data) > 4096 {
			return errors.New("payload exceeds 4096 bytes")
		}
	case *pb.Payload_Event:
		if p.ContentType != nil {
			return errors.New("event payloads must not have a content type")
		}
		e := body.Event
		if e == nil || e.GetCatalog() != 1 || e.GetCatalogVersion() != 1 || !validCategory(e.GetCategory()) || !validAction(e.GetAction()) || !validStatus(e.GetStatus()) || len(e.Parameters) > 3 {
			return errors.New("invalid catalog event")
		}
		for _, p := range e.Parameters {
			if p == nil || p.Value == nil {
				return errors.New("missing event parameter")
			}
			switch v := p.Value.(type) {
			case *pb.Parameter_F32:
				if math.IsNaN(float64(v.F32)) || math.IsInf(float64(v.F32), 0) {
					return errors.New("nonfinite parameter")
				}
			case *pb.Parameter_F64:
				if math.IsNaN(v.F64) || math.IsInf(v.F64, 0) {
					return errors.New("nonfinite parameter")
				}
			}
		}
	default:
		return errors.New("missing payload body")
	}
	return nil
}
func optional(s string) *string {
	if s == "" {
		return nil
	}
	return proto.String(s)
}
func (c *Client) Publish(ctx context.Context, topic string, payload *pb.Payload, expires *time.Time) (*pb.Result, error) {
	if !validName(topic) {
		return nil, errors.New("invalid topic")
	}
	if err := validatePayload(payload); err != nil {
		return nil, err
	}
	request := &pb.Publish{Topic: proto.String(topic), Payload: payload}
	if expires != nil {
		request.Expires = &pb.Expires{Time: proto.String(expires.UTC().Format("2006-01-02T15:04:05.000Z"))}
	}
	frame, err := c.exchange(ctx, "publish", request, "result", true)
	if err != nil {
		return nil, err
	}
	return frame.GetResult(), nil
}
func (c *Client) Poll(ctx context.Context, topic, after string, pageLimit uint32) (*pb.Messages, error) {
	if !validName(topic) || after != "" && !validCursor(after) {
		return nil, errors.New("invalid topic or cursor")
	}
	n, err := limit(pageLimit)
	if err != nil {
		return nil, err
	}
	frame, err := c.exchange(ctx, "poll", &pb.Poll{Topic: proto.String(topic), After: optional(after), Limit: proto.Uint32(n)}, "messages", false)
	if err != nil {
		return nil, err
	}
	page := frame.GetMessages()
	if !validCursor(page.GetNextCursor()) || len(page.Items) > int(n) {
		c.Close()
		return nil, errors.New("invalid poll page")
	}
	return page, nil
}

type ListOptions struct {
	Topic, Cursor string
	Limit         uint32
}

func (c *Client) ListMessages(ctx context.Context, options ListOptions) (*pb.MessagesPage, error) {
	if options.Topic != "" && !validName(options.Topic) || len(options.Cursor) > 4096 || !utf8.ValidString(options.Cursor) {
		return nil, errors.New("invalid producer filter or cursor")
	}
	n, err := limit(options.Limit)
	if err != nil {
		return nil, err
	}
	frame, err := c.exchange(ctx, "list_messages", &pb.ListMessages{Topic: optional(options.Topic), Cursor: optional(options.Cursor), Limit: proto.Uint32(n)}, "messages_page", false)
	if err != nil {
		return nil, err
	}
	return frame.GetMessagesPage(), nil
}
func (c *Client) GetMessage(ctx context.Context, id string) (*pb.MessageStatus, error) {
	if !validName(id) {
		return nil, errors.New("invalid message ID")
	}
	frame, err := c.exchange(ctx, "get_message", &pb.GetMessage{MessageId: proto.String(id)}, "message_status", false)
	if err != nil {
		return nil, err
	}
	return frame.GetMessageStatus(), nil
}
func (c *Client) GetPermissions(ctx context.Context, topic string) (*pb.Permissions, error) {
	if !validName(topic) {
		return nil, errors.New("invalid topic")
	}
	frame, err := c.exchange(ctx, "get_permissions", &pb.GetPermissions{Topic: proto.String(topic)}, "permissions", false)
	if err != nil {
		return nil, err
	}
	return frame.GetPermissions_(), nil
}
func (c *Client) GetCatalog(ctx context.Context, name string, version uint32) (json.RawMessage, error) {
	if !validName(name) || version == 0 {
		return nil, errors.New("invalid catalog")
	}
	frame, err := c.exchange(ctx, "get_catalog", &pb.GetCatalog{Name: proto.String(name), CatalogVersion: proto.Uint32(version)}, "catalog", false)
	if err != nil {
		return nil, err
	}
	raw := frame.GetCatalog_().GetDefinition()
	var identity struct {
		Name    string `json:"name"`
		Version uint32 `json:"version"`
	}
	if err = json.Unmarshal(raw, &identity); err != nil || identity.Name != name || identity.Version != version {
		return nil, errors.New("catalog identity mismatch")
	}
	return json.RawMessage(raw), nil
}
