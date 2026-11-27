<a id="lokales-kurzbeispiel-operator-und-zwei-worker"></a>

# Local Short Example: Operator and Two Workers

This file is set up with absolute paths according to `.local/agent-demo/AGENT-COMMANDS.md` Use
the generated file. Start three separate agent sessions, one each MCP -process with the
specified command as stdio- MCP The command alone does not start an AI agent, if your agent does
not MCP -Configuration at runtime supported, configure it in the host before the session starts.
CLI -Access is possible. Do not use a third party role or its keys.

<a id="gemeinsame-startanweisung--an-alle-drei-sitzungen-geben"></a>

## Shared starting instructions – give to all three sessions

```text
You only participate in a short local mynofy demo (about 5-10 minutes).
Owner: agent-demo
human topic: demo/human
operator topic: demo/operator
worker topic: demo/agents
Projektverzeichnis: @STATE@/work
Own permanent status: @STATE@/agent-state/<dein-name>.json

Read all three topics. Worker writes only demo/agents, the operator writes in demo/operator and
demo/agents. Binding human instructions come only from demo/human and have priority. Work only
on this example, not on the mynofy repository. Large artifacts remain in the project directory.

Use free messages payload= {"text":"..."} Agreed demo scheme in the text: child=< INTRO | ASSIGN
| ACCEPT | STATUS_REQUEST | STATUS | RESULT | PAUSE | QUESTION | MANDATE | DONE >; from=<name>;
to=<name|all>; ticket=<ID|->; state=<pending|running|success|paused>; ref=<mandate or order
reference|->; text=<short content> This is a convention of this example, not a new broker
protocol version. ACK No blind-retry with unclear publish output, no duplication of technical
work after re-poll.

Poll every three topics every 5 seconds for this short demo. Use next_cursor per topic as an
after; get more pages at has_more. Polling helpers as a subprocess are allowed, but messages
must be processed during work. Not just poll once and then stop participating. Report status
after 10 minutes of participation and ask if no completion has been made. Not getting human
permission for each poll.
```

## session 1: Operator

MCP server process:

```sh
@MCP_OPERATOR@
```

Instruction in addition to the common text:

```text
Your name is operator-001. Read @ROOT@ / skills/mynofy-operator/SKILL.md and the referenced
topic rules. You do not coordinate and perform any technical work. Introduce yourself in the
operatortopic. Clarify with the human before the start mandate, risk profile. If the human
already specifies it in the topic, use it without repeated questions. Document the mandate with
the demo reference DEMO - MANDATE -1 in the operatortopic and briefly ask for confirmation in
demo/human. waiting for this confirmation before you delegate. The once confirmed mandate is
valid until revocation or completion of tasks, without term or renewal. Changes require human
confirmation.

Give TASK -001, then subtasks to worker-001 (implementation) and worker-002 (review). Worker 2
first checks the finished result of Worker 1. Use skills from their ideas and get explicit
assumptions. Ask at least once the status of a worker so that the example shows this exchange.
Check result and review against the completion criteria and just register the conclusion in
demo/operator. Do not take any implementation or technical code check yourself. After successful
demo completion, finish participation; do not start a new project.
```

## session 2: Worker 1

MCP server process:

```sh
@MCP_WORKER_1@
```

Additional instruction:

```text
Your name is worker-001. Read @ROOT@/skills/mynofy/SKILL.md and its shared workflow rules.
Introduce yourself in demo/agents as a Python implementer.
Wait for a mandated assignment of operator-001; confirm acceptance explicitly.
In the demo project, implement greeting.py with greet(name). Strip leading and
trailing whitespace and return "Hi, <name>!". Raise ValueError for an empty
or whitespace-only name. Write a small unittest
file, run it and report the result, file paths and remaining issues. Don't change other
projects. Responses to status queries. Stay available until the demo ends for queries.
```

## session 3: Worker 2

MCP server process:

```sh
@MCP_WORKER_2@
```

Additional instruction:

```text
Your name is worker-002. Read @ROOT@/skills/mynofy/SKILL.md and its shared workflow rules.
Introduce yourself in demo/agents as the independent reviewer. Wait for
an assignment within the mandate from operator-001 and explicitly accept it.
Review worker-001's result for the greeting, trimming and empty-name cases.
Run the tests. Report clearance or specific findings in the agreed schedule. Don't change
implementation files yourself. Responses to status queries and remain available for queries
until demo completion.
```

<a id="mensch-startauftrag-und-bestätigung"></a>

## Human: Start order and confirmation

First service and observer start, then the three agent sessions, execute the following command
in the human terminal, and the wording is a suggested mandate for this example and will only be
mandated by your message.

```sh
@ROOT@/scripts/agent-demo.sh human 'operator-001: Coordinate only the short greeting demo with worker-001 and worker-002. You may assign and prioritize work, request status and coordinate results. Leave implementation and review to the workers. Use only the demo project and the supplied local service. Do not add agents, install software or use other network services. Small local file mistakes are acceptable. Pause and report possible writes outside the demo directory or unexpected external actions. The mandate lasts until revoked or the demo is complete. Completion requires the greet implementation, passing tests and an independent review. Record DEMO-MANDATE-1 and request my confirmation.'
```

After the documented mandate is visible in the observer, check it and confirm:

```sh
@ROOT@/scripts/agent-demo.sh human 'I confirm DEMO-MANDATE-1 as documented by operator-001 in the operator topic within the scope of my instructions.'
```

More messages simply with `agent-demo.sh human 'Dein Text'`, or interactively:

```sh
@ROOT@/scripts/agent-demo.sh human
```

To stop: `agent-demo.sh human 'All participants: stop the demo, acknowledge the stop and briefly report open work.'` After that, if necessary, manually end the agent sessions: a topic message
is not a guaranteed process termination.

## CLI fallback without MCP

```sh
@ROOT@/scripts/agent-demo.sh client operator
@ROOT@/scripts/agent-demo.sh client worker-1
@ROOT@/scripts/agent-demo.sh client worker-2
```

Each has its own client process, JSONL requests via stdin, such as:

```json
{"version":1,"type":"poll","id":"read-human","topic":"demo/human","limit":20}
{"version":1,"type":"publish","id":"intro","topic":"demo/agents","payload":{"text":"kind=INTRO; from=worker-001; to=all; ticket=-; state=pending; ref=-; text=Implement and test Python code"}}
```

The scripts do not simulate agents and do not start paid AI calls.
