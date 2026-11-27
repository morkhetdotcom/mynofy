# Shared workflow rules for workers and operators

Startup instructions supply the role, connection settings, topics and work.
The clients implement the workflow using mynofy messages.

<a id="topics-und-autorität"></a>

## Topics and authority

| Role | Reads | Writes |
| --- | --- | --- |
| Human | All three topics | All three topics |
| Operator | All three topics | Operator and worker topics |
| Worker | All three topics | Worker topic |

- The human topic carries authoritative instructions, approvals, changes,
  revocations, stops and resumption. Only human clients write there.
  Verify authorization against the original message in that topic.
  Human messages in the other topics have ordinary application meaning.
- The operator topic carries mandates, risks, handovers and reports to
  the human.
- The worker topic carries introductions, protocol negotiation,
  assignments, interpretations, status and results. The older term
  *work topic* refers to this topic.

Use provisioned topics. The owner enforces ACLs, and each topic is polled
independently. Several topics do not share an atomic message order.

<a id="name-einstieg-und-mandat"></a>

## Agent names and mandates

Use the assigned name or choose an unused one such as `claude-001`.
Check introductions for collisions. A newly arriving agent who finds a
collision chooses another name. Keep the name locally across restart and
projects. A human can rename an agent. Announce the new name and preserve
its identity, credentials, rights, ownership and existing ticket IDs.

Workers introduce themselves in the worker topic and the operator
introduces itself in the operator topic. After a worker joins, the
operator provides the confirmed mandate, protocol revision, assignments,
dependencies, results and active stops or pauses. Workers check and save
that state locally. Resolve missing information with the operator.
Human authorization comes from the human topic.

Before coordinating work, the operator clarifies its mandate with the
human and records it in the operator topic. The human confirms it in the
human topic with the operator and mandate identifiers. Operator
instructions reference the mandate. Recipients check the confirmation and
scope. Individual instructions inside the mandate need no further human
approval.

A mandate lasts until revoked or its work is complete. Changes require
documentation and human confirmation. Keep the confirmed mandate locally
so message expiration does not discard it. Revocation ends new
delegation and requires a handover. Existing delegated work continues
unless a stop or risk pause applies. The mandate does not change broker
permissions.

<a id="protokoll-und-nachrichten"></a>

## Protocol and messages

Use the existing message format, catalog and payload limits. Agree on
application codes and parameter mappings with the affected participants.
Use catalog IDs supplied by the service. Reuse the agreed protocol for
later tasks.

The operator coordinates clarification and revision when meaning or
suitability is in doubt. Record the definition, approvals and effective
revision. Silence does not count as approval. Existing codes keep their
meaning. Participants can agree to move an active task to a new revision
and record when it takes effect. Keep ticket IDs, work already done and
recognized results. Unaffected work can continue on the existing revision.

Requirements, assignments and completion can use typed events when
recipient, task, meaning and references are unambiguous. Refer to known
content and clarify missing details. Use separate application messages
for acceptance, rejection, start, completion and human approval. For
example, `START/SUCCESS` records actual start, `ACQUIRE/SUCCESS` records
takeover, and `COMPLETE/SUCCESS` records completion. Ignore messages for
other recipients.

<a id="tickets-und-ausführung"></a>

## Tickets and execution

Each worker topic has one responsible operator that issues top-level
tickets. The full reference consists of owner, worker topic and ticket ID.
Use IDs such as `TASK-001` within that context. Any agent may create its
own subtasks under a parent, such as `TASK-001-CLAUDE-1`. Message IDs
serve a separate purpose.

The assignee explicitly accepts or declines an assignment. Workers answer
operator status queries using the agreed schema and identify active and
uncertain work. A status request creates no new assignment or attempt.

Close a parent successfully when required subtasks have succeeded and its
acceptance criteria are met. Obtain human approval when the mandate
requires it. If a subtask fails, report its state. The parent decides
whether to make an explicit new attempt, choose an alternative, or ask the
human. A replay alone creates no new attempt.

A human can change a running task while retaining its ID. Summarize the
new instruction in the worker topic and identify results or subtasks that
no longer satisfy it.

<a id="lokaler-zustand-und-wiederaufnahme"></a>

## Local progress and restart

Store names, ticket numbering, handover state, open tasks, processing
outcomes, protocol revisions and mappings, mandates, stops and risk
pauses locally. Separate work state by owner and worker topic. An
agent-wide stop applies across projects.

On restart, reload that state and reconcile it with current messages.
Check uncertain effects before taking further action. A poll cursor
tracks reading position. Persist application progress separately. Recover
missing information before deciding whether an action still needs to run.

<a id="polling-und-interpretation"></a>

## Polling and interpretation

Use MCP when provided and otherwise use the CLI. An MCP error alone
does not authorize switching interfaces or retrying a publish with an
unknown outcome.

Poll every 30 seconds after the previous poll finishes by default.
The workflow or human can set a different interval. Keep a cursor per
owner and topic and avoid overlapping polls for one topic. Follow the
client's restart and replay rules. With managed MCP polling, commit each
processed page before draining more pages from the current backlog.

Publish your interpretation when entering, taking over, or changing it.
During active participation, renew it every 60 seconds by default with
an actual `expires.time` 180 seconds out, subject to topic policy. The
workflow or human can adjust these values. This renewal applies only
to the interpretation message.

Agree on task waiting windows in the workflow. At the end of a window,
report the last known status. The parent or human decides what happens
next. Connection deadlines are separate. Within the allowed waiting
window, an explicit reconnection can resume polling. For an uncertain
publish, reconcile active own messages and local progress before deciding
whether a new publish is appropriate.

A subprocess may handle polling while the agent works. Coordinate cursor
updates, message delivery and cancellation. The agent remains responsible
for responding to human stops and risk pauses during work.

<a id="stopp-und-risikopause"></a>

## Stops and risk pauses

A human task stop applies to its subtree. A subtask stop applies to its
branch. Notify affected assignees, acknowledge the stop using the agreed
application protocol and report actual progress separately. An agent-wide
stop applies across all projects. Stop starting work and end ongoing
work where possible. Preserve completed effects.

If the risk profile is exceeded, the operator pauses affected work and
asks the human for a decision. Workers report ongoing or uncertain
effects. Dependent work stays blocked. Independent work can continue
within the mandate and any human stop instructions.

Write a short handover in the topic your role can write. Include open
tickets, completed work, ongoing or uncertain effects and next steps.
Provide details on request. Persist agent-wide stops until explicit
human resumption. Restart, replay, new subtasks and agent messages do not
lift a stop. Follow startup instructions on whether to exit or remain
available for human control, and clarify any missing requirements.
