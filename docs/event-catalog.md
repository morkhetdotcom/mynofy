<a id="eingebauter-ereigniskatalog-mynofy-version-1"></a>

# Built-in event catalog: mynofy, version 1

The categories, actions and status values are from
`logging-engine/morkhet_logging_engine/src/provider_strings.rs`; the numerical IDs are from
`types.rs`. Source checksums are in [maschinenlesbaren
Definition](../protocol/catalogs/mynofy-v1.json). `logging-engine` remains unchanged and is not
a build or runtime dependency.

The catalog can be accessed via `get_catalog` and has the fixed numerical catalog ID `1`.
Protobuf and the two payload variants are implemented: typed events and free bytes with a hard
maximum of 4096 bytes. Generated enums and numerical membership checks avoid one catalog scan
per event. Technical Signaturen/Profiles and automatic catalog matching remain draft.

## Beschlossenes Zielmodell

- A built-in catalog with name `mynofy` and version `1`; compact numerical references in the
  event messages.
- Transfer built-in definitions to Enums/statische tables during build. No parsing, reloading or
  registering catalog definitions in the message path.
- Optional own catalogs are a later extension: fully load and validate at start, then keep
  immutable. No dynamic catalog transmission through messages.
- Typed events: Category, Action, Status and up to three primitive values, together with a
  maximum of 24 bytes of pure values. Type-/field identifiers and message metadata are added; this is
  not a statement of the complete wire size.
- Free payload: maximum 4096 actual bytes; no large buffer in each small event and no automatic
  splitting.
- Professional events have the same message envelope and the same ACL-/workflow rules as free
  bytes.A professional `SUCCESS` is not a transport ACK and vice versa.

<a id="übernommene-werte"></a>

## Values taken

ID `0` is reserved in all three namespaces and is invalid. In particular, `UNSPECIFIED` is a
valid status with ID `1`. The numeric gaps from the template remain; no automatic renumbering.
These numbers denote the catalog and do not yet determine its prototype representation.

### Kategorien

43 entries.

| ID | Name |
| ---: | --- |
| 1 | `CORE` |
| 2 | `ENGINE` |
| 3 | `SYSTEM` |
| 10 | `MEMORY` |
| 11 | `THREAD` |
| 12 | `JOB` |
| 13 | `SCHEDULER` |
| 20 | `FILESYSTEM` |
| 21 | `STORAGE` |
| 22 | `DATABASE` |
| 23 | `CACHE` |
| 30 | `NETWORK` |
| 31 | `RPC` |
| 32 | `HTTP` |
| 40 | `LOCK` |
| 41 | `MUTEX` |
| 42 | `ATOMIC` |
| 43 | `ASSET` |
| 44 | `RESOURCE` |
| 45 | `BUNDLE` |
| 46 | `SERIALIZATION` |
| 47 | `DESERIALIZATION` |
| 48 | `RENDER` |
| 49 | `SHADER` |
| 50 | `TEXTURE` |
| 51 | `GPU` |
| 52 | `EVENT` |
| 53 | `MESSAGE` |
| 54 | `AUDIO` |
| 55 | `INPUT` |
| 56 | `ANIMATION` |
| 57 | `SCENE` |
| 58 | `ADDRESSABLES` |
| 59 | `STREAMING` |
| 100 | `GAMEPLAY` |
| 101 | `PLAYER` |
| 102 | `AI` |
| 103 | `PHYSICS` |
| 300 | `LOG` |
| 301 | `METRICS` |
| 302 | `TELEMETRY` |
| 303 | `DEBUG` |
| 304 | `SECURITY` |

### Aktionen

49 entries.

| ID | Name |
| ---: | --- |
| 1 | `CREATE` |
| 2 | `DESTROY` |
| 3 | `LOAD` |
| 4 | `SAVE` |
| 10 | `START` |
| 11 | `STOP` |
| 12 | `INIT` |
| 13 | `SHUTDOWN` |
| 14 | `RESET` |
| 20 | `READ` |
| 21 | `WRITE` |
| 22 | `UPDATE` |
| 23 | `DELETE` |
| 30 | `COMPARE` |
| 31 | `MATCH` |
| 32 | `MISMATCH` |
| 33 | `VALIDATE` |
| 40 | `EXECUTE` |
| 41 | `SCHEDULE` |
| 42 | `COMPLETE` |
| 43 | `CANCEL` |
| 50 | `CONNECT` |
| 51 | `DISCONNECT` |
| 52 | `SEND` |
| 53 | `RECEIVE` |
| 60 | `ALLOCATE` |
| 61 | `FREE` |
| 62 | `MAP` |
| 63 | `UNMAP` |
| 70 | `LOCK` |
| 71 | `UNLOCK` |
| 72 | `ACQUIRE` |
| 73 | `RELEASE` |
| 80 | `QUEUE` |
| 81 | `DEQUEUE` |
| 82 | `WAKE` |
| 83 | `SLEEP` |
| 90 | `REGISTER` |
| 91 | `UNREGISTER` |
| 92 | `ATTACH` |
| 93 | `DETACH` |
| 100 | `OPEN` |
| 101 | `CLOSE` |
| 102 | `FLUSH` |
| 103 | `SYNC` |
| 110 | `COMMIT` |
| 111 | `ROLLBACK` |
| 120 | `QUERY` |
| 121 | `LOOKUP` |

### Status

21 entries.

| ID | Name |
| ---: | --- |
| 1 | `UNSPECIFIED` |
| 2 | `SUCCESS` |
| 3 | `FAILED` |
| 4 | `TIMEOUT` |
| 5 | `CANCELLED` |
| 6 | `RETRY` |
| 7 | `PENDING` |
| 8 | `PARTIAL` |
| 9 | `INVALID_ARGUMENT` |
| 10 | `NOT_FOUND` |
| 11 | `ALREADY_EXISTS` |
| 12 | `BUSY` |
| 13 | `LOCKED` |
| 14 | `CONFLICT` |
| 15 | `UNAVAILABLE` |
| 16 | `RESOURCE_EXHAUSTED` |
| 17 | `PERMISSION_DENIED` |
| 18 | `NOT_IMPLEMENTED` |
| 19 | `SKIPPED` |
| 20 | `DEFERRED` |
| 21 | `CORRUPTED` |

### Parameter types

| ID | Type | Raw value bytes |
| ---: | --- | ---: |
| 1 | `U32` | 4 |
| 2 | `U64` | 8 |
| 3 | `I32` | 4 |
| 4 | `I64` | 8 |
| 5 | `F32` | 4 |
| 6 | `F64` | 8 |
| 7 | `BOOL` | 1 |

The types also come from `types.rs` Its native byte order, memory layout, layout IDs and generic
flagbits are not adopted as a network format. Proprietary flags are so far without defined
meaning and remain outside of this base.

<a id="prüfung-anhand-von-anwendungsfällen"></a>

## Testing against use cases

The [Product testing with 50 use cases](use-cases.md) ranks the entire catalog. `UNITY`,
`UNITY_OBJECT`, `UNITY_SCENE` and `UNITY_ASSET` have been removed from the built-in design. Your
IDs 200-203 remain unfilled and may not be reused for other terms.

The draft now contains 43 categories, 49 actions and 21 status values; other domain-specific
categories are marked in the review report as candidates for later optional catalogs; they
have not yet been removed; new terms and signatures in the report are proposals, not already
available catalog values.

Typed payloads do not yet check technical event signatures. Executable profiles must specify:
allowed combination, meaning, type and order of parameters, and versioning rules. Technical
statuses do not trigger automatic repetition or broker state change. The protobuf format uses
separate numeric fields; no native EventKey storage layout.

<a id="vom-client-abrufen"></a>

## Retrieve from client

After successful registration, any client can request the built-in definition, even without
topic rights:

```json
{"version":1,"type":"get_catalog","id":"schema-1","name":"mynofy","catalog_version":1}
```

The answer is a `catalog` frame with `request_id` and the full object under `catalog`, including
categories, actions, statuses, parameter types, and intended limits. The [complete
example](../protocol/v1/catalog-examples.json) and the
[metadata schema](../protocol/catalogs/catalog.schema.json) describe the format. The CLI client
accepts the query like other JSONL commands; the Rust client library offers `get_catalog(&mut
socket, "mynofy", 1).await` and returns the catalog object.

Unknown name or unavailable version results in `not_found`; missing login results in
`unauthorized`. Existing client rate limits and response byte limits also apply here. Retrieval
does not change state and does not set up a catalog. Clients can cache the response per
Owner/Name/Version; as long as version 1 is a draft, retrieve it after a service update.

The embedded JSON definition is parsed once per process and checked for valid, unique IDs/Namen,
at engine start before accepting commands. Multiple engines share the same immutable definition.
Individual queries copy the prepared metadata into their limited response; no re-interpretation
of the catalog file and no Datei-I/O in the request path. Generated enums and static numerical
checks are already implemented for events.

## unified Aktions-/Statusmodell

[Workflow-Semantik](workflow-semantics.md) defines the meaning of existing triples as the basis: `(JOB, START, PENDING)` = start is pending, `(JOB, START, SUCCESS)` = started, `(JOB, ACQUIRE, SUCCESS)` = assumed, `(JOB, COMPLETE, SUCCESS)` = completed. Additional statuses RUNNING/ACCEPTED or a REQUEST/EVENT-Field are not provided for this basic model. [50 UCs als data](../protocol/catalogs/use-cases-v1.json) only reference existing identifiers. New wire fields or an installed AI skill do not arise.

The catalog is also the basis for [Running-related agreements between agents and
people](agent-coordination.md). Negotiating your own application protocols does not install
additional catalogs in the service.
