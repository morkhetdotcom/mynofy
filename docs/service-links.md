<a id="verbindungen-zwischen-mynofy-diensten"></a>

# Connections between mynofy services

CA-based handshake, route search and limited data tunnels are implemented. [Executable example,
wire format and operating limits](service-network.md). SPKI -Pinning remains a draft. One
instance can manage local topics itself and mediate configured remote topics via another
service, so their role varies depending on the topic. For the first expansion, at most one
upstream per instance is provided; one instance can simultaneously serve several subordinate
services.

The [security model](security.md) describes open routing, identity and operational issues. An
upstream does not prevent a cycle. First of all, a static, acyclic tree is recommended without
automatic adoption of remote topics. Static owner binding, Pfad-/Hop-verification, owner-bound
confirmations and limited connection clearance are now implemented. [Live peer management and
local certificate revocation](runtime-peers.md) are implemented; CA-/TLS-Datei-Reload and
upstream address change remain outside this expansion. The handshake alone does not replace
owner authentication or routing checking. The [Topic-Routensuche](topic-routing.md) now
complements temporary tables, loop boundaries and the only decisive ACL holder at the owner; the
[Configuration](configuration.md) provides the starting scenarios.

There is just one responsible service per topic. It manages active messages, producer
assignment, delivery status and effective expiration time. If the producer does not specify
time, its topic policy determines expiration. Intermediate instances mediate topic polls, their
responses and confirmations; transport between services is not a confirmed end reception. Local
topics remain usable in the event of an upstream outage, remote topics are temporarily
unreachable. Proprietary ACLs and static topic responsibilities come from the configuration.
After the handshake, configured trusted peers can learn temporary next hop routes. This table
does not change topic ownership or ACL rights.

<a id="zwei-schritte-zur-verbindung"></a>

## Two Steps to Connecting

1. **Authenticate:** TLS verifies the identity of both counterparts and the ownership of their
   private keys.
2. **Register:** On the authenticated WebSocket connection, the services identify with
   `service_hello` and `service_welcome`. Both are the same as the indicated `service_id` with
   their local assignment to the cryptographic identity. `link_version: 2`.

The [Client-Keypair-Handshake](client-auth.md) and the client ACL are separate from each other:
a peer certificate does not grant client or producer rights; the original client authenticates
itself to the topic owner; intermediate instances mediate the channel and may not delegate a
client identity; the [Owner-session](topic-routing.md) is separate from the peer handshake.

A name or fingerprint claimed in the JSON is not proof of identity; a successful handshake only
allows the configured connection; it does not grant blanket publish, subscribe or routing
rights.

## Vertrauensmodelle

Both variants use mutual TLS certificate authentication; they differ in how certificates are
trusted. TLS remains outside the JSON protocol. TLS 1.3 describes the proof of the private key
over `CertificateVerify` in [RFC 8446, section
4.4.3](https://www.rfc-editor.org/rfc/rfc8446.html#section-4.4.3).

| Modus | Vertrauen | Einsatz |
| --- | --- | --- |
| `ca` | Both sides check certificate chain, validity, permitted use, and configured peer identity against a trusted CA. | Managed networks with their own certificate issuance. |
| `pinned_key` (geplant) | Both sides check a pre-configured SHA -256 fingerprint of the public key in the peer certificate. Self-signed certificates are possible; TLS signature verification, validity and permitted use remain required. | Small installations without a common CA. |

For `pinned_key`, the local format `SHA256:<Base64>` is provided: SHA -256 via the DER encoded
SubjectPublicKeyInfo structure of the sheet certificate, Base64 with padding. The SPKI
fingerprint construction is described in [RFC 7469, section
2.4](https://www.rfc-editor.org/rfc/rfc7469.html#section-2.4); its HTTP header protocol is not
used here. The fingerprint is calculated from the certificate actually checked in the TLS
handshake, not taken from JSON.

The configuration binds a stable `service_id` to the allowed certificate identity or the pin.
IDs must be unique within the communication network and be retained via restarts. Certificates
and private keys are local file inputs. Routing state remains in RAM; Topic-/Content- and
security storage follow their separate rules.

The implemented prototype only uses `ca`; configuration with `pinned_key` is rejected. For a
later pin mode, pins would be entered in advance on both sides. A later SSH type of device can
display the fingerprint for checking at first contact and store it locally after express
confirmation. Until then, no service connection is released. An unknown or changed key is not
automatically accepted. Key change requires a deliberate update of the assignment; another trust
mode is not automatically tried out in the event of errors.

## JSON-Handshake

The separate [Peer-Schema](../protocol/v1/service-frame.schema.json) describes the handshake
frames; they are not topical messages and apply exclusively to connections between services;
normal clients do not need this handshake.

| Type | Direction | Mandatory fields in addition to `version` and `type` |
| --- | --- | --- |
| `service_hello` | connection initiator → Gegenstelle | `id`, `service_id`, `link_version` |
| `service_welcome` | Gegenstelle → connection initiator | `request_id`, `service_id`, `link_version` |
| `error` | Gegenstelle → connection initiator | `code`, `detail`; optional `request_id` |

Example after successful TLS handshake:

```json
{"version":1,"type":"service_hello","link_version":2,"id":"hello-1","service_id":"workstation-a"}
```

```json
{"version":1,"type":"service_welcome","link_version":2,"request_id":"hello-1","service_id":"office-hub"}
```

`request_id` must match the `id` of the Hello. `version: 1` denotes the accepted protocol version; another version is rejected. There is no version negotiation, role message, automatic topic announcement or global service registration.

<a id="zustände-und-fehler"></a>

## Conditions and errors

- Before the JSON handshake, TLS must have successfully authenticated both counterparts. TLS
  errors terminate the connection setup without JSON response.
- The initiator sends exactly one hello, the counterpart checks version, structure, identity
  assignment and local release, rejecting its own service ID and an already active second
  connection of the same peer ID.
- When accepted, it sends Welcome. The initiator checks its structure, version, `request_id` and
  the binding of the peer ID to the TLS certificate. Only then is the connection on its side
  considered registered. The counterpart can send follow-up frames directly to Welcome; they are
  processed by the initiator only after successful Welcome check.
- The handshake must be completed within a configured time limit; the proposed standard is five
  seconds after WebSocket setup; only handshake frames are allowed before completion; repeat
  hello and topic frames before completion are protocol errors.
- An error leads to the end of the connection. `error` codes are sent. `invalid_frame`,
  `unsupported_version`, `identity_mismatch`, `peer_not_allowed`, `duplicate_service`,
  `handshake_required`, `handshake_timeout` and `internal_error`. Details do not contain keys or
  internal trust lists.
- Registration is valid only for the current connection. If discontinued, their active
  registration and connection-related states are removed. Trust assignments and topic
  configuration are retained. A reconnect goes through TLS and registers again; an old
  connection is deemed replaced only after its termination.

<a id="nachrichtenrouting-und-endempfang"></a>

## Message routing and final reception

A client authenticates independently at the owner over inner TLS. Channel
numbers identify transport streams. The owner retains producer attribution
and applies the tunnel's topic scope and current client permissions.
[Topic routing](topic-routing.md) · [Tunnel frames](service-network.md)

The regular service stop cancels an ongoing joint upstream setup and waits for its task and for
the upstream manager, even if the runtime of an embedded owner continues.
