# mynofy Bash-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Shell library for the existing Rust- CLI. Bash ab 3.2, `mynofy`, `jq` and the usual Unix tools
() `head`, `base64`, `tr`, `mktemp`, `cat`, `rm` No new Rust dependency, no own
WebSocket-/Protobuf- or cryptography code.

## Einrichten

```bash
source /absoluter/pfad/mynofy/sdks/bash/mynofy.sh
MYNOFY_BIN=/absoluter/pfad/mynofy/target/debug/mynofy
MYNOFY_OWNER=local
MYNOFY_KEY=/absoluter/pfad/mynofy/.local/unix/client.pk8
MYNOFY_SOCKET=/absoluter/pfad/mynofy/.local/unix/run/mynofy.sock
```

The variables do not need to be exported; the public key and the topic rights must be registered
with the owner; and if the Unix client and service are running as the same normal OS user; the
CLI checks path, mode and peer-UID.

For WSS, use the variables `MYNOFY_URL=wss://HOST:PORT` and
`MYNOFY_CA=/path/to/ca.pem`. The CLI uses TLS 1.3 and verifies the certificate, hostname
and owner identity. No insecure option or plaintext fallback. Optional after rotation
`MYNOFY_CLIENT_ID` Set on the original stable ID.

For owner tunnels, set `MYNOFY_VIA_TOPIC`, `MYNOFY_OWNER_CA` and optionally `MYNOFY_OWNER_NAME`.
The outer CA checks the proxy, the owner-CA checks the endpoint. The rust client takes over the
full tunnel and the owner-auth.

<a id="senden-pollen-und-bestätigen"></a>

## Send, poll and confirm

```bash
mynofy_publish_text local/events 'Review requested for TASK-001'
mynofy_publish_file local/events ./trigger.bin application/octet-stream
mynofy_publish_event local/events 12 42 2 \
    u64:18446744073709551615 i64:-9223372036854775808 bool:false

page=$(mynofy_poll local/events '' 20)
printf '%s\n' "$page"
# After processing/takeover save; at has_more read more pages immediately:
cursor=$(printf '%s' "$page" | jq -r '.next_cursor')
mynofy_poll local/events "$cursor" 20
```

Bytes are limited to 4096. Files can contain NUL bytes, bash strings cannot. Text is encoded
unchanged including final line breaks. Events use the existing catalog 1/v1; IDs and parameter
meaning from catalog/Anwendungsvereinbarung. Up to three parameters: `u32`, `u64`, `i32`, `i64`,
`f32`, `f64`, `bool`. For events, the last option `--expires 2030-01-01T00:00:00Z` is possible;
without expiration, the topic policy decides. Float values and catalog IDs are finally validated
by the CLI.

| Funktion | Argumente |
| --- | --- |
| `mynofy_publish_text` | `TOPIC TEXT [EXPIRES]` |
| `mynofy_publish_file` | `TOPIC FILE [CONTENT_TYPE] [EXPIRES]` |
| `mynofy_publish_event` | `TOPIC CATEGORY ACTION STATUS [TYPE:VALUE ...] [--expires TIME]` |
| `mynofy_poll` | `TOPIC [AFTER] [LIMIT]` |
| `mynofy_list_messages` | `[TOPIC] [CURSOR] [LIMIT]` |
| `mynofy_get_message` | `MESSAGE_ID` |
| `mynofy_get_permissions` | `TOPIC` |
| `mynofy_get_catalog` | `[NAME] [VERSION]`, Standard `mynofy 1` |
| `mynofy_keygen` | `NEW_KEY_FILE` |
| `mynofy_rotate` | `NEW_KEY_FILE`, must not yet exist |
| `mynofy_session` | JSONL -Commands over stdin |

Empty optional Filter/Cursor as `''`, Limits 1..100, Standard 20.Paging your own messages keep
filters **and page size**. The broker keeps no reader state. No automatic publishing, polling
loop or local progress file in SDK.

Integers are written as tested decimal strings directly into CLI -JSON and answers are output
unchanged. No bash arithmetic or `jq` rounding to `u64` / `i64`. For further reading, also use a
parser with exact integers: for example `jq` with unchanged decimal number retention; older
Versionen/Builds can round large numbers.

<a id="rückgaben-und-fehler"></a>

## Returns and errors

Success provides the complete CLI - JSON response to stdout, diagnosis on stderr. The library
does not change shell options, traps, IFS or umask when sourced.

| Exit status | Meaning |
| --- | --- |
| 0 | Erfolgreiche Antwort |
| 10 | Broker error; complete error frame on stdout, such as ACL or rate limit |
| 20 | CLI-/Transport-/lokaler I/O-errors; no automatic retry |
| 21 | Invalid or incorrectly assigned response |
| 64 | Argument-/configuration errors or local payload check failed |
| 129/130/143 | Operation interrupted by HUP/INT/TERM |

Bei 20/21 or abort, a mutation may have already been assumed to have been matched first with
your own messages and local state, not re-send blindly. `set -e` explicitly address expected
errors:

```bash
if result=$(mynofy_publish_text local/events 'Review'); then
    printf '%s\n' "$result"
else
    status=$?
    printf 'Status %s, response: %s\n' "$status" "$result" >&2
fi
```

Each convenience call opens a new authenticated CLI connection using a private temporary
response file (directory 0700, file 0600) that is deleted at the end of the operation. Signals
sent to the operation process terminate the CLI child and wait. The calling process manages its
own signal handling and process group; the library does not overwrite its traps. `SIGKILL` does
not allow clean-up.

There is no continuity queue: parallel convenience calls are independent connections. The
request ID text `bash-sdk` is for response mapping within this connection and is **not** a
deduplication key.

## Persistent sessions

For multiple commands without repeated Prozess-/Auth-Aufwand:

```bash
mynofy_session <<'JSONL'
{"version":1,"type":"get_catalog","id":"catalog","name":"mynofy","catalog_version":1}
{"version":1,"type":"poll","id":"poll","topic":"local/events","limit":20}
JSONL
```

This is the JSONL -Passthrough der CLI: Protobuf on the network, local JSON - Ein-/Ausgabe.
Broker errors are response lines; CLI -Exit status remains unchanged. The comfort exit codes and
additional response assignment check do not apply here. EOF closes the connection; no automatic
reconnection or repetition CLI hat feste Timeouts pro connection phase/Austausch, typically ten
seconds, no bash-configurable total timeout. Welcome-/Neustart-metadata In this case, CLI After
a known restart, intentionally read again without a cursor; no automatic restart detection.

<a id="schlüsselwechsel"></a>

## Key changes

`mynofy_keygen NEW.pk8` creates a new private file and provides the data to
Registration. `mynofy_rotate NEW.pk8` uses the controlled client change: the CLI creates and
stores the new key **before** the commit, refuses to overwrite existing files and keeps it even
if the output is unclear. After success, change `MYNOFY_KEY` and obtain the original client ID
via `MYNOFY_CLIENT_ID`.

## Tests

As a normal user with Rust/Cargo, Python 3 (standard library only), jq and OpenSSL:

```sh
./scripts/test-bash-sdk.sh
# Optionally test another bash version:
MYNOFY_BASH=/pfad/zu/bash ./scripts/test-bash-sdk.sh
```

Real Unix-/WSS-services, Bytes/Events, integer limits, cursor, ACL, Keygen/Rotation, JSONL
session and TLS rejection. Controlled CLI doubles check quoting, large integers, incorrect
zugeordnete/defekte responses, exit codes, signal abort and temporary files. The request
builders use the common SDK vectors; Wire-/Auth-/Rotationstests remain in the delegated Rust
protocol. Tunnel arguments are checked; real multihop tunnel tests are checked at Rust/CLI. on
Linux with Bash 3.2 and 5.2. Native macOS check remains open for the CI. Further steps: [SDK -
Overview](../../docs/sdks.md).

## Owner-authenticated producer identity

Read `.items[].message.producer_id` in the JSON result for the stable client identity stamped by the owner. It is immutable, survives authorized key rotation and storage restore, and is separate from any sender or actor claim inside the payload. Match workflow roles to this value. This is an assertion over the protected owner connection, not a portable cryptographic message signature.

Early broker rate limits may omit `request_id` because rejection happens before request parsing. With `rate_limited` and a valid integer `retry_after_ms`, Bash preserves the response and returns exit 10. Wrong IDs and other missing-ID errors still return 21. The SDK never retries a mutation automatically.
