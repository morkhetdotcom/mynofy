<a id="service-vernetzung-implementierter-stand"></a>

# Service networking: implemented status

An instance can have local topics and mediate remote topics via a configured upstream.
Implemented are CA-based mutual TLS authentication, registration, temporary route information,
and multiple logical client channels on a shared peer WebSocket. Two- and three-service chains
are tested. [Runtime peer management](runtime-peers.md) adds incoming peer sharing, locks, and
local certificate revocation; listeners, CA files, and upstream address remain start
configuration.

```text
Client ── WebSocket to edge ── mTLS link to hub ── mTLS link to owner
       ╰──────────────── innere TLS-session ────────────────────╯
                   Client Keypair Authentication and Messages
```

The external connections identify neighboring services. TLS - Session ends only with the owner;
in it the unchanged client keypair handshake and binary Protobuf application frames. The client requires
registration and ACL Registration on the proxy is not required for this access and does not
replace owner clearance. Client-/Producer-Rechte.

## Lokal ausprobieren

As an unprivileged user in the repository:

```sh
./scripts/setup-links.sh
```

The script creates `.local/links/` It creates a Demo-CA, three certificates with
`serverAuth,clientAuth`, eindeutigen DNS -SANs, a client key and YAML Configurations with
separate security directories. `MYNOFY_BINARY` can point to an already built binary,
`MYNOFY_LINKS_DIR` Ports: edge 8090, hub 8091, owner 8092.

Start in three terminals:

```sh
cargo run --locked -- serve --config .local/links/owner.yaml
cargo run --locked -- serve --config .local/links/hub.yaml
cargo run --locked -- serve --config .local/links/edge.yaml
```

The already applicable explicit UID-/GID-Zwang at privileged CLI start is retained.The setup
does not select a service account fallback.

Client across the entire chain to the owner:

```sh
cargo run --locked -- client \
  --url wss://127.0.0.1:8090 \
  --ca .local/links/ca.crt \
  --key .local/links/client.pk8 \
  --owner owner \
  --via-topic team/events \
  --owner-ca .local/links/ca.crt
```

For example, JSONL on stdin:

```json
{"version":1,"type":"publish","id":"p","topic":"team/events","payload":"review requested"}
{"version":1,"type":"poll","id":"q","topic":"team/events"}
{"version":1,"type":"get_permissions","id":"r","topic":"team/events"}
```

`--ca` checks the entry point. `--owner-ca` checks the owner within the tunnel; both can be different CAs. The expected Owner-DNS-/IP-Name is `--owner` by default, alternatively `--owner-name DNS_OR_IP`. The expected logical service ID remains regardless `--owner`. An incorrect owner, certificate name or trust anchor terminates the setup. Route information of the proxy is not a substitute for this check.

A local entry via `--socket PATH` supports the same `--via-topic`, `--owner-ca` and
`--owner-name` options, but the Unix configuration may also contain `service.tls` with the
outgoing peer access data if an upstream exists. The local socket remains a file
system-protected access without external TLS; the internal owner TLS session remains. `rotate`,
`adapter` and `--progress` use the same generic client channel.

<a id="konfiguration-und-identitätsbindung"></a>

## Configuration and Identity Binding

Example of an intermediary service:

```yaml
upstream:
  service_id: owner
  url: wss://127.0.0.1:8092/
  trust:
    mode: ca
    ca_file: ca.crt
    expected_certificate_identity: owner
topics:
  - name: team/events
    owner_id: owner
    via: owner
```

Your own topics can also be in the same list. Remote entries do not have a local policy or ACL.
There is only one upstream per instance.

```yaml
accepted_peers:
  - service_id: hub
    topics: [team/events]
```

The accepting service requires a TCP-/TLS-Listener with `service.tls.peer_ca_file`. It verifies
client certificate, validity and client use by rustls. Normal clients are allowed to connect to
the public data listener without TLS client certificate; `service_hello`, however, requires an
actually verified client certificate.

In addition, its SAN must match the configured peer identity. Default is
`accepted_peers[].service_id`; a different certificate name can be explicitly assigned via
`certificate_identity`. Double peer IDs or equivalent TLS names are rejected. This also applies
to DNS names with a different capitalization or final point and for alternative spellings of the
same IPv6 address. Starting from TLS `upstream.trust.expected_certificate_identity`, then the
welcome must confirm exactly the configured `service_id`. A JSON name alone does not
authenticate anything. A service certificate should contain both uses `serverAuth,clientAuth` if
the service performs both roles.

The peer CA must issue separate service keys and certificates for separate peer policies. Use a
peer-specific SAN and check before deployment that no additional configured peer name is covered
by additional SANs or wild cards. A service can use the same own key for its incoming and
outgoing links.


At startup, the upstream WSS address including host and port is checked. The first successful
remote access builds the shared upstream link; a failed upstream does not prevent local startup.
Each peer can have one active registration. A new access after the connection has ended can
set up a new link. Already started commands or client sessions are not repeated or resumed.

`ca` is the implemented trust mode. Listener, TLS material, upstream and static remote routes are loaded at startup. Local peer approval and its topic grants can be changed at runtime. Administrative revocation of a single peer certificate ends affected active links and prevents new registrations with this certificate. [Runtime peer management](runtime-peers.md)

The runtime topic management edits local topics. A new local topic is not automatically
announced on the network. [Runtime topic management](runtime-topics.md)

<a id="routing-und-owner-sitzungen"></a>

## Routing and Owner Sessions

`route_query` / `route_info` run only on registered peer connections. The last `visited` entry must correspond to the authenticated neighbor; own, duplicate or invalid nodes are rejected. Each redirection reduces the hop limit. At a limit of 1, only the local owner can answer. The configuration limits it to 1-16.

Each hop must explicitly release the topic to the requesting peer. The response must begin with
the immediate next hop and end with the configured owner. Owner conflicts, loops and expired
routes lead to failure, never local takeover. The edge can reuse a valid learned route for a new
tunnel of the same topic. Without a valid entry, it retrieves the route. A rejected channel
setup discards the entry. The releases of all hops are rechecked during channel setup.
Intermediate stations do not extend the received expiration time. A peer disconnect discards routes learned through it. Expired entries are removed. No routing persistence and no broadcast.

With `--via-topic`, the client dials the topic and the path to the owner. The tunneled session
is tied to this topic. For another topic, the client opens its own tunnel. The owner also checks
its client ACL and the producer property. Operating system access and peer sharing do not grant
data rights to the owner. Producer lists are valid for the tunnel topic even if the client does
not specify a topic. Individual queries outside the topic report the message as not found.
Catalog and key rotation remain available for logged-in clients. Multiple tunnels can use the
same peer link.

The owner reserves for pending tunnels no more than one quarter of its channel slots (at least
one) and no more than one eighth (at least one) per authenticated peer. A TCP edge additionally
limits the pending tunnels to one sixteenth of its channel slots (at least one) per source IP.
After successful broker registration, the owner confirms the channel via the peer link.
Intermediate stations pass on the confirmation up to the edge. Termination and connection end
release the recording slots. The edge validates the requested owner before the route query. The
peer handshake requires on both sides `link_version: 2`. A missing or different version is
rejected before registration. Services of a peer network are jointly updated.

Message content, expiration time, cursor and producer ownership remain tied to the owner.
Message IDs apply in the context of the owner. Professional processing reports applications via
their own messages. Owner processes authorization and producer queries as well as
client-initiated key rotation.

<a id="drahtformat-und-grenzen"></a>

## Wire format and boundaries

[Schema](../protocol/v1/service-frame.schema.json), [handshake examples](../protocol/v1/service-examples.json), [route examples](../protocol/v1/route-examples.json) and [tunnel examples](../protocol/v1/tunnel-examples.json).

- Control: strictly checked JSON text frames, version 1, maximum 16 KiB; `service_hello`,
  `service_welcome`, `route_query`, `route_info`, `tunnel_open`, `tunnel_ready`,
  `tunnel_authenticated`, `tunnel_close`, `tunnel_credit` and correlated errors.
- Per peer data frame: WebSocket binary with four bytes of LE channel number, then 1–8192 opaque
  TLS bytes. Channel numbers are not reused within a connection. The inner message content
  remains protobuf or JSON as with direct owner access.
- Entry to the client: `client_tunnel` / `client_tunnel_ready`, then binary TLS bytes in blocks
  up to 8192 bytes without a peer channel prefix.
- `limits.max_service_channels`: 64 by default, allowed 1–4096, effectively maximum
  `max_connections`. The global budget per instance counts incoming and outgoing channels, even
  until their workers are completely finished.
- Four credits per direction and channel, a maximum of four buffered data frames and a limited
  duplex buffer. A credit is returned only after it has been taken over by the local stream. A
  blocked reader stops its own channel; it does not block the reading of other channels. Wrong
  credits and reused channel numbers are log errors.
- Limited event queue and a maximum of 32 pending or parallel route queries per link.
  TCP-/TLS-/WS-setup, Writes, queries and stream transfers use deadlines; Auth and Idle
  timeouts remain active.

With Timeout/Abbruch there is no automatic command repetition, no store-and-forward and no
subsequent retrieval of lost replies. A publish can have arrived at the owner even if the caller
no longer receives a reply. The client decides on a new connection setup and repetitions.
Netzwerk-I/O does not hold an engine lock.

<a id="diagnose-und-geprüfter-umfang"></a>

## Diagnosis and scope tested

`admin status` shows the configured upstream ID under `service_links` and
`upstream_connecting`, `upstream_connected` and `upstream_last_error`.
The last error contains only phase and UTC time in Unix milliseconds. After a failed setup, the
Edge throttles new connection attempts for 250 ms.


```sh
cargo run --locked -- admin status --socket .local/links/run/edge-admin.sock
```

`service_links` displays registered peer IDs, active channel resources, and learned routes with Owner, Next Hop, Source, Path, and `expires_at_ms`. It does not contain payloads or keys. Failed registration or Owner TLS check may appear as a disconnect; there is no automatic easing of check.

Unit tests check strict control frames, documented examples, Hop-/Schleifenregeln and binding to
configured owners. Genuine integration tests check zwei/drei services, parallel channels, large
poll pages, slow readers, channel budget and sharing by Disconnect,
Owner-ACL/Client-registration, topic binding of tunneled sessions, Zertifikats-/SAN-/CA-errors,
double peer registration, cyclical routes, key changes and a CLI client via Unix entry. In
addition, the generated three-services YAML example with three real CLI processes, separate
security journals and explicit unprivileged UID was executed locally.

Existing functional tests and memory termination tests remain part of `cargo test --locked
--workspace`. Native macOS runtime testing and network performance measurements were not
performed in this step.


Tunnel setup shares an in-flight connection attempt per upstream. Each request uses one deadline through route lookup, channel opening and readiness. Disconnecting the requesting socket releases its pending admission early. The established link ends at the remote peer certificate's expiry, including any attached tunnels. The next request needs a fresh TLS handshake.
