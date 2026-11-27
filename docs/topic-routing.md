# Topic owners, permissions and routing

**The topic owner holds the ACL and authorizes each topic operation.** Intermediate services route the channel to the configured owner. See the [peer handshake](service-links.md) and [service networking guide](service-network.md).

## Inspecting your permissions

An authenticated client can send `get_permissions` with `id` and an exact `topic`. The `permissions` response includes `request_id`, `topic`, `owner_id`, `acl_revision`, and an `actions` list drawn from `publish`, `subscribe`, and `inspect_own`. The request uses the client identity established by authentication.

The actions list may be empty. Producers retain access to their own original active content under the producer-inspection rules. The response describes current permissions at the owner. The owner checks each publish, poll or inspection request under its current ACL and ownership rules. The ACL revision helps clients detect stale information. A failed route or an unreachable owner leaves the request unauthorized by the proxy.

## Service-to-service route discovery

Mutually authenticated services exchange these routing frames.

| Frame | Direction | Fields in addition to `version` and `type` |
| --- | --- | --- |
| `route_query` | Service → peer | `id`, `topic`, `visited`, `hop_limit` |
| `route_info` | Peer → service | `request_id`, `topic`, `owner_id`, `path`, `expires_at` |

The last entry in `visited` must match the authenticated neighbor that sent the request. A forwarding hop decrements `hop_limit`. A limit of 1 allows a local owner to answer and disallows forwarding. A local owner returns a path containing only itself. Other services use their one configured upstream and prepend their own service ID to its reply. A cache entry uses an expiration no later than the one received.

For example, a workstation can reach `team/events` through an office service. It learns owner `central`, next hop `office`, and path `[office, central]`. It sends traffic through `office` and authenticates with `central`.

The first path entry must be the authenticated peer that replied, and the last entry must match the configured owner. The service rejects repeated nodes and paths back to itself. Every hop must permit that exact topic. A conflicting owner is a route error. The configured owner remains authoritative.

## In-memory route table

| Field | Purpose |
| --- | --- |
| `topic` | Exact routing key. |
| `owner_id` | Service responsible for the topic and its ACL. |
| `next_hop` | Directly reachable authenticated neighbor. |
| `path` | Learned path used for consistency and loop checks. |
| `learned_from` | Peer that supplied the route. |
| `expires_at` | End of the learned route's validity. |

A service resolves its own topics locally. A cached route can help open a new channel for the same topic. Each hop checks its current policy at channel setup. Expired routes and routes learned through a disconnected peer are discarded. Restart begins with the static configuration. Local ownership stays fixed if a remote owner becomes unreachable. [Configuration](configuration.md)

A client can cache a permission response with the topic, client ID, owner and ACL revision. Different clients using the same route can have different rights.

## Client authentication at the topic owner

```text
Client ── tunneled channel via A, B, … ── Topic owner
          authentication challenge and proof end here
```

1. The routing table identifies the path to the configured owner. Each intermediary opens or forwards its part of the channel.
2. The client verifies the expected owner identity over TLS within the tunnel. This TLS session is separate from TLS between service peers. [Channel framing](service-network.md#drahtformat-und-grenzen)
3. The owner issues a challenge and verifies the proof with the client's registered public key. Intermediaries relay the exchange. [Client keypair handshake](client-auth.md)
4. After successful authentication, the owner binds the session to the stable client ID. Topic operations, producer inspection and key rotation use that session. The shared peer link can carry several independent sessions.
5. The owner binds each tunnel to one topic. Publish, poll and `get_permissions` are scoped to it. Producer lists use the same scope even without an explicit topic filter. Single-message lookups outside the topic return `not_found`. The owner's current client ACL and producer ownership rules apply within the tunnel.

Register each client public key and its stable identity with every owner it needs to reach. OS access, peer grants and credentials registered at an intermediate service do not grant data access at the owner. Inner TLS protects client commands and replies. Owner rate limits use the authenticated client ID. Intermediaries apply their own channel and peer resource limits. Poll cursors, expiration and producer attribution keep their owner-defined meanings along the route.

Configure remote routes at startup. Admin can manage local peer admission, topic grants and individual certificate revocations at runtime. Provision topic names explicitly on the services that need them. [Runtime peer management](runtime-peers.md)

## Proxy behavior and per-owner credentials

A connection timeout ends its channel and owner session. Commands and responses in flight may have an unknown outcome. For example, a publish may have been accepted before its response was lost. The proxy never automatically retries a command, resumes a disconnected session or retrieves a lost response.

A client can explicitly open a new connection and authenticate again. It may then poll with a still-valid owner cursor or intentionally submit a new publish. The application decides how to handle duplicate outcomes.

Client registration, key expiry, revocation, rotation and ACLs are scoped to each owner. Store credentials by owner so rotating a key for one owner preserves the key still used by another.

Admin status exposes the learned owner's ID, next hop, route source, path and `expires_at_ms` under `service_links`. The network guide documents link deadlines, framing and admission limits. [Service networking](service-network.md)
