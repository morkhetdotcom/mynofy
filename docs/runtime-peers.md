# Managing service peers at runtime

The local Admin API manages incoming peers and their topic allowlists.
It can disable known neighbors and revoke individual peer certificates.
Clients still authenticate directly with the topic owner through the tunnel.

## Requirements and commands

Configure a separate `service.admin_socket`. To admit incoming peers at
runtime, configure TCP/TLS and `service.tls.peer_ca_file` at startup. The
service loads the peer CA even when `accepted_peers` is empty. A missing or
invalid configured CA prevents startup. Ordinary clients use the mynofy
keypair handshake and can connect without a TLS client certificate.

```sh
mynofy admin peers list --socket run/admin.sock

# Admit an incoming peer or replace its policy.
mynofy admin peers set edge --topics team/events,owner/events --socket run/admin.sock
# Override the default certificate SAN, which is the service ID.
mynofy admin peers set edge --identity edge.example --topics team/events --socket run/admin.sock

# These commands also apply to the configured outgoing upstream.
mynofy admin peers disable edge --socket run/admin.sock
mynofy admin peers enable edge --socket run/admin.sock

# Remove incoming admission.
mynofy admin peers delete edge --socket run/admin.sock
```

`set` replaces the entire allowlist. Validation requires distinct service
and certificate identities and a nonempty, duplicate-free list of exact
existing topics. `list` returns incoming peers in lexical order, their
disabled status, `next_after`, and the configured upstream and its disabled
status. Use `--after ID --limit 1..100` for paging.

A disabled peer stays disabled if its incoming policy is deleted and later
recreated. Use `enable` to lift the restriction. `delete` removes incoming
admission only. Repeating an identical `set`, `disable` or `enable` leaves
existing links in their current state.

## Revoking an individual certificate

```sh
mynofy admin peers revoke-certificate --certificate edge-leaf.pem --socket run/admin.sock
mynofy admin peers revoked-certificates --socket run/admin.sock
# Revoke by fingerprint when it is already known.
mynofy admin peers revoke-certificate --fingerprint 'sha256:BASE64' --socket run/admin.sock
```

The first certificate in the PEM file must be the leaf. The CLI hashes the
complete leaf DER with SHA-256. The fingerprint consists of `sha256:`
followed by canonical standard Base64 encoding of the 32-byte digest.
It identifies the certificate for local revocation. The active peers' leaf
fingerprints appear in `admin status` under
`service_links.registered_certificates`.

Local revocation applies to inbound and outbound peer links in addition
to CA and SAN verification. To replace a revoked certificate, provision a
valid certificate with a different fingerprint and restart the service
to load its TLS files. The local list is limited to 10,000 fingerprints.
Use `--after FINGERPRINT --limit 1..100` for paging.

## Effect on active connections

Changing, removing or disabling a peer invalidates the affected
registration. Certificate revocation invalidates links matching the leaf
fingerprint. The link managers receive the change and stop their tunnel and
route workers even during I/O. They free channel capacity and remove
learned routes. Other links with unchanged policies remain active. A
disable followed by an enable requires a fresh peer authentication.

The new in-memory policy applies before the Admin response. Physical
closure and worker cleanup follow asynchronously. Commands already
accepted by the owner retain their effects. Affected clients may see an
unknown outcome. A later explicit request may open a fresh authorized link.
Commands and sessions from the failed link are never replayed or resumed.

Local topics and direct client connections remain available. A peer
allowlist controls route discovery and tunnel setup. The owner enforces
the tunnel's topic scope and its own client ACL on broker operations.

## Persistence and limits

Peer allowlists, disablement and certificate revocations apply immediately
in memory and are journaled in order. A full or unavailable queue rejects a
mutation before applying it. Admin status shows the durable sequence.
With `security_storage.sync: true`, the default, successful mutation
responses confirm synchronization of the journal. Set `sync: false` to
allow asynchronous confirmation.

Restart loads the persisted policy. Admin exports include it. The startup
file stays unchanged. `disabled_peers` and `revoked_peer_certificates`
are optional configuration fields. A legacy snapshot without peer fields
uses the startup policy as its initial baseline. Older binaries cannot
read the newer journal record types.

Remove all peer grants for a topic before deleting it. Otherwise deletion
returns `topic_in_use`. During recovery, grants for unsaved dynamic RAM
topics that disappeared are removed and do not carry over to recreated
topics. Other unexpected references to missing topics prevent startup.

Configure listeners, CA and service certificates, the upstream endpoint
and remote owner routes at startup. The Admin commands apply only to
local peer policy. Topics are provisioned explicitly.

## Verification

Unit tests cover policy validation, revisions, idempotence, atomic queue
admission, export and journal or snapshot recovery, including legacy
snapshots and missing dynamic RAM topics. CLI and mTLS tests cover runtime
peer admission from an empty policy, changes, disable and enable, deletion,
tunnel teardown, local topic availability and bidirectional certificate
revocation. The link suite also covers multi-hop routing, owner
authentication and ACLs, resource limits and rotation.

```sh
cargo test --locked -p mynofy-server runtime_peer
cargo test --locked -p mynofy-cli --test links
```
