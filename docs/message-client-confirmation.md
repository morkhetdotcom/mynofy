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

Release v0.1.0 is the first supported baseline: protocol version 1 and C ABI 2.
Earlier demos and development formats are not supported release versions.
Retired Protobuf tags remain reserved. Future incompatible changes require an
explicit version and upgrade plan; see [the protocol guide](protobuf.md).

Legacy checkpoint import remains an implementation convenience and does not
establish a supported pre-release compatibility contract.
