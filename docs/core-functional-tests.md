# Core functional and reproducible mutation tests

The owner core is tested against a small reference model, equivalent protocol paths and bounded mutations. The workspace suite runs these tests without external fuzzing tools.

## State model and encoder equivalence

The state model mixes publish, poll, producer inspection, expiration, bounded cleanup, clock rollback, ACL changes and reconnects. It also attempts deletion of populated topics, recreates empty topics and tests old cursors against new messages. It checks permissions, record and index consistency, and owner, topic and producer accounting. The simulated clock avoids timing sleeps. Different seeds select different active-message limits.

The protocol comparison compares JSON-facing commands with typed Protobuf, and compares direct response bytes with the generated encoder. Coverage includes expiration, event parameter boundaries and payloads through 4,096 bytes, plus rejection of 4,097 bytes. The focused poll-budget test also checks page membership, cursor and continuation at the budget boundary.

<a id="datenstrukturen-und-parser"></a>

## Data Structures and Parsers

Check the additional reference and mutation tests:

| area | Check |
| --- | --- |
| Ordered indexes | Comparison with `BTreeMap` at Insert/Replace/Remove/Get/Mutation, open and closed range limits, U64 margins, reclaim and change between small vectors and bitmap blocks. |
| expiration index | Comparison with simple reference table with mixed insert sequence, equal and exact deadlines, deadline 0/U64:: MAX, pop and reclaim. |
| JSON-/Protobuf-Frames | Mutated bytes on non-authenticated sessions must not generate content; output binary errors remain decodeable. |
| YAML/JSON-configuration | Mutated valid/invalid templates are rejected or re-issued, read in and compared as accepted values/Configs. |
| Binary message images | Mutation, truncation, insertion, duplication and random bytes; accepted images are fully materialized and rewritten to byte accuracy; and short-read readers with 1-31 bytes per read. |
| Full checkpoints | Mutation of the control tables and embedded images; accepted checkpoints must be re-written in the same way. |
| Topic-Manifest | Check sizes, Topic-/client bindings, entries and references; parser calls do not change manifests or referenced topic files. |
| Encrypted envelopes | Round trips of different plain text lengths, incorrect key, size limit, mode confusion, and mutated headers, ciphertext and tags; an accepted decryption must not contain modified authenticated bytes. |

In the case of a part of the message image-/Checkpoint-/Manifest-Mutationen, the respective
framing checksums are recalculated, thereby achieving structural checks after the checksum
comparison. Targeted changed valid byte payloads ensure that the image parser actually accepts
new valid content. The cryptographic tags are not repaired subsequently.

The random choices and sequences of operations use a fixed, adjustable seed. Cryptographic
Nonces/Challenges continues to come from the existing provider. These are limited, seed-driven
model and mutation tests with explicit oracles; they provide neither full path coverage nor
proof of arbitrary input. Automatic failure shrinking is not implemented.

<a id="ausführen-und-fehler-wiederholen"></a>

## Perform and repeat errors

Fast standard run with 16 model seeds × 600 steps and mostly 2048 cases per additional test:

```sh
cargo test --locked -p mynofy-server functional -- --nocapture
```

Extended run with 64 model seeds × 1000 steps and 16,384 cases per additional campaign:

```sh
./scripts/test-core-functional.sh
```

The JSON/Protobuf-Vergleich is limited to 1500 requests per run. The short-read test uses five
fixed reader sizes. The remaining eight campaigns use `MYNOFY_FUZZ_CASES` each.

Other start seed, including U64 seed orbit:

```sh
MYNOFY_TEST_SEED=0xffffffffffffffe0 ./scripts/test-core-functional.sh
```

Own run size:

```sh
MYNOFY_MODEL_SEEDS=32 MYNOFY_MODEL_STEPS=1000 MYNOFY_FUZZ_CASES=8192 \
  ./scripts/test-core-functional.sh
```

The model test reports the specific seed and the last 24 operations in the event of errors.

```sh
MYNOFY_TEST_SEED=0x6d796e6f66790001 MYNOFY_MODEL_SEEDS=1 MYNOFY_MODEL_STEPS=1000 \
  cargo test --locked -p mynofy-server seeded_reference_model -- --nocapture
```

For parser errors, additionally apply the original `MYNOFY_FUZZ_CASES` setting and filter the
relevant test name. Seed, case index and the same source status belong to the error description.

<a id="abgrenzung-und-nächste-schritte"></a>

## Delimitation and next steps

This extension tests existing behavior. New restart detection, Offline-Rekey/Backup-Werkzeuge,
memory conversions and performance work are not part of this step. Existing
Prozess-/Transporttests remain in the full suite; new continuous transport tests have not been
brought forward. Native macOS testing remains for GitHub CI.

The user order is: first complete function tests including Modell-/Mutationstests, then restart
and treat memory, then measure performance and optimize targeted.

## Validation status


The linked test report records the status before the subsequent [Restart and memory
maintenance](restart-and-storage.md); its source fingerprint is historical and is not issued as
a fingerprint of subsequent changes.
