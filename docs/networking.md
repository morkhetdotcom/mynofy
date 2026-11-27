# Networking and proxy layers

A mynofy service is an owner, a proxy, or both. Ownership belongs to a topic, not to a permanent service role. One instance can handle `laptop/events` locally and route access to `team/events` at another owner.

## Where you can run it

| Scope | Connection | Trust boundary |
| --- | --- | --- |
| Processes on one machine | Unix socket on Linux/macOS | Private filesystem path, peer UID, client keypair, and owner ACL |
| Machines on a LAN | Direct WSS | TLS CA/name verification, expected service ID, client keypair, owner ACL |
| A central service | Direct WSS | Same checks as LAN; expose only the endpoints you intend to serve |
| Separate service environments | Service mTLS links and tunneled owner TLS | Neighboring service identity and independent end-to-owner client authentication |

These are application transports. The proxy is not an IP router, a transparent proxy for arbitrary traffic, or a replicated message store.

## Four layers, four different responsibilities

```text
Client ── Unix socket or WSS ── Edge ── mTLS/WSS ── Hub ── mTLS/WSS ── Owner
       ╰──────────────────── inner owner TLS ─────────────────────╯
```

1. **Client ingress.** The client connects to a local Unix socket or a WSS endpoint. Direct connections use the API at that service. A proxied connection requests a route to the configured owner using a topic.
2. **Service links.** Neighboring services authenticate with CA-based mTLS. Each service has at most one configured upstream and can accept explicitly approved incoming peers. Multiple logical channels share a peer WebSocket.
3. **Owner channel.** Inside the forwarded channel, the client establishes a separate TLS connection that terminates at the final owner. Proxies forward encrypted bytes. The owner authenticates the client's Ed25519 proof itself; peer identity is never a substitute for client registration.
4. **Application protocol.** Publish, poll, permissions, producer queries, and client-initiated key rotation run on the authenticated owner session. Agent task IDs and workflow meanings sit above this protocol.

The ingress CA authenticates the entry service. The owner CA and certificate name authenticate the remote endpoint. The expected logical owner ID is checked separately. These trust anchors may be different.

## Ownership and routing

Local topics resolve locally. A remote topic entry names its owner and next hop:

```yaml
upstream:
  service_id: hub
  url: wss://127.0.0.1:8091/
  trust:
    mode: ca
    ca_file: ca.crt
    expected_certificate_identity: hub
topics:
  - name: team/events
    owner_id: owner
    via: hub
```

This is a configuration excerpt, not a complete runnable configuration. The next service must explicitly accept the requesting peer for the routed topic, and the chain must lead to the configured owner. A remote topic has no local policy or ACL: the owner holds those. A registration at an edge service does not register the client at the owner.

Routes carry an expiry and a bounded hop count. Cycles, owner conflicts, invalid peer identities, and expired routes are rejected. The configured hop limit is 1–16. The service never takes over ownership when an upstream disappears.

**Important:** a peer's allowed topic list controls route discovery and channel opening. Once a client has an encrypted session to an owner, that owner's ACLs control all commands inside it. The proxy cannot inspect hidden commands or act as a second per-topic authorization layer for that session.

## Run a three-service chain

From the repository root, with Rust, Python 3, and OpenSSL CLI:

```sh
./scripts/setup-links.sh
```

This creates a development CA, service certificates, a client key, and three YAML configurations in `.local/links/`. It does not start services or overwrite an existing setup. Start these in separate terminals:

```sh
./target/release/mynofy serve --config .local/links/owner.yaml
./target/release/mynofy serve --config .local/links/hub.yaml
./target/release/mynofy serve --config .local/links/edge.yaml
```

Connect through edge and hub to owner:

```sh
./target/release/mynofy client \
  --url wss://127.0.0.1:8090 --ca .local/links/ca.crt \
  --key .local/links/client.pk8 --owner owner \
  --via-topic team/events --owner-ca .local/links/ca.crt
```

Then publish or poll:

```json
{"version":1,"type":"publish","id":"send-1","topic":"team/events","payload":{"text":"Review requested"}}
{"version":1,"type":"poll","id":"read-1","topic":"team/events","limit":20}
```

The owner certificate name defaults to `--owner`; use `--owner-name DNS_OR_IP` when it differs. A Unix-socket entry accepts the same `--via-topic`, `--owner-ca`, and `--owner-name` options. This supports local agents reaching a remote owner without giving them a direct network connection to it.

## Runtime and failure behavior

A remote access lazily establishes the shared upstream link. An unavailable upstream does not prevent local service startup or local-topic access. Incoming peer approvals, blocking, and local certificate revocation can be administered at runtime. Listener addresses, CA/certificate files, upstream selection, and static remote routing remain startup configuration.

Links use bounded channels, credit-based flow control, response limits, and connection/request deadlines. A broken peer link closes its dependent channels. A later client request can establish a new connection, but an in-flight command is not replayed or resumed.

A timeout is not proof that a publish did not happen. There is no automatic retry, store-and-forward, response retrieval after disconnect, broadcast mesh discovery, replica election, or failover owner. CA-based trust is implemented; SSH-style TOFU and SPKI pinning are not available.

For lower-level frame and configuration details, the source schemas under `protocol/v1/` are authoritative. Security boundaries are summarized in [Security](security.md).
