# Client authentication and access control

Clients generate an Ed25519 keypair and register the public key with the
owner. Each connection proves possession of the private key. The owner
then applies its ACL to operations on exact topic names. The token ID
identifies the registered key.

## Keys and stable identity

- Ed25519 public keys are 32 bytes and signatures are 64 bytes. Keep private
  keys in the client credential store. [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032.html)
- A `token_id` is `ed25519:` followed by padded standard Base64 of the
  SHA-256 digest of the raw public key. An owner computes it at registration.
- The initial `client_id` is `client:` followed by the first key's token ID.
  An authorized rotation keeps that stable identity and changes the key's
  token ID. An independently registered keypair starts a new identity.
  [Key rotation](key-rotation.md)
- Register credentials at every owner the client needs to reach. Intermediate
  services forward the tunnel and the owner runs the client handshake itself.
  [Owner authentication](topic-routing.md)

## Authenticating a connection

Direct WSS verifies the server certificate and expected service identity.
The local Unix transport uses a protected socket with mode `0600` and
peer-UID verification. The same client keypair proof and ACLs apply to
both. In a tunnel, the client authenticates the final owner over inner TLS.
[Operating guide](getting-started.md) · [Service networking](service-network.md)

The [authentication schema](../protocol/v1/client-auth.schema.json) and
[fixtures](../protocol/v1/client-auth-examples.json) describe the readable
form. The broker transports application frames as binary Protobuf.
[Client transport](broker-client-transport.md)

| Frame | Direction | Required fields besides `version` and `type` |
| --- | --- | --- |
| `auth_hello` | Client → owner | `id`, `token_id` |
| `auth_challenge` | Owner → client | `request_id`, `service_id`, `nonce` |
| `auth_proof` | Client → owner | `id`, `auth_id`, `signature` |
| `auth_welcome` | Owner → client | `request_id`, `client_id`, `service_id`, `restart_counter` |
| `error` | Owner → client | `code`, `detail`. The `request_id` is optional. |

1. Open the configured transport and send `auth_hello`. The owner looks
   up the registered key using its token ID.
2. The owner issues a random 32-byte nonce, binds it to this connection
   and hello, and sets a deadline. The challenge's `request_id` identifies
   the hello. The nonce uses padded standard Base64.
3. The client checks the correlation ID and expected service ID. It signs
   the bytes below. The proof carries a new `id` and the hello's ID in
   `auth_id`.
4. The owner verifies the deadline, state, current key authorization and
   signature. A proof attempt consumes the pending challenge.
5. The owner binds a successful connection to the stable client ID. The
   welcome's `request_id` identifies the proof. The client checks it and
   the returned identities before sending application commands. The
   authenticated WebSocket is the session.

### Signed bytes

The client signs this concatenation with ordinary Ed25519.

```text
ASCII("mynofy-client-auth-v1") || 0x00
|| nonce_raw_32_bytes
|| public_key_raw_32_bytes
|| u32_big_endian(length(service_id_utf8)) || service_id_utf8
```

The service ID is at most 256 UTF-8 bytes. The client constructs this
message from the verified fields and encodes the signature using padded
standard Base64. The challenge is specific to the current connection.
The client obtains the expected service ID from its connection configuration.

### Timeouts and errors

`limits.auth_timeout_seconds` defaults to five seconds. Unknown or
disabled credentials, incorrect signatures and invalid challenge state
can produce `unauthorized`. Invalid frames and unsupported protocol versions
have distinct error codes. An expired authentication window closes the
connection. TLS failures occur before the broker protocol starts.

Sessions share rate limits by stable client identity even across a key
rotation. Source admission limits apply before authentication. A
`rate_limited` response includes `retry_after_ms`. Reconnecting requires a
new handshake. [Configuration](configuration.md)

## Owner ACL

Permissions are explicit per client and exact topic. The owner rejects
duplicate rules and unknown actions in configuration.

| Action | Meaning |
| --- | --- |
| `publish` | Publish a new message to the topic. |
| `subscribe` | Read active messages using `poll`. |
| `inspect_own` | Compatibility action. Access to original active messages follows the authenticated producer identity. |

`get_permissions` reports the current actions. The owner checks the
actual authorization again when it handles a request. A producer can
use `get_message` and `list_messages` to read its own original active
content even after losing the topic grant. Unavailable, foreign and
expired message IDs return `not_found`. Topic filters can limit this
inspection. Reading responses from others still requires permission to
poll their topic, and every new publish requires current publish permission.
[Permission queries](topic-routing.md)

Revoking a key invalidates its active sessions. An ACL change invalidates
affected sessions and producer-list cursors. Previously delivered bytes
remain with the recipient. The owner rechecks authorization before sending
output that has not yet been released.

Use the [local Admin API](admin-api.md) to manage registrations, keys
and ACLs. A service that publishes uses a registered client identity and
key. Its service-peer certificate authorizes the link, while the owner
authenticates the client. Readers receive producer attribution through
the protected owner session. [Security model](security.md)

## Restart detection

`auth_welcome` reports the required `restart_counter` and, on newer
owners, an optional `boot_id`. The boot ID changes on startup, including
after a restore that repeats the counter. Managed clients use this
information to decide when to poll without a saved cursor.
[Restart and storage](restart-and-storage.md)
