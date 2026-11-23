package mynofy

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
	"google.golang.org/protobuf/proto"
	pb "mynofy.local/sdk/go/protocol"
)

// Controlled peers exercise failures after a request has reached the wire.
func faultClient(t *testing.T, handler func(*websocket.Conn)) *Client {
	t.Helper()
	done := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer close(done)
		conn, err := websocket.Accept(w, r, nil)
		if err != nil {
			return
		}
		defer conn.CloseNow()
		handler(conn)
	}))
	ws, _, err := websocket.Dial(context.Background(), "ws"+strings.TrimPrefix(server.URL, "http"), nil)
	if err != nil {
		server.Close()
		t.Fatal(err)
	}
	c := &Client{ws: ws, timeout: time.Second, info: ConnectionInfo{Owner: "local"}}
	t.Cleanup(func() {
		c.Close()
		server.Close()
		select {
		case <-done:
		case <-time.After(2 * time.Second):
			t.Error("fault peer did not terminate")
		}
	})
	return c
}
func receive(conn *websocket.Conn) (*pb.Frame, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_, data, err := conn.Read(ctx)
	if err != nil {
		return nil, err
	}
	f := new(pb.Frame)
	err = proto.Unmarshal(data, f)
	return f, err
}
func TestLostPublishResponse(t *testing.T) {
	var received atomic.Int32
	c := faultClient(t, func(conn *websocket.Conn) {
		if f, err := receive(conn); err == nil && f.GetPublish() != nil {
			received.Add(1)
		}
	})
	_, err := c.Publish(context.Background(), topic, Bytes([]byte("once"), ""), nil)
	var failure *TransportError
	if !errors.As(err, &failure) || !failure.OutcomeUnknown {
		t.Fatalf("missing uncertain outcome: %v", err)
	}
	if _, err = c.Publish(context.Background(), topic, Bytes([]byte("again"), ""), nil); !errors.Is(err, ErrClosed) {
		t.Fatalf("connection reused: %v", err)
	}
	if received.Load() != 1 {
		t.Fatalf("sent %d commands", received.Load())
	}
}
func TestCancellationBusyAndClose(t *testing.T) {
	for _, mode := range []string{"cancel", "timeout", "close"} {
		t.Run(mode, func(t *testing.T) {
			started := make(chan struct{})
			c := faultClient(t, func(conn *websocket.Conn) {
				if _, err := receive(conn); err != nil {
					return
				}
				close(started)
				receive(conn)
			})
			c.timeout = 150 * time.Millisecond
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			result := make(chan error, 1)
			go func() { _, err := c.Publish(ctx, topic, Bytes([]byte("pending"), ""), nil); result <- err }()
			select {
			case <-started:
			case <-time.After(time.Second):
				t.Fatal("request missing")
			}
			if _, err := c.Poll(context.Background(), topic, "", 20); !errors.Is(err, ErrBusy) {
				t.Fatalf("concurrent operation queued: %v", err)
			}
			switch mode {
			case "cancel":
				cancel()
			case "close":
				c.Close()
			}
			select {
			case err := <-result:
				var failure *TransportError
				if !errors.As(err, &failure) || !failure.OutcomeUnknown {
					t.Fatalf("missing uncertain outcome: %v", err)
				}
				if mode == "cancel" && !errors.Is(err, context.Canceled) {
					t.Fatalf("cancellation lost: %v", err)
				}
				if mode == "timeout" && !errors.Is(err, context.DeadlineExceeded) {
					t.Fatalf("deadline lost: %v", err)
				}
			case <-time.After(time.Second):
				t.Fatal("operation did not stop")
			}
		})
	}
}
func TestInvalidResponsesDiscardSession(t *testing.T) {
	for _, mode := range []string{"request-id", "version", "text", "malformed", "required"} {
		t.Run(mode, func(t *testing.T) {
			c := faultClient(t, func(conn *websocket.Conn) {
				f, err := receive(conn)
				if err != nil {
					return
				}
				response := frameFor("result", &pb.Result{RequestId: f.GetPublish().Id})
				typ := websocket.MessageBinary
				switch mode {
				case "request-id":
					response.GetResult().RequestId = proto.String("wrong")
				case "version":
					response.Version = proto.Uint32(2)
				case "text":
					typ = websocket.MessageText
				case "required":
					response.Version = nil
				}
				data, _ := proto.MarshalOptions{AllowPartial: true}.Marshal(response)
				if mode == "malformed" {
					data = []byte{255}
				}
				conn.Write(context.Background(), typ, data)
				receive(conn)
			})
			_, err := c.Publish(context.Background(), topic, Bytes([]byte("pending"), ""), nil)
			var failure *TransportError
			if !errors.As(err, &failure) || !failure.OutcomeUnknown {
				t.Fatalf("accepted invalid response: %v", err)
			}
			if _, err = c.Poll(context.Background(), topic, "", 20); !errors.Is(err, ErrClosed) {
				t.Fatalf("broken session reused: %v", err)
			}
		})
	}
}

func TestUncorrelatedRateLimitPreservesConnection(t *testing.T) {
	c := faultClient(t, func(conn *websocket.Conn) {
		for i := 0; i < 2; i++ {
			if _, err := receive(conn); err != nil {
				return
			}
			f := &pb.Frame{Version: proto.Uint32(1), Body: &pb.Frame_Error{Error: &pb.Error{Code: proto.String("rate_limited"), Detail: proto.String("wait"), RetryAfterMs: proto.Uint64(123)}}}
			data, _ := proto.Marshal(f)
			if conn.Write(context.Background(), websocket.MessageBinary, data) != nil {
				return
			}
		}
	})
	for i := 0; i < 2; i++ {
		_, err := c.Publish(context.Background(), topic, Bytes([]byte("rejected"), ""), nil)
		var failure *BrokerError
		if !errors.As(err, &failure) || failure.RetryAfterMS == nil || *failure.RetryAfterMS != 123 {
			t.Fatalf("not a regular broker refusal: %v", err)
		}
	}
}

func TestSharedReplySemantics(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join(repoRoot(t), "protocol/conformance/sdk-v1.json"))
	if err != nil {
		t.Fatal(err)
	}
	var vectors struct {
		Cases []struct {
			Name     string `json:"name"`
			Valid    bool   `json:"valid"`
			Mutation bool   `json:"mutation"`
			Response string `json:"response_base64"`
		} `json:"reply_validation"`
	}
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatal(err)
	}
	if len(vectors.Cases) == 0 {
		t.Fatal("missing response vectors")
	}
	for _, item := range vectors.Cases {
		t.Run(item.Name, func(t *testing.T) {
			bytes, err := base64.StdEncoding.DecodeString(item.Response)
			if err != nil {
				t.Fatal(err)
			}
			var received atomic.Int32
			c := faultClient(t, func(conn *websocket.Conn) {
				n := 1
				if item.Valid {
					n = 2
				}
				for i := 0; i < n; i++ {
					request, err := receive(conn)
					if err != nil {
						t.Error(err)
						return
					}
					received.Add(1)
					response := new(pb.Frame)
					if err := proto.Unmarshal(bytes, response); err != nil {
						t.Error(err)
						return
					}
					if item.Mutation {
						response.GetResult().RequestId = request.GetPublish().Id
					} else {
						response.GetMessageStatus().RequestId = request.GetGetMessage().Id
					}
					data, _ := proto.Marshal(response)
					if err := conn.Write(context.Background(), websocket.MessageBinary, data); err != nil {
						t.Error(err)
						return
					}
				}
				if !item.Valid {
					if _, err := receive(conn); err == nil {
						received.Add(1)
						t.Error("application command sent after invalid response")
					}
				}
			})
			operation := func() error {
				if item.Mutation {
					_, err := c.Publish(context.Background(), topic, Bytes([]byte("once"), ""), nil)
					return err
				}
				_, err := c.GetMessage(context.Background(), "00000001")
				return err
			}
			if item.Valid {
				if err := operation(); err != nil {
					t.Fatal(err)
				}
				if err := operation(); err != nil {
					t.Fatal(err)
				}
			} else {
				err := operation()
				var failure *TransportError
				if !errors.As(err, &failure) || failure.OutcomeUnknown != item.Mutation {
					t.Fatalf("wrong failure: %v", err)
				}
				if err := operation(); !errors.Is(err, ErrClosed) {
					t.Fatalf("reused broken session: %v", err)
				}
			}
			want := int32(1)
			if item.Valid {
				want = 2
			}
			if received.Load() != want {
				t.Fatalf("commands=%d want %d", received.Load(), want)
			}
		})
	}
}
