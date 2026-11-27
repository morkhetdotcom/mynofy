# Managing local topics at runtime

The separate Admin API supports `topics list|get|create|policy|delete`. These commands are available through the local Admin socket. The owner is the local `service.id`. Remote routes are configured at startup.

```sh
mynofy admin topics list --socket run/admin.sock --limit 20
mynofy admin topics get jobs/status --socket run/admin.sock
mynofy admin topics create jobs/status --default-ttl 60 --max-ttl 3600 --socket run/admin.sock
mynofy admin acl set CLIENT jobs/status --allow publish,subscribe,inspect_own --socket run/admin.sock
mynofy admin topics policy jobs/status --default-ttl 120 --max-ttl 7200 --socket run/admin.sock
mynofy admin topics delete jobs/status --socket run/admin.sock
```

`create` requires an explicit policy, creates a RAM topic and starts with
an empty ACL. A publish requires an existing topic. As at startup, names must be unique, nonempty and at most 256 UTF-8 bytes. The configuration limit is 10,000 topics including remote routes. TTL must satisfy `1 <= default <= max <= 31536000` seconds. An existing local or configured remote topic returns `conflict`.

`list` returns local topics in lexical order. The limit is 1–100 and
defaults to 20. `--after NAME` resumes after that name, even if the named topic was deleted. `next_after: null` marks the end. Each page reflects the current list. `get` and list entries report the owner, policy, storage mode, active messages and physically resident records. The diagnostic counts scan the message stock.

## Policy and existing messages

`policy` changes TTL defaults and limits for future publishes. Accepted
messages retain their original expiration. A persistent topic becomes dirty and the new policy becomes durable on its next successful save. Validation precedes mutation. Listener settings, resource limits and routing are outside this command's scope.

## Deleting and recreating topics

`delete` requires an empty RAM topic. Active messages and expired
records awaiting cleanup return `topic_not_empty`. Retry after the normal bounded cleanup finishes.

A `manual` or `interval` topic returns `storage_transition_forbidden` even when empty. Stop the service, configure the RAM baseline and run `storage topic-ram` offline before deleting the RAM topic after restart. [Restart and storage](restart-and-storage.md)

Deletion clears the topic ACL and invalidates revisions, sessions and pending rotations for affected clients. Other topics retain their message references. The core keeps numeric slots compact. When needed, it moves the last topic slot and its record references, ACLs and content schedule atomically under the core lock. This Admin operation can take time proportional to the stock it moves.

A recreated topic starts with an empty ACL and takes its initial head from the current owner's global acceptance position. Older cursors in the same boot can thus continue without skipping new messages because of a reused position. Restart and cursor rollover rules still apply.

## Persistence and restart

Topic definitions and policies are part of RAM or content state. ACLs belong to the security state.

- A new RAM topic disappears on restart unless an exported configuration containing it becomes the startup configuration.
- `topics storage` can switch a RAM topic to `manual` or `interval` if the backend and any required secret were configured at startup. A successful save records the topic, policy and messages. Startup can restore that topic even if it is absent from the original configuration.
- Create and policy responses report `durable: false` for the topic definition or policy. Use content save status to confirm persistence.
- Deleting a RAM topic from the running configuration leaves the configuration file unchanged. After a restart with the same file the topic definition reappears, with an empty ACL if the reset was durable. To persist the new topology, export it and use that configuration at the next start.
- Create and delete enqueue a bounded `topic_reset` security record. It clears ACLs and maintains a marker for dynamic topics. Recovery discards ACLs for marked topics absent from both configuration and content. An unmarked missing local ACL target causes recovery to fail.
- With `security_storage.sync: true`, the Admin response confirms the security reset after journal sync. With `sync: false`, an unsynced reset can be lost in a crash. A full security queue rejects create or delete before mutation.

Topology and policy changes are rejected during a running or unresolved content save with `content_save_busy` or `content_storage_requires_restart`.

## Admin JSON and tests

Send a JSON line to the separate Admin socket.

```json
{"version":1,"command":{"op":"topic_create","topic":"jobs/status","policy":{"default_ttl_seconds":60,"max_ttl_seconds":3600}}}
```

See the [schema](../protocol/admin/v1.schema.json) and [examples](../protocol/admin/examples.json) for all Admin operations.

Core tests cover default deny, paging, atomic validation, expiration, queue and save-busy handling, slot moves, cursors after recreation, export and restart. A CLI integration test uses a real Unix socket to check creation, queries, paging, policy, ACLs, export and deletion. The workspace suite also runs the storage interruption tests.
