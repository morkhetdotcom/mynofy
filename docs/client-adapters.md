<a id="clients-als-adapter-für-vorhandene-software"></a>

# Clients as adapters for existing software

## Produktmodell

A mynofy client can be a native SDK application, an AI agent, a wrapper around stdin/stdout or a
rule-based launcher for an existing binary. The connected software does not need to know mynofy.
This connects tools that are controllable via command line arguments, stdin, stdout, stderr and
exit codes. Programs that require an interactive terminal may need a separate PTY adapter; an
ordinary pipe wrapper cannot operate every terminal program unchanged.

Trigger evaluation, interpretation and process control are carried out by the client, the server
remains transport and limited message memory, and no scripts or programs are executed in the
message core.

| Clientform | Eingang | Implementation | Ausgang |
| --- | --- | --- | --- |
| Native application or agent | Structured messages | Application logic | Publish messages and status |
| Durable Process with Pipes | Appropriate messages are written on stdin after agreed framing | A running program processes the input | stdout is read framed and published according to rules; stderr treated separately |
| Binary pro Trigger | Appropriate message provides validated input values | Client starts a locally configured program with set arguments and optionally stdin | Exit code and limited output are translated into the agreed result profile |

Framing is a part of the adapter, such as JSONL or length-marked bytes. stdout alone does not
provide reliable message limits. Free text output can also contain logs; their interpretation
must be configured. Without conscious mapping, exit code 0 only means successful process output,
not automatically successful professional examination or job completion.

<a id="regelmäßige-interpretationsbekanntgabe"></a>

## Periodic disclosure of interpretation

In addition to communication and status, there is a separate topic, for example
`workflow/4711/interpretations`. at the start of the participation, after rule changes and
regularly during its participation, the client publishes its interpretation again. A technical
topic join is not necessary for this and does not exist in the pull protocol.

The notification shall describe at least this technical information; the specific wire field
names are still draft:

- Announcing subscriber and current Prozess-/Teilnahmeinstanz so that restarts are
  distinguishable.
- Profil/catalog including version and clear revision of the interpretation rules.
- Exact input topics and triggers: Kategorie/Aktion/Status, required Lauf/Versuch/step and
  other requirements.
- Effect: which technical action the client performs, which Status-/result messages follow and
  which inputs it requires.
- Validity of the notification and renewal rhythm.

The announcement does not contain secrets; local executable paths and complete process
environments do not need to be published; for the participants, the announced effect is
critical; renewed announcement of the same revision does not change the interpretation; if rules
or profiles change, a new revision is necessary; the communication clarifies the required
consent and its activation.

Disclosure means “this is how I work”, not automatic approval or registration of execution
rights. Static-configured clients can repeatedly announce an immutable profile; dynamic agents
can agree on a new profile in the communication topic and then announce their activated
interpretation. The contract must prevent a participant from changing other roles or obligations
just by a new announcement.

<a id="ablauf-und-speicherbegrenzung"></a>

## Expiration and storage limitation

Announcements are normal time-limited messages. The application selects finite validity and a
shorter renewal interval under the topic policy. A specific standard for both times has yet to
be established; no new broker TTL semantics will be introduced.

For each Teilnehmer/Instanz, the observer keeps the latest valid announcement and reviews it.
Old announcements remain active with the broker until expiration; they are not replaced by a new
publish. Short finite TTL and appropriate renewal time limit the validity. For a new
participation instance, the profile determines when their announcement replaces the old one;
different instances may not inadvertently claim the same exclusive role.

In the absence of renewal, the interpretation is considered to be no longer available, which
means that the binary has been stopped, nor that an ongoing job has been terminated. A monitor
detects this itself; mynofy does not generate an offline message on expiration. Expired
announcements must not justify new execution clearances.

A common topic currently does not offer a consumer-tested individual producer identity per
message. The application must be able to prove the actual origin for binding
Rolen-/rule mappings; a self-registered participant field is not sufficient for this.
Likewise, a notification that identifies an external coordinator, his statement and no atomic
job assignment by mynofy remains.

<a id="regeln-eines-binary-adapters"></a>

## Rules of a Binary Adapter

Prior to implementation, at least the following points shall be established:

1. Local assignment of triggers to program and arguments, input validation and mapping. Messages
   do not select any shell commands. Simple implementation starts a fixed program directly with
   separate arguments.
2. Framing of stdin/stdout, separate stderr, caps on input and recorded output. Drop large
   outputs externally and report only one reference. Free payload remains limited to 4096 bytes.
3. Parallelism limit and limited backlog: no unlimited number of processes or waiting tasks.
4. Duration limit, termination, behavior at the end of a permanent process and treatment of its
   child processes.
5. Repeated poll, duplicate trigger, client restart and lost result behavior. A process call and
   the publish of its result are not atomic; automatically repeated triggers can have duplicate
   effects.
6. Feste status mapping: START/SUCCESS only after the start defined in the profile,
   EXECUTE/SUCCESS after the defined step, COMPLETE/SUCCESS only for the overall contract
   actually concluded. Report application acknowledgments as separate messages.
7. Required topic rights as well as expiration and renewal of the own interpretation
   announcement.

The launcher is itself the authenticated producer, and the called program does not automatically
need its own key, and it does not separately authenticate a claim about a user or device behind
the adapter.

<a id="beispiel"></a>

## Example

A client is configured locally for a test binary. It regularly publishes in the interpretation
topic: “I observe EXECUTE/PENDING for the test step of the agreed run profile. If appropriate, I
call the permanently configured test runner and report the output in the status topic.”

An AI publishes the test order in the communication topic or coordinates the step there. The
appropriate trigger is published on the topic defined in the profile. The client checks context
and jurisdiction, starts the binary, processes the output and reports the result. A person can
read the interpretation, suggest changes in the communication topic and track what is actually
happening based on the status topic.

Repeating the interpretation is not a repeat of the test order; the client does not respond to
its own announcement with a new binary call.

## implementation status

The CLI client uses Protobuf on the wire and JSONL on stdin/stdout. The adapter described below
implements Once-/JSONL-process execution and regular interpretation publication. PTY Support,
dynamic rule negotiation and persistent execution checkpoints remain open.

<a id="implementierter-adapter-und-lokaler-test"></a>

## Implemented adapter and local test

`mynofy adapter` implements a single locally configured trigger and exactly one simultaneous active process step. No additional waiting client queue: no further trigger is polished during processing. The interpretation continues during this process. Entries remain with the owner only until their TTL.

```sh
./scripts/setup-adapter.sh
cargo run --locked -- serve --config .local/adapter/config.yaml
```

In a second terminal:

```sh
cargo run --locked -- adapter --socket .local/adapter/run/mynofy.sock --key .local/adapter/client.pk8 --owner local --rules .local/adapter/rules.json
```

In a third terminal, open the CLI client:

```sh
cargo run --locked -- client --socket .local/adapter/run/mynofy.sock --key .local/adapter/client.pk8 --owner local
```

In it, send an exact trigger:

```json
{"version":1,"type":"publish","id":"trigger","topic":"workflow/4711/communication","payload":{"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":40,"status":7,"parameters":[{"u64":4711}]}}
{"version":1,"type":"poll","id":"status","topic":"workflow/4711/status"}
{"version":1,"type":"poll","id":"rules","topic":"workflow/4711/interpretations"}
```

If necessary, resend the status query after the program runs. Setup provides all three topics
and uses the same client key for all roles for the local test. Configure separate
identities/ACLs for real separate roles. The examples use `/bin/cat` under Linux/macOS; other
platforms need a matching program and WSS instead of Unix socket. Setup does not overwrite an
existing demo directory.

The [Once-configuration](../config/adapters/echo.json) starts `/bin/cat` per trigger. The [JSONL
-Configuration](../config/adapters/echo-stdio.json) uses `mode: "stdio_jsonl"`: a process
remains, receives exactly one JSON line of the event per trigger and returns exactly one valid
JSON line. To test, specify the appropriate rule file with `--rules`; the topics are identical.
The program must comply with this one-to-one framework. PTY/prompt handling and any log outputs
on stdout are therefore not supported.

Trigger comparison is exact including parameters and catalog version. The rule file contains
triggers, Start-/Erfolgs-/failure event, Profil/Revision, three topics, local
Programm/Argumente, polling interval, maximum runtime, output limit, and renewal and
announcement-TTL. A new professional run requires a suitable local rule configuration. Program
and arguments are not transferred from a message; the input is the framed JSON event
representation on stdin.

After successful process start, the configured `started` event is reported. For a once process,
exit code 0 is considered a success after full limited output; in permanent JSONL mode, a valid
complete response line is considered a success; this is the local execution profile, not a
general technical guarantee of success for each program. stdout is published as a limited byte
payload before the separate success event; both are not atomic. stderr is discarded. spawn
error, unsuccessful exit, invalid JSONL output, timeout, and output overrun result in the
configured error event. A failed persistent process is not automatically repeated for the same
job; the next explicit trigger can start a new process.

The interpretation is published before the first poll, then regularly with the same revision and
new finite TTL. It contains a new random participation instance per adapter start, the reported
participant, effect, trigger, status mapping, profile and revision; no local
Programmpfade/Argumente or secrets. Standard example: renewal after 10 seconds, validity 30
seconds. There is still no replacement of old messages or broker registry.

The adapter advances its in-memory cursor after reading the trigger and before executing the program. Application completion is reported separately. A restart may replay a still-active trigger, so downstream effects can repeat. SIGINT and SIGTERM stop the adapter.

On Unix, the adapter starts configured programs in their own process group. Timeout and adapter shutdown terminate that group while the child is still tracked. Programs that detach into another group require a separate supervisor if their descendants must be bounded. Workflow authorization and deduplication remain deployment decisions.
