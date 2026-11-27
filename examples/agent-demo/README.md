# Lokales Operator-Demo (Linux/macOS)

Prerequisites: Rust/Cargo, Python 3 and three agent sessions with local file sharing and MCP or
CLI access. Start all processes under the same normal OS user; the Unix socket does not accept
foreign UID.

Im Repository:

```sh
# Build once, create keys/ ACLs/YAML and agent statements:
./scripts/agent-demo.sh setup

# Terminal 1: Start service in the foreground (also for later restarts):
./scripts/agent-demo.sh start
```

In Terminal 2 read all topics:

```sh
./scripts/agent-demo.sh watch
```

The observer has his own key with only reading privileges. He shows all still active messages at
the start, then new ones. He confirms no messages technically. After service restart, the
observer restarts; thus he deliberately starts again at the active stock. Standard polling
interval 1 second, changeable with `watch --interval 2`. Worker-/Operator-Intervalle are
independent of this.

The generated file **`.local/agent-demo/AGENT-COMMANDS.md`** contains absolute MCP commands and
copyable instructions for operators and both workers, as well as the human
startup and confirmation commands. giving the common text and role text to each session.The project is
just a small Python feature with tests and review; no changes to mynofy.

Communicating with the human topic in Terminal 3:

```sh
./scripts/agent-demo.sh human 'Meine Anweisung'
# Or interactive, one message per line; Ctrl-D terminates the client:
./scripts/agent-demo.sh human
```

Rights: Man writes everywhere, operator in `demo/operator` and `demo/agents`, worker only in
`demo/agents`. all read all three topics. binding human instructions only in `demo/human`.

<a id="persistenz-und-beenden"></a>

## Persistence and Termination

All three topics have `storage.mode: interval`, `seconds: 1`. Content and security are
separately stored under `.local/agent-demo/content` and `security`. Messages are stored locally
unencrypted, active by default 24 hours (expiration continues even during service downtime).The
setup does not change existing data and refuses a second setup on the same path. Use only
`start` for restarts. Topic configuration is in `config.yaml`.

Before the end of the service trigger a common save of all topics:

```sh
./scripts/agent-demo.sh save
./scripts/agent-demo.sh status
```

Wait for `content_storage.save_in_progress` to be false and `dirty` to be false and `last_error`
to be null, while no new messages are generated, and only then terminate Terminal 1 with Ctrl-C.
Publish first confirms RAM; a hard termination can lose changes that have not yet been saved. A
regular shutdown waits for accepted saves, but does not start an additional save itself.

Topic persistence does not replace the agents' local work levels under `agent-state/`.The script
itself does not keep tickets or workflows.

Optionally set the same absolute `MYNOFY_DEMO_DIR` before **each** command to use a different
private state directory. `MYNOFY_BIN` selects its own binary; the default setup runs `cargo
build --locked -p mynofy-cli`.

The integration test uses its own temporary directory and checks the real topic rights, text
input, observer output and Save/Neustart:

```sh
cargo build --locked -p mynofy-cli
python3 examples/agent-demo/test_demo.py
```
