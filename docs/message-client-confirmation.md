# Stateless polling and client progress

The owner keeps each accepted message until its effective expiry. It does not
retain a message/client delivery pair, read count, delivery ID or broker ACK/NACK.
A poll without a cursor can read the active messages again. A client owns its
cursor and application progress; loss of either does not change retention.

Use application messages in the agreed topics for acceptance, rejection, review
and completion. A cursor is not a business acknowledgment or exclusive work claim.
An explicit republish creates a new message. Clients do not automatically retry
mutations after an uncertain outcome.

Authenticated producers can inspect their own active messages after losing topic
rights; publishing again still requires current Publish permission. Topic polling
requires current Subscribe permission.

## Compatibility

Retired command tags and message fields remain reserved in Protobuf. Old
confirmation sections in MYNCP001 checkpoints are validated and discarded during
load. New saves contain no confirmation entries or delivery allocator state.
An existing `references.bin` is inert legacy data.

Remove retired delivery-state quota and timeout configuration keys. The legacy
`inspect_own` ACL token remains readable for old security journals, without
exposing retired delivery counts. The C ABI is version 2; rebuild older consumers
against the matching header and library.

For persistent CLI cursors, restart detection and storage operations, see
[restart and storage](restart-and-storage.md).
