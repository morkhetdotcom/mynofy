# Getting started

This guide runs one local service and two CLI sessions. The same API works over
WSS when participants run on different machines.

## Requirements

- Linux or macOS, running as a normal OS user.
- Rust 1.88+, a C compiler, and CMake for the TLS cryptography provider.
- Python 3 for the setup helpers; OpenSSL CLI for certificate-based examples.
- Optional: jq and Bash 3.2+ for the Bash SDK.

Build from the repository root:

```sh
cargo build --locked --release
./target/release/mynofy --help
```

The `mynofy` executable includes the server, client, key generation,
configuration tools, administration, MCP adapter, and storage tools. There is no
separate broker installation or required database.

## 1. Create a local configuration

```sh
./scripts/setup-unix.sh
```

This creates `.local/unix/` with a client private key, its public registration,
and `config.yaml`. The local topic is `local/events`, owned by service `local`.
The setup grants this client publish, subscribe, and inspect-own permissions.
Use newly generated credentials; the public keys in source configuration samples
are test placeholders, not ready-to-use accounts.

Setup helpers refuse existing output directories. Reuse the configuration rather
than rerunning setup. Keep the directory private and never commit its contents.

## 2. Start the service

```sh
./target/release/mynofy check-config --config .local/unix/config.yaml
./target/release/mynofy serve --config .local/unix/config.yaml
```

Keep it running. Relative service paths resolve against the configuration file;
relative client paths resolve against your current directory. The sample content
is RAM-only. The security journal is separate and persists registrations and ACLs.
A local socket expects service and clients to use the same OS account.

## 3. Publish and poll

In another terminal, start a client:

```sh
./target/release/mynofy client \
  --socket .local/unix/run/mynofy.sock \
  --key .local/unix/client.pk8 --owner local
```

Paste these requests, one at a time:

```json
{"version":1,"type":"get_permissions","id":"rights-1","topic":"local/events"}
{"version":1,"type":"publish","id":"send-1","topic":"local/events","payload":{"text":"Build complete"}}
{"version":1,"type":"poll","id":"read-1","topic":"local/events","limit":20}
```

To observe from another terminal, start another client with the same command and
poll the topic. These two sessions share one identity; use separate registered
keys when you need independent identities or permissions.

Poll entries contain `message` with an owner-stamped `producer_id`, distinct from anything inside the payload. The page includes `next_cursor` and `has_more`. The broker keeps no reader state or ACK/NACK records. Application acceptance and completion are separate published messages.

Save `next_cursor` only after
processing or durably recording the page. If `has_more` is true, fetch the next
page immediately; otherwise wait your chosen interval. Use the cursor as `after`
on the next poll. No cursor means reading the current active messages again.
After a known service restart, reconcile from the active set and your local work
state rather than assuming a saved cursor proves completion.

## 4. Use a shell API instead of writing JSON

```bash
source ./sdks/bash/mynofy.sh
MYNOFY_BIN="$PWD/target/release/mynofy"
MYNOFY_OWNER=local
MYNOFY_KEY="$PWD/.local/unix/client.pk8"
MYNOFY_SOCKET="$PWD/.local/unix/run/mynofy.sock"

mynofy_publish_text local/events 'Ready for review'
mynofy_poll local/events '' 20
```

Variables do not have to be exported. Bash wraps the Rust CLI; the other language
SDKs offer native clients. None performs your application work or decides whether
an external operation is safe to repeat.

## 5. Use WSS on a network

For a separate local certificate demo:

```sh
./scripts/setup-local.sh
./target/release/mynofy serve --config .local/demo/config.yaml
```

In another terminal:

```sh
./target/release/mynofy client \
  --url wss://localhost:8080 --ca .local/demo/ca.crt \
  --key .local/demo/client.pk8 --owner local
```

Requests are unchanged. The sample CA and certificate are for local development.
For a LAN or central deployment, provide your own trusted CA, matching certificate
names, endpoint address, registered clients, and explicit ACLs. There is no
plaintext network fallback or certificate-verification bypass.

## Next

- [Networking and proxy layers](networking.md): reach a remote owner through peers.
- [Security](security.md): credentials, authorization, storage, and privilege drop.
- [Code-review example](../examples/code-review/README.md): operator, coder, and reviewer.
- [Contributing](../CONTRIBUTING.md): checks and public pull-request workflow.

The client command reads JSON lines locally and translates them to Protobuf binary
WebSocket messages. Sending those JSON lines directly to the broker as text frames
is unsupported. The same rule applies through owner tunnels; see the
[broker client transport](broker-client-transport.md).
