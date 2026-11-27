# mynofy Go-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Native client from Go 1.27.4 for direct WSS and Unix connections. No CLI subprocesses in SDK.
The API and the common prototype v1 protocol are still variable prototypes.

## Einbinden

The module has not yet been published. Use a local repository path in your own Go project:

```sh
go mod edit -require=mynofy.local/sdk/go@v0.0.0
go mod edit -replace=mynofy.local/sdk/go=/absoluter/pfad/mynofy/sdks/go
go mod tidy
```

The three direct dependencies are [`coder/websocket`](https://github.com/coder/websocket),
[`golang.org/x/sys`](https://pkg.go.dev/golang.org/x/sys) and the official
[Protobuf-Runtime](https://pkg.go.dev/google.golang.org/protobuf). Ed25519, TLS and AES - GCM
come from the Go standard library. Generated protobuf types are included; Python, protoc and
gRPC are not runtime dependencies.

<a id="lokal-senden-und-lesen"></a>

## Send locally and read

The public key must be registered with the owner and have the necessary topic rights. `mynofy
keygen` creates compatible keys. Client and Unix service run as the same normal OS user.

```go
package main

import (
    "context"
    "fmt"
    "os"

    mynofy "mynofy.local/sdk/go"
)

func run() error {
    der, err := os.ReadFile(".local/unix/client.pk8")
    if err != nil { return err }
    key, err := mynofy.ParseKey(der)
    if err != nil { return err }
    ctx := context.Background()
    client, err := mynofy.Dial(ctx, mynofy.Config{
        Socket: ".local/unix/run/mynofy.sock", Owner: "local", Key: key,
    })
    if err != nil { return err }
    defer client.Close()
    fmt.Println(client.Info()) // Owner, stable client ID, restart counter and optional boot ID
    sent, err := client.Publish(ctx, "local/events",
        mynofy.Bytes([]byte("Hallo"), "text/plain"), nil)
    if err != nil { return err }
    page, err := client.Poll(ctx, "local/events", "", 20)
    if err != nil { return err }
    for _, item := range page.Items {
        fmt.Println(item.Message.GetId(), item.Message.Payload)
        // After Verstehen/Zuordnen; no professional degree:
    }
    fmt.Println(sent.GetMessageId(), page.GetNextCursor(), page.GetHasMore())
    return nil
}
func main() {
    if err := run(); err != nil { fmt.Fprintln(os.Stderr, err); os.Exit(1) }
}
```

`Poll` does not start a background loop. `has_more` more pages immediately
Retrieve. Save the cursor only after own processing or permanent takeover. Read it again without
cursor after a known service restart using `Info().RestartCounter` and compare it with local
state. No automatic professional confirmation, no status file and no automatic repetition of
professional actions. Poll reads without recording reader state; the client keeps its progress
locally.

<a id="wss-und-weitere-operationen"></a>

## WSS and other operations

Set WSS `URL: "wss://localhost:8080"` instead of `Socket`, optional `CAFile:
".local/demo/ca.crt"`. Without CA file, the system trustroots apply. TLS at least 1.3, hostname
check, no unsafe option, redirects or proxy environment variables. Unix checks socket mode 0600,
directory protection and kernel peer UID below Linux/macOS; missing peer check leads to
cancellation. Native macOS tests are pending; the Darwin code is crosscompiled.

All operations take a `context.Context`. `Config.Timeout` limits connection setup including
authentication and one exchange each; default ten seconds.

- `Publish(ctx, topic, payload, expires)`: maximum 4096 raw bytes or one event; `expires` is
  optional `*time.Time`, otherwise the topic policy will decide.
- `Event(*protocol.Event)`: Catalogue 1/v1, valid enum IDs, up to three typed parameters.
  Protobuf gets `uint64` and `int64` without floating point conversion.
- `ListMessages(ctx, ListOptions{Topic:..., Limit: 20})`: own active messages; keep the same
  filter ** and the same page size** when paging.
- `GetMessage(ctx, id)`, `GetPermissions(ctx, topic)` and `GetCatalog(ctx, "mynofy", 1)`: the
  latter yields `json.RawMessage` with verified catalog identity. All remaining answers are
  generated prototype types from `mynofy.local/sdk/go/protocol`; optional fields are pointers.

Payloads and their byte slices do not change in parallel during a call. `Bytes` takes over the
slice without a precautionary copy.

<a id="fehler-und-schlüsselwechsel"></a>

## Mistakes and key changes

`errors.As` distinguishes `*BrokerError` (code, detail, optional RetryAfterMS) and
`*TransportError`. Bei I/O-/protocol errors The connection is discarded.
`OutcomeUnknown` means that a mutation has already been assumed
Context cancellation and timeout are testable with `errors.Is`. `ErrBusy` denies simultaneous
operations without queue; `Close` also cancels an ongoing exchange. No automatic retries,
reconnections or backoff strategies.

`GenerateKey`, `PublicKey`, `TokenID`, `ClientID` liefern registration data;
`PrivateBytes` exportiert Rust/Python-kompatibles PKCS#8 v2. `ParseKey` akzeptiert
canonical PKCS #8 v1 and the thoroughly checked Rust v2 structure including matching of the
included public key. Store private key files with 0600. Go does not guarantee complete deletion
of all key copies from memory.

`Rotate(ctx, newKey)` assumes that the new key is already stored securely
Success closes the old session; reconnect with new key and original `Config.ClientID`; keep both
keys and check the new access if the commit is unclear; the SDK does not write files itself.

<a id="tests-und-generierung"></a>

## Testing and generation

As a normal user with Rust/Cargo and Go:

```sh
./scripts/test-go-sdk.sh
# Optional additional test with installed Python SDK:
MYNOFY_PYTHON=/pfad/zum/venv/bin/python ./scripts/test-go-sdk.sh
```

The tests start temporary Rust services and check Unix/WSS, TLS/Auth/ACL, Bytes/Events, integer
limits, cursor, rotation and Rust key interop. Common Wire-/Auth-/Rotationsvektoren apply to
Rust, Python and Go. Error peers check response loss, false frames, timeout, cancellation and
busy; `-race` check concurrent accesses. Existing demo data is unchanged.

Regenerate bindings only from the canonical `.proto` source and the catalog:

```sh
go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.12
# Set up Python development environment as in the Python SDK:
PROTOC_GEN_GO="$(go env GOPATH)/bin/protoc-gen-go" python sdks/go/generate.py
```

Kein Peer-/Owner-Tunnel, admin access, or Workflow-/Polling-Helfer In the first go-- SDK For
owner tunnels, the Rust client remains available. [SDK - Overview](../../docs/sdks.md).

## Owner-authenticated producer identity

Read `item.Message.GetProducerId()` for the stable client identity stamped by the owner. It is immutable, survives authorized key rotation and storage restore, and is separate from any sender or actor claim inside the payload. Match workflow roles to this value. This is an assertion over the protected owner connection, not a portable cryptographic message signature.

For cursor continuity, compare `ConnectionInfo.BootID` between authenticated sessions. An empty value indicates a legacy owner.
