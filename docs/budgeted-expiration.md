# Expiration and bounded cleanup

A message becomes logically expired at its accepted expiration time. Polling and
own-message inspection must not return it after that point, even if its physical
storage has not yet been reclaimed. Reading never extends retention.

The owner advances cleanup in bounded chunks during requests and background
maintenance. Records, indexes, payload storage and expired rotation transactions
are reclaimed without draining an unbounded backlog under one request's lock.
A requesting client can replace its own expired rotation even when global cleanup
has work remaining; its rotation attempt limit still applies.

Cleanup budgets bound work, not elapsed time. Tree operations, memory allocation
and operating-system scheduling have their own costs. Snapshot capture and
maintenance share the owner's state lock; no hard request-latency bound is implied.

Polling also limits how many positions it scans. An empty page can have
`has_more: true` and an advanced cursor when more positions remain to be checked.
Continue from the returned cursor; this flag does not promise another active
message. See the [protocol guide](protocol.md).
