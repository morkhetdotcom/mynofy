# Demo 2: protocol negotiation

Like Demo 1, but with its own service, keys and persistent topics under `.local/agent-demo-2/`.
The first demo is not changed. Linux/macOS, Python 3, Rust and normal OS users; all clients
under the same UID.

```sh
# Einmalig:
./scripts/agent-demo-2.sh setup
# Terminal 1, even if restarted later:
./scripts/agent-demo-2.sh start
# Terminal 2:
./scripts/agent-demo-2.sh watch
# Terminal 3, one message per line:
./scripts/agent-demo-2.sh human
```

The generated `.local/agent-demo-2/AGENT-COMMANDS.md` file contains the human
startup and confirmation commands and all roles. Instead, give each of the three agent sessions **their
own full startup file**:

- `.local/agent-demo-2/operator-START.md`
- `.local/agent-demo-2/worker-1-START.md`
- `.local/agent-demo-2/worker-2-START.md`

Each file contains common expiration, unique role, and matching MCP-/CLI- command. MCP Deploy
before in the host; without MCP, the existing CLI is allowed. Absolute paths generated apply to
the setup environment. If the agent mounts differently, match the paths in his startup files
**previously**. Do not give keys of other roles to a session.

<a id="was-diesmal-anders-ist"></a>

## What is different this time

No pre-determined semicolon scheme and no pre-determined Ereignis-/Parameterbelegung. All three
retrieve the existing catalog. They negotiate a concrete version with event significance and
assignments on the agent topic. Both workers must expressly agree; technical ACK is no consent.
Maximum two revisions and two minutes of negotiation. No agreement means blockage, no silent
reversion to exclusively text.

Once agreed, real typed events are used for recurring status/lifecycle messages, including at
least one per worker and one status query with response. Free text is allowed for negotiation,
explanation and technical details.

The mandate is confirmed once and is valid until revocation or termination; there is no term and
no periodic renewal. Changes still require human confirmation. Requirements, assignments and
completion may be made without supplementary text in the agreed protocol.

The short technical task this time comes via the operator: time formatting function, small tests
and independent review. Worker start files do not provide the solution or specific requirements
in advance. Target size 5-10 minutes.

<a id="zustand-und-auswertung"></a>

## Condition and evaluation

Topics: `demo2/human`, `demo2/operator`, `demo2/agents`; Owner `agent-demo-2`. Rights and
persistence like demo 1: read all, only human writes to the human topic, operator writes to
Operator-/worker topic, Worker only to the agent topic. Content is backed up every second of the
time when changes occur and kept by default 24 hours; local agent state remains separate.
Observer shows text and real event payloads as JSON. Restart Observer.

For later evaluation, the operator records revision, consent messages and event messages in a
locally traceable manner; it is about observed agreement and application, not claimed
performance values. The test setup itself does not contain a simulation of the AI agreement.

Before quitting without further publishing:

```sh
./scripts/agent-demo-2.sh save
./scripts/agent-demo-2.sh status
```

Wait for `save_in_progress: false`, `dirty: false`, `last_error: null`, then terminate service
with Ctrl-C. A publish first confirms RAM, no disk commit.

Technical integration test (no AI simulation):

```sh
cargo build --locked -p mynofy-cli
MYNOFY_DEMO_VARIANT=2 python3 examples/agent-demo/test_demo.py
```

`MYNOFY_DEMO_DIR` and `MYNOFY_BIN` work as in Demo 1.
Use the same environment variables for all commands.
