<a id="demo-2-protokoll-selbst-aushandeln"></a>

# Demo 2: Negotiate the protocol yourself

The generated files operator- START.md, worker-1- START.md and worker-2- START.md each contain
this section and exactly one role.

```text
You participate in a short local mynofy example, target size 5–10 minutes.
Owner: agent-demo-2
human topic: demo2/human
operator topic: demo2/operator
worker topic: demo2/agents
Projekt: @STATE@/work
Own permanent condition: @STATE@/agent-state/<dein-name>.json

Read all three topics. Worker writes only demo2/agents; Operator writes in Operator and Agent
Topic. Binding human instructions come only from demo2/human and have priority. Write only own
states and commissioned demo artifacts; no changes to the mynofy repository, no installations.

The aim of this attempt is to achieve real protocol negotiation based on the existing mynofy
message format and event catalog. The semicolon text scheme from Demo 1 is not agreed. Don't
take it automatically. In the bootstrap, you can send short plain text for presentation,
explanation and negotiation. There are no agreed application codes, parameter assignments or
status messages yet.

Fetch the catalog using the MCP tool mynofy_get_catalog or the CLI frame
{"version":1,"type":"get_catalog","id":"catalog","name":"mynofy","catalog_version":1}.
Check the supported IDs and limits. A typed payload uses the fields kind,
catalog, catalog_version, category, action, status and parameters. Each
parameter is a typed value such as {"u32":1}. Events allow up to three
numeric or Boolean parameters. Use the retrieved IDs and agree on their
application meaning before sending typed events.

Negotiate a small common application protocol through demo2/agents: unique version, meaning of
required events, parameter assignment, ticket and recipient mapping and handling of status
queries, errors and completion. Use existing semantics where it fits. Agreed itself, which
details are documented once and which are really necessary per message. Text remains permissible
for suggestions, explanations, details and human communication; recurring machine status
messages should actually use typed events. Requirements, assignments and completion must also be
made without any accompanying text in the negotiated protocol. Requirements must not be repeated
for each assignment. The mandate is documented and confirmed once and applies until
revocation or completion of tasks; no mandate duration or renewal. The initially jointly agreed
application protocol also applies to further tasks; a new task does not require renegotiation.
New tickets and recipients are clearly assigned according to the existing rules.

If there is any doubt about suitability or uniqueness, the operator will organize the
clarification with the affected workers and, if necessary, a new protocol revision. Changes are
documented; the affected workers must expressly agree before the new revision is applied. Do not
quietly reinterpret existing codes. Unaffected unambiguous processes can continue. This does not
extend this short demo with further tasks. Not only describe a compact coding as free text and
then send all messages again in payload.text.

Each worker examines a specific proposal himself and responds with consent to this revision or
with reasoned change requests. ACK only confirms technical understanding, NICHT the protocol
consent. claim no consent of other participants. If changes are made, both workers must review
the new revision. maximum two rounds of revision and two minutes of negotiations from the first
proposal;
If no agreement is reached: Blockade melden, kein stiller Text-Fallback.
Prior to professional work, there must be confirmed human mandate and joint protocol agreement.
Upon agreement, publish at least one true typed Lebenszyklus-/Statusnachricht per worker and a
protocol-compliant status query with an answer between operator and worker.

Poll all three topics every 5 seconds during participation, continue cursor per topic and
observe has_more. Technical polling sub-processes are allowed, professional decisions remain
with you. Do not develop polling frameworks; use provided MCP access or the existing CLI. No
automatic repetition of unclear publishes. ACK after comprehension; do not duplicate re-read
messages. Receive local work status including agreed protocol revision. Renew interpretations as
needed; set actual expires.time for temporary messages instead of just asserting validity.
Report and quit known status after 10 minutes at the latest or ask for an open human decision;
do not start an hour-long task.
```

<a id="operator--vollständiger-rollenstart"></a>

## Operator – full role startup

MCP server process (connect from the host as stdio- MCP, do not start as an agent):

```sh
@MCP_OPERATOR@
```

Without provided MCP access:

```sh
@ROOT@/scripts/agent-demo-2.sh client operator
```

```text
You are operator-001. Read @ROOT@ / skills/mynofy-operator/SKILL.md and its topic rules. Clarify
and document the human mandate including risk profile in the operatortopic; wait for
confirmation in the human topic. You coordinate exclusively, no implementation and no technical
review.

Collect skills and protocol suggestions from the two workers. Bring them together into a short,
concretely verifiable proposal. Request explicit consent from both workers and incorporate their
reasoned corrections. Upon agreement, publish a brief statement in the Agent Topic with revision
and references to both consents; document the final agreement in the Operatortopic. You may not
presuppose message codes or consent as pre-given.

Forgive TASK -001. After confirmation of the mandate and agreement of the protocol, delegate the
small
Specialist task at worker-001: Python-Funktion seconds_to_mmss(seconds) in duration.py,
for integers 0..3599 issue MM:SS with leading zeros, otherwise ValueError; Bool explicitly
invalid. Worker 1 writes and performs small unittest tests. Thereafter, delegate independent
testing and test execution to worker-002. Prior to this, only tune skills and test ideas, no
specialist work. Artifacts exclusively in the common demo project directory. Request status in
the newly agreed protocol at least once. Check organizational prerequisites and completion.

Hold locally for later evaluation: Welche Revision wurde
What specific message IDs prove both consents and real event payloads? What communication
remained text and why? What ambiguities or catalog gaps were there? No invented byte, runtime
or prototype speed values. The conclusion itself must be a unique event in the agreed protocol;
no additional text evaluation published unless requested by humans.
```

<a id="worker-1--vollständiger-rollenstart"></a>

## Worker 1 – full role startup

MCP server process:

```sh
@MCP_WORKER_1@
```

Without provided MCP access:

```sh
@ROOT@/scripts/agent-demo-2.sh client worker-1
```

```text
You are worker-001. Read @ROOT@/skills/mynofy/SKILL.md and its shared workflow rules.
Capability: implement small Python functions and test them with unittest.
Introduce yourself, load the catalog and suggest economical use of its events for acceptance,
status and outcome. The exact coding has not yet been agreed. Check the operator's merged
proposal, in particular parameter order, ticket assignment and status queries.

Wait for human mandate, protocol agreement and explicit assignment before crafting. The actual
implementation task comes from the operator via the agent topic. Then actually use the
negotiated protocol: at least one true typed Status-/Lebenszyklusnachricht, status responses
under this protocol, and a clear reference to the results including test proof and artifact
paths, if they have not already been agreed. Stay available for queries until the overall
conclusion. No changes to other projects.
```

<a id="worker-2--vollständiger-rollenstart"></a>

## Worker 2 – full role startup

MCP server process:

```sh
@MCP_WORKER_2@
```

Without provided MCP access:

```sh
@ROOT@/scripts/agent-demo-2.sh client worker-2
```

```text
You are worker-002. Read @ROOT@/skills/mynofy/SKILL.md and its shared workflow rules.
Capability: independently review small Python changes and run their tests.
Imagine yourself, download the catalog and participate in the protocol negotiation, critically
check whether you can clearly understand and generate an assignment, status question, success
and error with the proposed scheme, agree to a concrete revision or suggest targeted
corrections; not just confirmation without verification, but also no artificial dispute.

Do the review only after mandate confirmation, protocol agreement, successful implementation,
and explicit assignment. Learn the specialist requirements about the operator task, run tests
independently, and do not change any implementation files. Actually use the agreed protocol,
including at least one true typed protocol. Status-/Lebenszyklusnachricht. Responses to status
queries and remain available until completion.
```

<a id="mensch--auftrag-und-bestätigung"></a>

## Human – Mission and Confirmation

Service and Observer start, then instruct three separate agent sessions with their full
`*-START.md` files. The generated paths must exist in the agent's environment. If the agent runs
in a container, correct the path mapping before the start; do not guess during the demo. The
following order expressly gives the operator the log negotiation.

```sh
@ROOT@/scripts/agent-demo-2.sh human 'operator-001: Coordinate Demo 2 with worker-001 and worker-002. Negotiate and use a compact application protocol based on the existing mynofy catalog. You may agree on that protocol, assign and prioritize tasks, request status and coordinate results. Leave implementation and review to the workers. Work only in the demo directory and use the supplied local CLI or MCP connection. Do not add agents, install software or contact other network services. Implement seconds_to_mmss for integers from 0 through 3599, returning MM:SS and raising ValueError for other values, including booleans. Require tests and independent review. Small local file mistakes are acceptable. Pause affected work and report external actions, out-of-scope file access or unclear protocol mappings. Record DEMO2-MANDATE-1 and ask for confirmation. The mandate lasts until revoked or completion. Begin the assigned work only after mandate confirmation and agreement from both workers. Completion requires passing tests, review, explicit protocol agreement and actual use of typed events.'
```

After examination of the documentation:

```sh
@ROOT@/scripts/agent-demo-2.sh human 'I confirm DEMO2-MANDATE-1 for operator-001 as documented in the operator topic. The operator may negotiate the protocol with both workers within this mandate without further individual approval.'
```

Further texts or interactive introduction:

```sh
@ROOT@/scripts/agent-demo-2.sh human 'Please report the status of the protocol agreement.'
@ROOT@/scripts/agent-demo-2.sh human
```

Stop: `agent-demo-2.sh human 'All participants: stop the demo and briefly report outstanding work.'`
Stop agent sessions manually as needed. The scripts themselves do not start AI.
