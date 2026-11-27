<a id="produktprüfung-interaktionen-und-ereigniskatalog"></a>

# Product Review: Interactions and Event Catalogue

Status: November 11, 2026. Basis is the built-in draft `mynofy`, version 1. This review includes
product suggestions, no additional runtime function. Available are JSON-Publish/Poll, ACL,
expiration, producer queries and catalog retrieval. Typed events, free byte
payloads with hard 4096 byte limit, prototype and locally configured client adapters are
implemented. Professional expiration profiles and automatic protocol negotiation remain draft;
service tunnels are now implemented.

## Produktperspektive

A producer wants to inform another participant, ask for a decision, initiate an action or
communicate a compact state. People act through an application, devices through a client or
gateway and agents through their SDK. Anyone can be a producer and a consumer. The professional
role comes from the application and topic; the broker authenticates the actual client and does
not grant implicit rights to an allegedly represented person or device.

The small event variant is intended to transport recurring, clearly defined messages without
texts. Free bytes up to 4096 bytes are used for short texts and small application-specific
content. Large artifacts remain outside the broker. A URL-/Hash-Verweis may require the free
variant; numeric IDs are only sufficient if the application already knows its meaning.

In the tables below, only expressions in the column “Existing Values” are combinations of
existing identifiers. The triples follow the uniform semantics described below; concrete
parameter types and event signatures are still draft. Missing names in other columns are
suggestions. The catalog does not make a trigger executable, does not create workflows and is
not a state automaton.

<a id="einheitliche-semantik-statt-zusätzlicher-zustandsnamen"></a>

## Unified semantics instead of additional state names

The current product decision uses existing triples `(Kategorie, Aktion, Status)`: `PENDING`
means that this action is pending; `SUCCESS` means that this action has successfully occurred or
been completed. SUCCESS At start means "started", not "job done". For adoption, the existing
spelling is `ACQUIRE`, not `AQUIRE`. No additional REQUEST/EVENT-Field and no new statuses
ACCEPTED/RUNNING are required for this basic pattern.

Who is allowed to act additionally depends on the agreed expiration profile, topic,
Lauf/Versuch, artifact status and the role of the publishing application. A triple alone is not
an execution release. The same agent can report a pending start and confirm its successful start
according to met conditions. A PENDING message is not a request addressed to each reader. An
observer may not derive its own execution from it.

The table is larger so that it can later form an AI skill with specific response rules. It is
not yet an installed skill. The [maschinenlesbaren 50
UCs](../protocol/catalogs/use-cases-v1.json) contain trigger triples, messages, required
context, allowed receiver response and error rule per UC. All referenced identifiers already
exist; the Signaturen/Profiles are draft. The catalog retrieval still provides the event
catalog, not automatically this UC draft.

<a id="50-anwendungsfälle"></a>

## 50 Applications

The “Request → Messages” column describes alternatives, not an instruction to send all results.
If a UC describes only one observation, no artificial PENDING phase is introduced. The message
lists are limited to the UC each; not every operation can deliver every error or success. In
particular, a known positive single result is never a release for another action.

<a id="aufträge"></a>

### Contracts

| UC | Processing/Constituents | request → messages | Kontext | Recipient rule / error case |
| --- | --- | --- | --- | --- |
| UC-01: Starting the order | Human/application → Worker | `(JOB, START, PENDING)` → `(JOB, START, SUCCESS)`, `(JOB, START, FAILED)` | Run, test, job definition | Worker checks prerequisites, takes over according to award rule and reports started run. Start failed: claim no completion. |
| UC-02: Taking over assignment | Worker ↔ Koordinator | `(JOB, ACQUIRE, PENDING)` → `(JOB, ACQUIRE, SUCCESS)`, `(JOB, ACQUIRE, FAILED)` | Run, trial, candidate, awarding authority | Only confirmed owner starts exclusive work. BUSY/CONFLICT: not working; notification alone does not assign a job. |
| UC-03: Performing the work step | Worker → application/Agent | `(JOB, EXECUTE, PENDING)` → `(JOB, EXECUTE, SUCCESS)`, `(JOB, EXECUTE, FAILED)` | Run, test, step number, known inputs | Successor waits for SUCCESS of this step and checks further prerequisites. FAILED/PARTIAL: blocked follow-up steps do not start. |
| UC-04: Complete order | Worker → requester | `(JOB, COMPLETE, PENDING)` → `(JOB, COMPLETE, SUCCESS)`, `(JOB, COMPLETE, FAILED)` | Run, test, results | Client accepts result only for current attempt. FAILED: Mark result not as usable. |
| UC-05: Order to cancel | Human/application → Worker | `(JOB, CANCEL, PENDING)` → `(JOB, CANCEL, CANCELLED)`, `(JOB, CANCEL, CONFLICT)` | Run, attempt, termination target | Worker stops as far as possible and reports CANCELLED. CONFLICT if the order has already been completed: clarify the effect. |
| UC-06: Reset start | Worker → requester | `(JOB, START, PENDING)` → `(JOB, START, DEFERRED)`, `(JOB, START, FAILED)` | Run, attempt, lack of prerequisite | DEFERRED reports deferment; coordinator decides later release. no silent timer or automatic retry by broker. |
| UC-07: Order in a row | application → Scheduler | `(JOB, QUEUE, PENDING)` → `(JOB, QUEUE, SUCCESS)`, `(JOB, QUEUE, FAILED)` | Run, try, target queue | Scheduler only reports SUCCESS when actually enrolled. RESOURCE_EXHAUSTED: claim no assumption. |
| UC-08: Taking Order from Queue | Scheduler → Worker | `(JOB, DEQUEUE, PENDING)` → `(JOB, DEQUEUE, SUCCESS)`, `(JOB, DEQUEUE, FAILED)` | Run, trial, queue, award rule | SUCCESS reports withdrawal; check exclusive assignment separately. NOT_FOUND: do not start from obsolete queue event. |
| UC-09: Report progress | Worker → Human/Monitor | Observation without requirement → `(JOB, UPDATE, PARTIAL)`, `(JOB, UPDATE, SUCCESS)`, `(JOB, UPDATE, FAILED)` | Run, attempt, processed/total with fixed units | Recipient updates display; does not start a follow-up step. PARTIAL is not a conclusion; total= 0/invalid Treat values by profile. |
| UC-10: Order to try again | Producer → Worker | `(JOB, START, PENDING)` → `(JOB, START, SUCCESS)`, `(JOB, START, FAILED)` | Same technical run, new attempt, new message ID | Start a new attempt only after an explicit producer decision and duplicate check. Unclear previous outcome: no automatic side effect. |

### AI

| UC | Processing/Constituents | request → messages | Kontext | Recipient rule / error case |
| --- | --- | --- | --- | --- |
| UC-11: Create code | Human/Planer → Coding-Agent | `(JOB, CREATE, PENDING)` → `(JOB, CREATE, SUCCESS)`, `(JOB, CREATE, FAILED)` | Run, test, repository and input | Agent creates artifact; CREATE/SUCCESS means created, not tested. FAILED/PARTIAL: Review only with expressly usable status. |
| UC-12: Check the code | Coding-Agent → Review-Agent | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Run, test, unchangeable artifact status, review profile | Review agent checks; SUCCESS meets exactly this review profile. FAILED does not meet release condition; assign exact cause separately. |
| UC-13: Changes after review | Review-Agent → Coding-Agent | `(JOB, UPDATE, PENDING)` → `(JOB, UPDATE, SUCCESS)`, `(JOB, UPDATE, FAILED)` | Run, new artifact status, known review requirements | Coding agent reworks and requests VALIDATE/PENDING for new status. Old successful test does not apply to changed status. |
| UC-14: Carrying out tests | Coding-Agent → Test-Agent/CI | `(JOB, EXECUTE, PENDING)` → `(JOB, EXECUTE, SUCCESS)`, `(JOB, EXECUTE, FAILED)` | Run, artifact status, agreed test step ID | SUCCESS of this test step can meet defined quality condition. test error and missing test execution distinguish; neither release. |
| UC-15: Retrieve information | Agent → Such-/Recherche-Agent | `(RESOURCE, QUERY, PENDING)` → `(RESOURCE, QUERY, SUCCESS)`, `(RESOURCE, QUERY, FAILED)` | Run, known Frage/Quellenregeln, result reference | Recipient provides the result externally or as a free payload. NOT_FOUND/UNAVAILABLE: do not invent an answer. |
| UC-16: Compare results | Two Agents Comparison Agent | `(RESOURCE, COMPARE, PENDING)` → `(RESOURCE, COMPARE, SUCCESS)`, `(RESOURCE, COMPARE, FAILED)` | Run, two fixed results, comparison criterion | SUCCESS states comparison executed; report equality separately over MATCH/MISMATCH. FAILED is not evidence of different content. |
| UC-17: Agent handover | Planer-Agent → Fach-Agent | `(JOB, ACQUIRE, PENDING)` → `(JOB, ACQUIRE, SUCCESS)`, `(JOB, ACQUIRE, FAILED)` | Run, test, confirmed transfer/offer status | New agent works only after a suitable assignment; old one finishes exclusive work. Lost confirmation: do not assume a double assumption. |
| UC-18: Check tool access | Agent → Policy Application | `(SECURITY, VALIDATE, PENDING)` → `(SECURITY, VALIDATE, SUCCESS)`, `(SECURITY, VALIDATE, FAILED)` | Run, concrete Operation/Ziel, standard | Only SUCCESS of the authorized examiner and matching execution rules allow tool call. PERMISSION_DENIED: refrain from operation; message does not change ACL. |

### Menschen

| UC | Processing/Constituents | request → messages | Kontext | Recipient rule / error case |
| --- | --- | --- | --- | --- |
| UC-19: Release Publication | Application → People about UI | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Run, artifact status, explicit human release profile | UI reports SUCCESS only after express approval; application checks matching profile. FAILED does not mean released; DEFERRED postpones decision. |
| UC-20: Send a short message | Human ↔ Human | `(MESSAGE, SEND, PENDING)` → `(MESSAGE, SEND, SUCCESS)`, `(MESSAGE, SEND, FAILED)` | Exact topics, free short text payload, agreed upon response convention | UI shows content; free text does not start a machine action. The UI needs a separate user action to record reading or consent. |
| UC-21: Report information | People about UI → Application | Observation without requirement → `(MESSAGE, RECEIVE, SUCCESS)`, `(MESSAGE, RECEIVE, FAILED)` | run or message reference, explicit acknowledgment profile | SUCCESS must document UI awareness, not release. Without explicit user action, do not derive from a poll. |
| UC-22: Announcing maintenance | Operator → Menschen/devices | `(SYSTEM, SCHEDULE, PENDING)` → `(SYSTEM, SCHEDULE, SUCCESS)`, `(SYSTEM, SCHEDULE, FAILED)` | Maintenance run, Termin/Zeitzone, specified participants | Recipient informs or plans locally; no immediate shutdown; expiration of message does not perform maintenance. |
| UC-23: Deferral | Man is a waiting agent | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, DEFERRED)`, `(JOB, VALIDATE, FAILED)` | Barrel, artifact status, release profile | DEFERRED leaves the dependent step blocked. Failure to make a decision is not consent. |
| UC-24: Escalating to Operators | Monitor/Agent → Human | Observation without requirement → `(EVENT, SEND, SUCCESS)`, `(EVENT, SEND, FAILED)` | Run, start, escalation stage, responsible person | UI informs the responsible person according to its own escalation rule. No guarantee that someone will take over; answer required separately. |

<a id="geräte"></a>

### Equipment

| UC | Processing/Constituents | request → messages | Kontext | Recipient rule / error case |
| --- | --- | --- | --- | --- |
| UC-25: Turn on the device | Human or rule → Device | `(SYSTEM, START, PENDING)` → `(SYSTEM, START, SUCCESS)`, `(SYSTEM, START, FAILED)` | Device in the topic, run, desired condition | Device reports SUCCESS after actual start according to profile. BUSY/FAILED: Do not set display as turned on. |
| UC-26: Turn off the device | Human or rule → Device | `(SYSTEM, STOP, PENDING)` → `(SYSTEM, STOP, SUCCESS)`, `(SYSTEM, STOP, FAILED)` | Device, run, desired condition, execution rules | Device reports successful standstill. TIMEOUT occupies no standstill; query actual state. |
| UC-27: Changing the nominal value | Regler/Human → Device | `(RESOURCE, UPDATE, PENDING)` → `(RESOURCE, UPDATE, SUCCESS)`, `(RESOURCE, UPDATE, FAILED)` | Device/property in the topic, barrel, unit, target value | SUCCESS only when the defined setpoint value is accepted in the profile. INVALID_ARGUMENT: reject value, do not silently shorten. |
| UC-28: Observe temperature | Sensor → Monitor/Regler | Observation without requirement → `(TELEMETRY, READ, SUCCESS)`, `(TELEMETRY, READ, FAILED)` | Sensor in the topic, Einheit/scaling, measured value, measuring time | READ/SUCCESS reports observation; rule checks age and limits. outdated measurement does not use for current control. |
| UC-29: Report meter reading | Counters → Application | Observation without requirement → `(METRICS, READ, SUCCESS)`, `(METRICS, READ, FAILED)` | Counter in the topic, unit, absolut/delta, Sequenz/Zeitraum | Application interpreted according to fixed measurement profile. Reset/Umlauf/Duplikate take into account, do not add delta twice. |
| UC-30: Report limit value | Sensor/Monitor → Human/Agent | Observation without request → `(RESOURCE, VALIDATE, FAILED)` | Monitored object, rule, measurement stand, threshold profile | FAILED reports injured condition; receiver follows agreed alarm rule. FAILED alone defines neither priority nor automatic countermeasure. |
| UC-31: Back to normal | Sensor/Monitor → Human/Agent | Observation without request → `(RESOURCE, VALIDATE, SUCCESS)` | Same rule and instance as the alert, updated measurement | SUCCESS reports again fulfilled condition. Unallocated normal message must not close any other alarm. |
| UC-32: Reset device | Operator/automation → Device | `(SYSTEM, RESET, PENDING)` → `(SYSTEM, RESET, SUCCESS)`, `(SYSTEM, RESET, FAILED)` | Device, run, reset scope and entitlement | SUCCESS reports the defined completed reset. Repeated message must not reset uncontrollably multiple times. |

<a id="betrieb"></a>

### Operation

| UC | Processing/Constituents | request → messages | Kontext | Recipient rule / error case |
| --- | --- | --- | --- | --- |
| UC-33: Starting the service | Supervisor → Service | `(SYSTEM, START, PENDING)` → `(SYSTEM, START, SUCCESS)`, `(SYSTEM, START, FAILED)` | Service, Lauf, Start-/readiness profile | SUCCESS occupies only the start state mentioned in the profile. process start is not automatically readiness. |
| UC-34: Stop service | Supervisor → Service | `(SYSTEM, SHUTDOWN, PENDING)` → `(SYSTEM, SHUTDOWN, SUCCESS)`, `(SYSTEM, SHUTDOWN, FAILED)` | Service, barrel, scope of orderly stop | SUCCESS only after the shutdown condition has been met. TIMEOUT/FAILED: clarify remaining work and condition. |
| UC-35: Building connection | application → Connector | `(NETWORK, CONNECT, PENDING)` → `(NETWORK, CONNECT, SUCCESS)`, `(NETWORK, CONNECT, FAILED)` | Destination, connection run, authentication rules | SUCCESS reports established tested connection. UNAVAILABLE/TIMEOUT: do not claim a secure connection. |
| UC-36: Connection lost | Connector → Monitor | Observation without requirement → `(NETWORK, DISCONNECT, SUCCESS)`, `(NETWORK, DISCONNECT, FAILED)` | connection instance, observation time | SUCCESS reports complete disconnection; monitor decides on Reconnect. Old disconnection is not confused with new connection. |
| UC-37: Lebenszeichen | Service → Monitor | Observation without requirement → `(SYSTEM, UPDATE, SUCCESS)`, `(SYSTEM, UPDATE, FAILED)` | Fixed heartbeat profile, instance, observation time/Sequenz | UPDATE/SUCCESS is only a sign of life according to the agreed profile. Monitor generates TIMEOUT itself; missing message does nothing in the broker. |
| UC-38: Check readiness | Monitor → Service | `(SYSTEM, VALIDATE, PENDING)` → `(SYSTEM, VALIDATE, SUCCESS)`, `(SYSTEM, VALIDATE, FAILED)` | Instance, standby profile, test time | SUCCESS meets specific readiness condition. FAILED/UNAVAILABLE: leave dependent starts blocked. |

### data

| UC | Processing/Constituents | request → messages | Kontext | Recipient rule / error case |
| --- | --- | --- | --- | --- |
| UC-39: Save backup | Application of Backup Worker | `(STORAGE, SAVE, PENDING)` → `(STORAGE, SAVE, SUCCESS)`, `(STORAGE, SAVE, FAILED)` | Run, source level, destination and safety profile | SUCCESS reports saved backup status. PARTIAL/FAILED: Do not mark backup as fully usable. |
| UC-40: Restore backup | Operator → Restore-Worker | `(STORAGE, LOAD, PENDING)` → `(STORAGE, LOAD, SUCCESS)`, `(STORAGE, LOAD, FAILED)` | Run, unchangeable backup reference, destination | SUCCESS reports restored state by profile. partial result is not an overall success; explicitly check re-execution. |
| UC-41: Cache disabled | Data source → Cache application | `(CACHE, DELETE, PENDING)` → `(CACHE, DELETE, SUCCESS)`, `(CACHE, DELETE, FAILED)` | Key/object in the topic or free payload, revision | SUCCESS actually reports entfernten/invalidierten entry. detect missed revision about relevant state. |
| UC-42: Changing configuration | Operator → application | `(RESOURCE, UPDATE, PENDING)` → `(RESOURCE, UPDATE, SUCCESS)`, `(RESOURCE, UPDATE, FAILED)` | Configuration object, revision, validated content | SUCCESS reports applied status defined in the profile. Mynofy start configuration is not automatically reloaded as a result. |
| UC-43: Artifact ready | Builddienst → Human/Agent | Observation without requirement → `(RESOURCE, CREATE, SUCCESS)`, `(RESOURCE, CREATE, FAILED)` | Run, artifact status, external reference | CREATE/SUCCESS informs about available artifact. re-check altered or missing artifact; no file transfer. |
| UC-44: Data set modified | Application → Dependent service | Observation without requirement → `(DATABASE, UPDATE, SUCCESS)`, `(DATABASE, UPDATE, FAILED)` | Objekt, Revision, Schema-/change profile | UPDATE/SUCCESS triggers a reload only in the appropriate profile. No atomic DB transaction plus publish guaranteed. |

### Koordination

| UC | Processing/Constituents | request → messages | Kontext | Recipient rule / error case |
| --- | --- | --- | --- | --- |
| UC-45: Checking the key | Operator/Agent → Policy-Service | `(SECURITY, VALIDATE, PENDING)` → `(SECURITY, VALIDATE, SUCCESS)`, `(SECURITY, VALIDATE, FAILED)` | Key reference without secret, owner, test time | SUCCESS reports positive test result. Entzug/workflow can occur afterwards; current service test remains relevant. |
| UC-46: Reserve resource | Worker ↔ Koordinator | `(RESOURCE, ACQUIRE, PENDING)` → `(RESOURCE, ACQUIRE, SUCCESS)`, `(RESOURCE, ACQUIRE, FAILED)` | Resource, barrel, owner and, if applicable, external lease generation | SUCCESS use only from a reliable allocation authority as a reservation. Broker does not offer a lease; do not use expired reserve. |
| UC-47: Release resource | Inhaber → Koordinator | `(RESOURCE, RELEASE, PENDING)` → `(RESOURCE, RELEASE, SUCCESS)`, `(RESOURCE, RELEASE, FAILED)` | Resource, run, valid Vergabeinstanz/Generation | SUCCESS confirms release in the external coordinator. Obsolete release must not resolve a new reservation. |
| UC-48: Asking group for decision | application → mehrere Menschen/Agenten | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Run, stand, number of participants, quorum rule | Coordinator collects suitable decisions and decides according to the rule, delivery counters are neither the number of participants nor the quorum. |
| UC-49: Waiting for results | Agent/application → Agent/application | `(JOB, COMPLETE, PENDING)` → `(JOB, COMPLETE, SUCCESS)`, `(JOB, COMPLETE, FAILED)` | Run, test, required work steps | Only matching COMPLETE/SUCCESS unlocks the agreed follow-up step. Timeout/workflow/connection loss leaves output unclear. |
| UC-50: Connecting Environments | Device in LAN → Local Agent → Central Service → Human | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Owner per topic, run, stand, roles and direct connections | Professional semantics remains the same; each step checks its context. Service tunnels remain design; currently direct owner connections are needed. |

<a id="ausformulierter-ablauf-code-und-review"></a>

## Formulated process: Code and Review

The application uses three exact topics per run, for example `workflow/4711/communication` for
task, tuning and human decisions, `workflow/4711/status` for Job-/step reports and
`workflow/4711/interpretations` for regularly renewed client rules. It provides all three
together with ACL. Agents can agree on a suitable application protocol in the communication
topic; the human can participate in it. Details and open implementation questions are in the
[Three-topic coordination](agent-coordination.md).

1. The application publishes the work order. Ziel/Commit/instruction as free bytes if necessary; a
   small typed trigger is only sufficient for already known context.
2. Codex polls the communication topic and processes the order. Codex reports acceptance with an application event.
3. Codex publishes the finished message and the actual artifact status. The event may not be
   interpreted as a new review request if the topic does not expressly carry this meaning.
4. Claude observes the agreed review trigger in communication or status and reports either a
   technical decision or technical review error. The review profile must distinguish these
   cases; a failed review does not meet the release condition. A comment can be a separate free
   message; required content is not atomically coupled to a second message. If they must belong
   together, use a single free payload.
5. Codex awaits the decision on the correct run and artifact status. An old release or
   verlorene/abgelaufene decision must not release a new execution. Timeout leaves the output
   unclear; a producer decides himself about query or new attempt.

There are no Reply fields, Read Status Guarantees, Automatic Repeat or atomic linking of processing and
Response introduced; for multiple competing workers, UC-02 and UC-46 apply; the
[workflow model](workflow-semantics.md) describes the limits of assignment.

<a id="bewertung-aller-kategorien"></a>

## Assessment of all categories

| Gruppe | Vorhandene Kategorien | Empfehlung |
| --- | --- | --- |
| Cross-product processes | `SYSTEM`, `JOB`, `SCHEDULER`, `RESOURCE`, `NETWORK`, `EVENT`, `MESSAGE`, `METRICS`, `TELEMETRY`, `SECURITY` | Keep. Define differences EVENT/MESSAGE and METRICS/TELEMETRY precisely; do not categorize the same message multiple times. |
| Infrastructure and diagnosis | `CORE`, `ENGINE`, `MEMORY`, `THREAD`, `FILESYSTEM`, `STORAGE`, `DATABASE`, `CACHE`, `RPC`, `HTTP`, `LOCK`, `MUTEX`, `ATOMIC`, `SERIALIZATION`, `DESERIALIZATION`, `LOG`, `DEBUG` | Useful for machines and operational diagnostics. Before a cut, clarify how much low-level diagnosis the basic catalog should offer. |
| Specific domains | `ASSET`, `BUNDLE`, `RENDER`, `SHADER`, `TEXTURE`, `GPU`, `AUDIO`, `INPUT`, `ANIMATION`, `SCENE`, `ADDRESSABLES`, `STREAMING`, `GAMEPLAY`, `PLAYER`, `AI`, `PHYSICS` | Mark as candidates for later optional domain catalogs, keep for now. `GPU` / `STREAMING` are also useful outside of games. `AI` denotes a domain, not the identity of an agent; an agent order can simply be JOB. |
| Entfernt | `UNITY` (200), `UNITY_OBJECT` (201), `UNITY_SCENE` (202), `UNITY_ASSET` (203) | Removed from the built-in design. IDs remain reserviert/unbesetzt, no renumbering of other values. |

Result: 43 remaining categories, 49 unchanged actions and 21 unchanged status values. The
original import checksums remain as proof of origin. Version 1 is still a changeable design;
clients should retrieve it after a service update. Once a stable version has been released, its
meanings may not be changed under the same version.

<a id="aktionen-und-status-wo-die-basis-reicht"></a>

## Actions and status: where the base extends

Die CRUD -Aktionen, `START` / `STOP`, `EXECUTE`, `SCHEDULE`, `COMPLETE`, `CANCEL`, `CONNECT` /
`DISCONNECT`, `QUEUE` / `DEQUEUE`, `VALIDATE`, `QUERY` and `LOOKUP` cover many processes.
Resource and synchronization actions such as `ALLOCATE`, `MAP`, `LOCK`, `ACQUIRE` and their
counterparts remain useful for diagnosis, but do not offer appropriate broker functions. `SEND`
/ `RECEIVE` describe communication; they do not replace a technical description of the content.

In the case of status, `SUCCESS`, `FAILED`, `TIMEOUT`, `CANCELLED`, `PARTIAL`, `SKIPPED` and
`DEFERRED` usable as soon as it is clear to which operation they refer. `INVALID_ARGUMENT`,
`NOT_FOUND`, `ALREADY_EXISTS`, `BUSY`, `LOCKED`, `CONFLICT`, `UNAVAILABLE`,
`RESOURCE_EXHAUSTED`, `PERMISSION_DENIED`, `NOT_IMPLEMENTED` and `CORRUPTED` cover technical
reasons well. `UNSPECIFIED` is valid, but not a solution for an unclear event significance.

`PENDING` now uniformly means "this action is pending".Whether a job is in a queue or a worker has taken over, is reported by `QUEUE/SUCCESS` or `ACQUIRE/SUCCESS`. `RETRY` does not start a repetition. `SUCCESS` means success of exactly the named action in the agreed profile, no general approval.

<a id="abgleich-nach-dem-vereinheitlichten-ablaufmodell"></a>

## Reconciliation under the standardised operational model

The earlier recommendation of additional generic statuses `ACCEPTED` / `RUNNING` and a
REQUEST/EVENT-Fieldes is overtaken by the decision for action plus status. For basic processes,
existing actions are sufficient: transfer via ACQUIRE, job started via START, step executed via
EXECUTE and total completion via COMPLETE. `PENDING` is always related to the named action.
Separate steptopics and profiles distinguish e.g. tests, review and human release without
automatically introducing new global enums.

The actual gaps are especially in the case of fixed event signatures: Which parameters mean
what, which roles may generate which messages and which specific prerequisites unlock which
follow-up action? Test errors and technically negative results can both prevent approval; the
diagnosis can be made via existing technical error status or a free explanation.
`VALIDATE/FAILED` must not be declared incorrectly as a “code” without context.

For human knowledge, alarm, heartbeat and progress, existing triples are sufficient as a basis
for explicitly named profiles. Whether additional actions improve comprehensibility is later
inferred from these profiles; they are not added here. A universal executable automaton or any
secure task delegation cannot be inferred from the enums alone.

<a id="grenzen-des-kleinen-parametersatzes"></a>

## Limits of the small parameter set

Three primitive values with a maximum of 24 pure value bytes are often sufficient for bool
states, counters and locally known numerical IDs. The field meaning must come from an immutable
signature, such as progress `(processed: U64, total: U64)`; this signature is a suggestion and
not yet installed.

Not completely covered are free task descriptions, review comments, variable references,
UUID-/Hashwerte and several associated measured values with an additional
Zeit-/Einheiteninformation. The current basic type set does not contain Byte-/String-/UUID-Type
for small events. For this purpose, first use the free variant or put the context into the topic
agreed per run. Numerical abbreviations without agreed assignment are not a loss-free solution.
In particular, message ID and cursor do not replace a long-term unique
workflow and artifact identity.

A common case is the sending of a trigger together with small text. A typed message plus
separate free message would have no atomic togetherness. If both are required together, the
single free payload can contain both; an optional combination of both variants would be a
separate, not yet decided extension.

<a id="was-sich-durch-den-katalog-nicht-lösen-lässt"></a>

## What cannot be solved by the catalog

- No exclusive job assignment or at-most-once effect through an enum value.
- No stored current device status and no history solely by temporary messages.
- No safe execution of any actions by Publish right alone. recipients check the professional
  assignment and its execution rules themselves.
- No broker proof that a human read a message and no inferred recipient membership from polling
  counters alone.
- No authenticity of an alleged origin behind a gateway solely by its catalog values.
- No automatic timer action, offline message, escalation or distributed transaction.

These limits are part of the application or require a later explicitly modelled function, and
the core remains a slim, limited communication layer.

The executable role rules are in the [Worker-Skill](../skills/mynofy/SKILL.md) and
[Operator-Skill](../skills/mynofy-operator/SKILL.md).
