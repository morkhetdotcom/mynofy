<a id="kontrollierter-schlüsselwechsel"></a>

# Controlled key change

Draft according to the project decision of November 11, 2026.The client-side launched process is
implemented in the local prototype; the specific [Frames and cryptographic bonds](protocol.md)
is documented.Only the client initiates the change; server-side offerers are not provided. This
is a change of the client token at the application level, no TLS renegotiation and no TLS
traffic key update.

<a id="identität-und-gültigkeit"></a>

## Identity and validity

The stable client ID is derived from the first public key at the first setup: `client:` followed
by `token_id`. It remains the same for all subsequent authorized key changes. The current token
fingerprint, on the other hand, identifies exactly one current key. Display names such as
`codex` are not a property identifier and cannot take over an old identity by re-creation.

Registration and change are per owner. Change to an owner does not update other owners. The
client must manage the associated current private keys per owner. Global synchronization is not
foreseen. The conceptual rules of change are specified; the concrete frames and cryptographic
parameters mentioned below are implementation tasks.

The service keeps the assignment of stable identity, current key, validity period and lock
status in the RAM. At the start of the process, the baseline is loaded from the
[configuration file](configuration.md) and the current [security state](security-journal.md) is
restored. Runtime change and locks are automatically stored asynchronously. The client requires
corresponding validity metadata. A raw Ed25519 public key itself contains neither expiration
date nor revocation information; these are from the registration. A TLS certificate has its own
separate life cycle.

A change is only allowed as long as the existing token is still valid and not blocked and the
connection has thus been authenticated. If the token is blocked or has already expired, it must
be re-established via a trustworthy administrative channel. A certificate that has expired or is
revoked and is required for the connection must also not be circumvented by this expiration. A
manual reconnection to an old stable identity is an explicit operator decision, not a right from
the new public key.

<a id="vorgesehener-ablauf-mit-shared-secret"></a>

## Scheduled Expiration with Shared Secret

1. The client recognizes the upcoming expiration date and asks on the existing authenticated WSS
   connection if the key change should begin.
2. The opposite office confirms that only one change per identity must be pending at the same
   time. Both sides know a unique change ID and a short period of time.
3. The initiator creates a cryptographic random, one-time shared secret and transmits it
   exclusively through this authenticated, encrypted connection, which does not appear in URLs,
   logs, topic messages or general status queries.
4. The client creates a new key pair. It first keeps the old key locally and encrypts the new
   public key with authenticated encryption under the shared secret. Stable client ID, service
   ID, change ID and old token fingerprint must be tied for exchange.
5. The service decrypts and checks the public key. Before taking over, the ownership of the new
   private key must also be proven, for example with a new signed challenge. The encryption of
   the public key alone does not prove this ownership.
6. Immediately before committing, the service checks the validity and blocking status of the old
   token and the unchanged identity assignment. The new key is atomically assigned to the same
   stable client ID; the old key is replaced. Property and existing ACL remain in place. The
   change does not extend rights.
7. The service confirms the commit. The client checks the new login and discards the old key
   only after the success has been clearly established. Shared Secret and pending change state
   are discarded after success, cancellation or deadline.

Withdrawal of rights, token revocation and expiration take precedence over a change that has not
yet been completed. An initiated change may not itself extend the validity. Pending normal Auth
Challenges involving a replaced or revoked key become invalid; acceptance of the proof and
binding of the session must be atomically verified against the current configuration.

<a id="fristende-und-neustart-des-vorgangs"></a>

## End date and restart of the process

Each task has a fixed deadline from the configuration. If it is not completed by then, Shared
Secret, reserved new key and pending challenges of this task expires. A new attempt starts
completely with new change ID and new Shared Secret. An old task does not continue after
deadline. There is a rate limit for each stable client ID for started change, shared across all
connections; a timeout does not reset this budget.

Before the atomic commit, the previous key remains relevant. After a successful commit, the new
key is relevant even if the response was lost. A deadline does not roll back a previously
accepted key. The client therefore retains the new private key and can attempt to log on. If an
incomplete expiry fails, it restarts with the still valid accepted key; if none is valid or
exists, a new setup is necessary. No automatic reversion to a revoked or expired key is allowed.

On restart, the service restores the synchronized security journal. With `security_storage.sync: true`, `rotation_complete` confirms that the new key was synchronized. Durable confirmation is the default. Explicit `security_storage.sync: false` can lose unsynced changes on a crash. A lost commit response or failed synchronization leaves the outcome unknown; retain both keys and reconcile without automatically replaying the commit.

Withdrawal of rights, revocation and expiration take precedence over a still open commit. The
check and the commit are serialized against the current configuration generation. A limit does
not protect against every distributed attack; additional Listener-/Quellenlimits already apply
to successful authentication.

<a id="umsetzung-des-lokalen-prototyps"></a>

## Implementation of the local prototype

The client initiates `rotation_start`; the confirming offer contains deadline and new owner
validity. only after that the client creates the shared secret and sends it with
`rotation_confirm` The service responds with nonce and proof challenge without sending back the
secret. `rotation_commit` transportiert den AES -256- GCM The replacement is an atomic core step
without any atomic key. Netzwerk-I/O within the commit.

Shared secrets are deleted in the server state by `zeroize` when discarded. The CLI client keeps
old and new private files separate; a new key does not overwrite an existing path. The current
local prototype does not support a rotation initiated manually on the client side. A server-side
trigger is not provided. Automatic expiration warnings and a permanent change state are not
implemented. An independently re-provisioned identity is derived from its first public key and
does not take over any existing ACLs or contents. Regular client-initiated rotation, on the
other hand, receives the stable client ID.

A public key is not a secret. The additional shared secret can tie the change to an agreed
process, but does not protect a key exchange without the already authenticated TLS channel. The
permission to change comes from the valid old identity and the confirmation, not just from the
knowledge of a secret brought by the client itself.
