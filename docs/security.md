# Security model

mynofy uses explicit client registration, owner-controlled topic permissions, and
separate trust for clients and service peers. There is no anonymous topic access
or automatic enrollment of a key simply because a client can connect.

## Client identity and authorization

A client owns an Ed25519 keypair. The service stores the public key; a bounded
challenge/proof exchange proves possession of the private key on each connection.
The private key is not sent to the service. The client also checks the expected
logical service ID, in addition to transport certificate verification.

Identity starts from the first public key and remains stable across an authorized
key rotation. Changing keys does not create new ownership or reset rate limits.
A new unregistered identity requires explicit provisioning.

The owner stamps each stored message with `producer_id`, its stable authenticated
client identity. Readers obtain that identity through the protected owner session;
they do not need the sender's current public key to attribute the message. The
producer cannot select or override this envelope field. Application actor numbers
must be checked against it. A payload may contain arbitrary names or IDs, but they
are not authenticated attribution. This is an owner assertion, not a portable
message signature that can be verified independently of the owner. Existing
messages retain their producer identity across key rotation and storage restore.
Interpret the identity within the expected owner context. `producer_id` is required
in full messages (Protobuf field 5); update service and clients together for this
changing v1 prototype. Existing compact RAM and snapshot record layouts are unchanged.

The topic owner grants exact permissions: `publish`, `subscribe`, and
`inspect_own`. There are no implicit wildcard permissions or administrative powers
from client-key possession. ACLs are checked for actual operations; a permission
query is only a snapshot. Removing publish permission prevents new publishes,
including attempts to resend earlier content.

A still-authenticated producer can retain access to its own original active
messages after topic rights are removed. That does not grant access to other
participants' messages, replies, or new publish rights. This is an intentional
ownership rule, not a promise that revoking a topic ACL erases past knowledge.

## Transport boundaries

- **Direct WSS:** TLS 1.3, trusted CA and certificate-name verification, expected
  service ID, then client proof and owner ACL. No plaintext fallback.
- **Local Unix sockets:** restrictive filesystem permissions and peer-UID checks,
  followed by the same client proof and ACL. This mode is for the same OS account;
  processes sharing that account are not isolated security principals at the OS level.
- **Service links:** neighboring services authenticate with CA-based mTLS and
  explicit peer approvals. A service certificate does not confer client rights.
- **Proxied clients:** an inner TLS session terminates at the remote owner, which
  independently authenticates the client. Each owner requires its own registration
  and ACL. Trusting an edge alone does not authorize access at the owner.

The proxy's route topic allowlist controls opening a channel, not inspecting
commands hidden within owner TLS. The final owner remains the authorization
boundary. See [Networking](networking.md).

## Administration and operating-system privileges

The admin API uses a separate protected Unix socket. Keep that path accessible
only to the intended local administrator. The client/MCP surface does not expose
admin operations.

Run as an unprivileged account. If a privileged Linux launcher is used, it must
supply both `--run-as-uid` and `--run-as-gid` with an explicit unprivileged target.
The executable drops elevated credentials before creating its runtime or opening
service storage/listeners. There is no `nobody` fallback. Privileged startup is
not supported on macOS.

Do not make runtime socket directories writable by other users. Startup refuses
existing files, symlinks, and stale socket paths rather than overwriting them.
After an abnormal shutdown, verify that no service owns a stale socket before
removing it deliberately.

## Keys, rotation, and limits

Clients initiate bounded key rotation while the old registration remains valid.
The exchange uses a shared secret, encrypted new-key material, and proof of the
new private key before commit. A revoked or fully expired registration requires
reprovisioning. Rotation is not a recovery bypass for a compromised account.

The service applies client rate limits, pre-auth source limits, connection and
channel budgets, payload limits, poll-page limits, and deadlines. These bound
resource use; they are not a guarantee against every form of denial of service.
Use your usual network exposure and host-access controls.

CA-based trust is implemented. SSH-style TOFU and SPKI pinning are not. Incoming
peer blocking and local certificate revocation are available through the admin
API; automatic certificate reload, CRL/OCSP management, and distributed revocation
are not promised. Outgoing trust configuration and certificate-file changes need
an appropriate restart.

## Storage and durability

Content is RAM-only unless a topic opts into manual or periodic snapshots.
Optional content encryption uses one startup secret with per-topic storage policy.
The whole load/decrypt phase finishes before the service accepts clients; there
is no lazy content loading. Encryption does not hide data from the running owner
or protect a compromised process that already holds the secret.

Security state is separate. Changes take effect in RAM immediately and enter an
append-only journal. By default (`security_storage.sync: true`), successful Admin
security mutations and key-rotation commits confirm disk synchronization.
Explicit `security_storage.sync: false` allows asynchronous confirmations;
unsynced changes can then be lost on a crash. Waiting does not hold the core lock. Storage errors do not roll
back RAM changes. See the [security journal guide](security-journal.md).
Content publishing retains its separate durability contract.

## Reporting vulnerabilities

Follow [SECURITY.md](../SECURITY.md). Do not include real private keys, tokens,
production configs, or customer message content in public issues or pull requests.

Operational limits and durability checks are described in the [content storage guide](content-storage.md). Security updates take effect in RAM immediately; durable success confirmations are the default; `security_storage.sync: false` explicitly opts out.

Client sessions now notify their transports when ACL/key/client changes disconnect
them. A blocked response send is cancelled and its transport is dropped without
a graceful flush that could release remaining sensitive bytes. Bytes already
handed to the operating system cannot be recalled. See the
[client authentication](client-auth.md).

Credential rotation disconnects other sessions using the replaced token. Response
sends also observe credential expiry. The CLI synchronizes the new key file and
parent directory before starting remote rotation, and propagates sync failures.
See the [key rotation guide](key-rotation.md).
