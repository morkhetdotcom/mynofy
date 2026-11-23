# mynofy

**A lightweight communication layer for people, software, devices, and AI agents.**

Coordinating independent tools usually means custom integrations, scattered status
updates, and no shared communication channel. mynofy provides one lightweight,
self-hosted message and notification service for people, software, devices, and AI
agents. Publish short messages to topics and let participants poll them when they
are ready, on one machine or across networks. Written in Rust, it keeps the core
small and uses a compact Protobuf protocol.

**Compact status messages.** Typed catalog events express status without repeating
field names or status strings. A complete status publish is **37 bytes**, compared
with **176 bytes** in its JSON representation:

| Status content | Protobuf message | JSON representation | Size reduction |
| --- | ---: | ---: | ---: |
| `(JOB, START, PENDING)` | **37 bytes** | 176 bytes | **79.0%** |
| Same event + U64(4711), U32(1), Bool(true) | **50 bytes** | 212 bytes | **76.4%** |

Application-message sizes; transport headers excluded.

**Measured core performance and memory:**

| Measurement | Recorded result | Workload |
| --- | ---: | --- |
| Publish | **0.92 million messages/s** | 100,000 messages, 256-byte payloads |
| Batched poll | **5.77 million message reads/s** | 100,000 messages, 256-byte payloads, eight readers |
| Retained message heap | **107.9 bytes/event** | 10,000 parameterless events, including indexes |
| Process RSS growth | **2.07 MiB** | 20,000 parameterless events after two traversals by eight readers |

Reference measurements: Linux x86-64, AMD Ryzen 5 5500U, Rust 1.99.1;
15 November 2026. Median of seven optimized core runs, excluding network/TLS and
disk I/O. Heap and RSS values are incremental, not total process memory. Results
predate later security fixes and are workload-dependent.

[Get started](docs/getting-started.md) · [Networking](docs/networking.md) ·
[Security](docs/security.md) · [Examples](examples/code-review/README.md) ·
[Contribute](CONTRIBUTING.md)

## Networking

A service can own topics, proxy connections to other topic owners, or do both.
Connect locally, across a LAN, or through publicly reachable services.

### Local

Connect processes on one machine through a local service. Unix sockets on Linux
and macOS provide local access without setting up network certificates.

```text
Your machine
`-- mynofy: local topic owner
    |-- Local topics
    |-- CLI / shell tools      [Unix socket]
    |-- Applications / devices [Unix socket]
    `-- AI agents              [Unix socket, via CLI or MCP]
```

### Local + LAN

Keep local topics on each machine and reach shared topics through a LAN service.
A local instance can proxy remote connections; clients can also connect directly
to the LAN service over WSS (WebSocket over TLS).

```text
Your machine
`-- mynofy: local owner + proxy
    |-- Local topics
    |-- Local clients                  [Unix socket]
    `-- LAN machine                    [mTLS/WSS service link]
        `-- mynofy: LAN topic owner
            |-- Shared LAN topics
            `-- Other LAN clients      [WSS]
```

### Local + LAN + Public

Extend the same setup through a LAN gateway to a publicly reachable service.
Each service can own its own topics while forwarding connections to other owners.
Public reachability still requires authentication and explicit topic permissions.

```text
Your machine
`-- mynofy: local owner + proxy
    |-- Local topics
    |-- Local clients                      [Unix socket]
    `-- LAN machine                        [mTLS/WSS service link]
        `-- mynofy: LAN owner + gateway
            |-- Shared LAN topics
            |-- LAN clients                [WSS]
            `-- Public server              [mTLS/WSS service link]
                `-- mynofy: remote topic owner
                    |-- Remote topics
                    `-- Remote clients     [WSS]
```

Each topic owner enforces its own access permissions. Service links use mutual
TLS; proxied clients authenticate at the final owner. Local topics remain usable
when an upstream is unavailable.

[Transport layers, configuration, limits, and a three-service setup →](docs/networking.md)

## What it does

| Capability | What it means for your application |
| --- | --- |
| Topic-based publish/poll | Connect participants without embedding each other's APIs. Clients control when they read. |
| Small, typed messages | Use catalog events for compact machine-readable status, or arbitrary byte payloads up to 4,096 bytes. |
| Local and remote operation | Use Unix sockets locally, WSS over a network, and service proxies to reach a remote topic owner. |
| Per-topic storage | Keep content in RAM by default; opt into manual or periodic saves and optional encryption. |
| Multiple clients | Rust, Python, Go, Java, Bash, and C SDKs; a CLI, an stdio MCP server, and a rule-based executable adapter. |

Your applications and agents define the workflow; mynofy provides communication.

## Run it locally

Use a normal, unprivileged account on Linux or macOS. Install Rust **1.88 or
newer**, a C compiler, CMake, and Python 3 for the setup script.

```sh
git clone https://github.com/morkhetdotcom/mynofy.git
cd mynofy
cargo build --locked --release
./scripts/setup-unix.sh
./target/release/mynofy serve --config .local/unix/config.yaml
```

The setup creates local credentials, configuration, and the topic `local/events`.

In a second terminal:

```sh
./target/release/mynofy client \
  --socket .local/unix/run/mynofy.sock \
  --key .local/unix/client.pk8 \
  --owner local
```

Enter one JSON request per line:

```json
{"version":1,"type":"publish","id":"send-1","topic":"local/events","payload":{"text":"Device online"}}
{"version":1,"type":"poll","id":"read-1","topic":"local/events","limit":20}
```

The CLI accepts JSON and communicates with the service over Protobuf. Publish
returns a message ID; poll returns messages and a cursor for the next request.

[Full quick start, WSS, and the Bash client →](docs/getting-started.md)

## Security

| Implemented feature | Behavior |
| --- | --- |
| Early privilege drop | On Linux, the CLI drops elevated privileges before starting the runtime or opening service files and listeners. Root requires an explicit unprivileged UID/GID. macOS requires an unprivileged start. |
| Privilege escalation prevention | On Linux, active capability sets are cleared and `no_new_privs` is set before normal execution. Programs launched by the executable adapter inherit these restrictions. |
| Client authentication | Each client proves possession of its registered Ed25519 private key through a connection-bound challenge. There is no automatic enrollment. |
| Topic authorization | The topic owner checks exact-topic ACLs for each operation. Proxy services cannot grant access to another owner's topics. |
| Producer attribution | The owner stamps each message with the authenticated `producer_id`. Readers can identify the authenticated producer. |
| Session invalidation | Credential expiry, revocation, rotation, and affected ACL changes invalidate affected sessions. |
| Local endpoint protection | Unix sockets use protected paths and peer-UID checks. Administration uses a separate Unix socket. |
| Peer and owner verification | Service links require approved, certificate-authenticated peers. Proxied clients verify and authenticate independently at the final owner. |
| Resource limits | Authentication attempts, commands, connections, channels, frames, and output have configured limits or deadlines. |
| Optional content encryption | Encrypt per-topic snapshots using a shared startup secret. |

Cryptography and TLS use established libraries. Security state is stored separately
from message content.

[Trust boundaries, persistence, key rotation, and disclosure policy →](docs/security.md)

## Example: coder and reviewer

In the [code-review example](examples/code-review/README.md), a human authorizes a
small task, an operator coordinates it, Codex writes a function, and Claude reviews
it. The agents first agree on a protocol revision, actor IDs, ticket references,
and event meanings. They then report routine state with typed catalog events;
free text carries the original instruction, protocol agreement, and useful detail.

Includes English role prompts, runnable commands, and a scripted protocol demo
that requires no AI accounts.

## Guarantees that matter

- A successful publish means the owner accepted the message into RAM. It does not
  promise a disk commit, delivery to every participant, or completed work.
- An interrupted publish can have an unknown outcome. Request IDs are correlation
  values, not idempotency keys. Reconcile before retrying external effects.
- Message expiry is time-based. The owner supplies the topic default when the
  producer does not specify a time. Expired content is not an audit log.
- Content is loaded completely at startup. Security changes apply in RAM
  immediately and are journaled asynchronously; unsynced changes can be lost on
  a crash. There is no exactly-once execution, replication, or offline forwarding.

REST, MQTT/Kafka/SQS connectors, and custom runtime catalogs are future options,
not current features.

## Development and contributions

Public pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) for the
review and integration process, and [SECURITY.md](SECURITY.md) for security reports.
The public repository contains released source snapshots without CI workflows;
maintainers develop and validate changes in `mynofy-private`.

**All CI and release jobs run exclusively on self-hosted runners.** Validated
source snapshots are published automatically from private releases.

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

[Documentation](docs/index.md) · [Source protocol](protocol/protobuf/mynofy.proto) ·
[Event catalog](protocol/catalogs/mynofy-v1.json)

## License

Licensed under the [MIT License](LICENSE). Third-party dependencies retain their
own licenses.
