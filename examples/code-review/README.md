# Code review: Codex writes, Claude reviews

A human gives a bounded task to an operator. The operator assigns implementation
to Codex and review to Claude. Participants agree on message meanings before they
use compact events. No AI provider is built into mynofy: you supply your own agent
processes, or run the deterministic exchange below without any model accounts.

## Run the example yourself

Requirements: Linux/macOS, an unprivileged user, Rust/Cargo, C compiler, CMake,
and Python 3. No Python packages or TLS certificates are required for this local
Unix-socket example.

From the repository root:

```sh
cargo build --locked -p mynofy-cli
python3 examples/code-review/demo.py setup
python3 examples/code-review/demo.py start
```

In a second terminal, watch all three topics:

```sh
python3 examples/code-review/demo.py watch
```

In a third terminal, choose **one** of the two modes below. Use a fresh directory
for a second run: set `MYNOFY_REVIEW_DIR` consistently in every terminal. Setup
refuses an existing state directory.

### Mode A: inspect a scripted protocol exchange

```sh
python3 examples/code-review/demo.py exchange
```

This uses distinct registered clients to publish the human request, mandate,
introductions, protocol proposal, explicit acceptances, and typed events. A
read-only observer verifies that every message is present. It also verifies that
the coding worker cannot publish to the human topic.

The exchange is deterministic and intentionally contains no AI inference, code
generation, real test execution, or genuine review verdict. Its `SUCCESS` events
are scripted examples, not evidence that a model completed a task. Do not use this
mode as the input state for a real agent task.

### Mode B: connect real agents

Open the generated `.local/code-review/AGENT-COMMANDS.md`. It contains English
start prompts and absolute MCP commands for the operator, Codex, and Claude.
Configure those commands as stdio MCP servers in the corresponding agent hosts,
then give each agent only its own role section plus the shared contract.

The wrapper can launch each role's MCP endpoint directly:

```sh
python3 examples/code-review/demo.py mcp operator
python3 examples/code-review/demo.py mcp codex
python3 examples/code-review/demo.py mcp claude
```

These commands serve MCP to a host; they do not start AI models. Credentials stay
in the MCP process, not in prompt text. For a host without MCP, use `client ROLE`
as the JSONL CLI fallback.

Talk to the agents from another terminal:

```sh
python3 examples/code-review/demo.py human \
  'Please propose a mandate for a small slugify function and an independent review. Ask me about the risk boundary before starting.'
```

After the operator proposes a specific mandate and you agree to it:

```sh
python3 examples/code-review/demo.py human \
  'I confirm mandate M1 for operator-001: standard-library code and review under the example work directory only. No installs, external calls, or deployment.'
```

Use the actual proposed mandate identifier; do not confirm an unknown proposal.
You can send an authoritative stop at any time:

```sh
python3 examples/code-review/demo.py human 'Stop TASK-001 and its subtasks. Report the current state and open work.'
```

The task is deliberately small: implement `slugify(text)` and a few tests in
`.local/code-review/work/`. The acceptance criteria are lowercased ASCII
letters/digits, runs of other characters replaced by one hyphen, no leading or
trailing hyphens, and an empty result for empty input. Claude reviews the actual
files and checks. The operator coordinates; it does not perform either role's
technical work.

## Topics and authority

| Identity | Read | Publish |
| --- | --- | --- |
| Human | All three topics | All three topics; authoritative directions belong in `review/human` |
| Operator | All three topics | `review/operator`, `review/agents` |
| Codex / Claude | All three topics | `review/agents` |
| Observer | All three topics | None |

The owner `code-review` enforces these ACLs and stamps each message with
`producer_id`. Bind each agreed actor to that immutable stable client ID, not to a
name or ID inside the payload. Setup writes the role mapping to the generated
`IDENTITIES.json`. The scripted exchange verifies every observed producer and
shows that a Claude payload claiming Codex still has Claude’s `producer_id`.
Human authority comes from the protected human topic, not a message saying
“the human told me.” Independent topics do not provide an atomic global ordering.

The operator records a mandate and risk profile in `review/operator`. A human
confirms it once in `review/human`; it lasts until revoked or the task completes.
Workers introduce themselves in `review/agents`, receive current state, and
explicitly accept delegated work. New workers get a handover rather than guessing
from a single recent message.

## What the agents agree on

The sample agreement is [protocol.json](protocol.json). It is application-level
data, not a new broker protocol or dynamically registered catalog.

1. Name and revision: `code-review/1` using catalog `mynofy`, version 1.
2. Actor numbers: operator `1`, Codex `2`, Claude `3`; bind them to real client IDs.
3. Ticket `1` means `TASK-001` within owner `code-review` and `review/agents`.
4. Each event has three `u32` parameters: `[ticket_number, actor_id, attempt]`.
5. Text carries negotiation, assignments, artifact references, and useful findings.
   Routine status uses catalog events after explicit acceptance.
6. Review begins only after coding output, checks, and an operator assignment.
   Parent completion requires the coding and review results.
7. Timeouts do not imply failure or authorize retries. Human stops override the
   agreement. Application acceptance does not mean task success.

The built-in catalog supplies existing numeric categories, actions, and statuses.
The parties negotiate their meaning and parameter mapping for this workflow.
They cannot add enum members or change catalog IDs through a message.

| Event | Meaning in this agreement |
| --- | --- |
| `JOB / START / PENDING` | Actor accepted its assigned step; execution is not yet claimed. |
| `JOB / ACQUIRE / SUCCESS` | Actor claims that step for this attempt. This is an application assertion, not a broker lock. |
| `JOB / START / SUCCESS` | Actor has begun executing the step. |
| `JOB / COMPLETE / SUCCESS` | Actor's step and its completion criteria succeeded. |
| `JOB / COMPLETE / FAILED` | Actor reports failure; the parent remains open. |

One operator allocates main tickets per agent topic. Workers can reference
subtasks such as `TASK-001-CLAUDE-1`; negotiate a new numeric subtask mapping before
using it in compact events. Reuse the agreement for later tasks. If it becomes
ambiguous, the operator organizes an explicitly accepted new revision.

## Example conversation

The following is an illustrative real-agent conversation, not a recorded model
run. `A` is `review/agents`, `O` is `review/operator`, and `H` is `review/human`.
Brackets abbreviate typed parameters and are not literal CLI JSON.

```text
H  Human     Please implement and review slugify; work only in the example directory.
O  Operator  M1: coordinate coding and review; no installs or deployment. What risk limits apply?
H  Human     Confirmed M1. Escalate if the task needs external dependencies or broader edits.
A  Codex     codex-001: Python implementation and tests available.
A  Claude    claude-001: independent review available.
A  Operator  Propose code-review/1, actor and ticket mapping, event meanings, and review dependency.
A  Codex     Accept code-review/1 as actor 2.
A  Claude    Accept code-review/1 as actor 3.
A  Operator  Accepted. TASK-001 coding assigned to codex-001, attempt 1.
A  Codex     JOB/START/PENDING       [1,2,1]
A  Codex     JOB/ACQUIRE/SUCCESS     [1,2,1]
A  Codex     JOB/START/SUCCESS       [1,2,1]
A  Codex     Artifacts: slugify.py, test_slugify.py; checks passed. Review requested.
A  Codex     JOB/COMPLETE/SUCCESS    [1,2,1]
A  Operator  TASK-001 review assigned to claude-001 against the stated criteria.
A  Claude    JOB/START/PENDING       [1,3,1]
A  Claude    JOB/ACQUIRE/SUCCESS     [1,3,1]
A  Claude    JOB/START/SUCCESS       [1,3,1]
A  Claude    Reviewed the code and tests; no blocking findings against the agreed criteria.
A  Claude    JOB/COMPLETE/SUCCESS    [1,3,1]
O  Operator  JOB/COMPLETE/SUCCESS    [1,1,1]
```

If review finds a problem, Claude sends `JOB/COMPLETE/FAILED` with the same
parameters and a concise finding. The operator arranges an authorized correction;
it does not convert acceptance into success or silently increment attempts.

## The actual message shape

Codex's completion uses category `JOB=12`, action `COMPLETE=42`, status
`SUCCESS=2`, plus the three agreed parameters:

```json
{"version":1,"type":"publish","id":"codex-done-1","topic":"review/agents","payload":{"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":42,"status":2,"parameters":[{"u32":1},{"u32":2},{"u32":1}]}}
```

You can paste that into `python3 examples/code-review/demo.py client codex` after
agreement and actual completion. The request ID is only correlation. The broker
assigns its own message ID; recipients read it by polling and manage their cursors locally. Protobuf transports numeric references instead of repeatedly sending enum
names, role names, and the negotiated schema. The JSON CLI representation above
is intentionally readable, not an estimate of the binary wire size.

Three `u32` values contain 12 raw value bytes, plus event tags, message metadata,
and framing. This does not claim a 12-byte message or a fixed total overhead.
Free byte payloads remain capped at 4,096 bytes. Large patches and test logs belong
in the workspace or an artifact store; publish references and short results.

## Persistence and cleanup

Topics use unencrypted one-second periodic content saves in this local example;
security state is stored separately. A publish is still only RAM acceptance.
A hard crash before the next successful save can lose recent content. Expiry
continues to apply after restore; persistence does not keep messages forever.

Stop with Ctrl-C. Reuse `start` with the same state to restore saved messages.
Keep the generated directory private: it contains keys. Do not rerun `exchange`
on the same task as though replay were a new execution. To start afresh, stop all
participants and choose a new `MYNOFY_REVIEW_DIR`; do not delete active state.
