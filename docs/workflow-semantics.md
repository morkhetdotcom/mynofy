<a id="einheitliche-abläufe-für-menschen-agenten-und-maschinen"></a>

# Uniform processes for people, agents and machines

This document specifies the product decision for `(Kategorie, Aktion, Status)`. It is the
technical basis for the [UC-catalog](use-cases.md) and a later AI skill. It does not introduce
new wire fields or broker automatics. Typed events and free bytes are implemented; the core
checks identifiers and sizes, but no technical transitions or parameter meanings. The locally
configured adapter can already evaluate exact events as triggers.

## Grundregel

Category = affected area. Action = specific task. Status = status or result of that task.

| Muster | Meaning | Consignee may derive from this |
| --- | --- | --- |
| `(JOB, START, PENDING)` | The start of this job is pending. | Only the exporter specified in the expiry checks whether it is allowed to take off; observers wait. |
| `(JOB, START, SUCCESS)` | This job was started. | Start has been made; work and result are not yet completed. |
| `(JOB, ACQUIRE, PENDING)` | The takeover is pending. | The awarding authority shall examine the candidate named in the profile. |
| `(JOB, ACQUIRE, SUCCESS)` | The takeover was successfully completed. | The designated exporter has taken over under the contract and not every reader becomes the holder. |
| `(JOB, EXECUTE, PENDING)` | A defined work step is pending. | Competent exporter shall examine the conditions of this step. |
| `(JOB, EXECUTE, SUCCESS)` | The defined work step was successfully carried out. | Only dependencies linked to it can be fulfilled. |
| `(JOB, VALIDATE, SUCCESS)` | The examination in the named profile was successful. | Exactly this examination of this stand has been passed; other examinations remain open. |
| `(JOB, COMPLETE, SUCCESS)` | The job is successfully completed for this attempt. | Agreed result consumers can proceed according to their rules. |
| `(JOB, COMPLETE, FAILED)` | The job failed for this attempt. | No success path; apply error handling of the profile. |
| `(JOB, CANCEL, CANCELLED)` | The termination was actually carried out. | It is no longer just a wish to discontinue, but side effects that have already occurred remain possible. |

Use `ACQUIRE` (ID 72). `SUCCESS` always refers to the
action: a successful CANCEL would be successful termination, not a successful job. for completed
termination, the standard example uses uniformly `CANCEL/CANCELLED`.

`PENDING` is not a universal ‘perform this now’. It can also announce a producer’s own pending work. Only the pre-agreed profile indicates the exporter responsible and its requirements. SUCCESS is a notification of a result that has occurred, not a general request to all consumers. Some notifications, e.g. readings or signs of life, are immediate observations without prior PENDING.

<a id="beispiel-agent-startet-und-übernimmt-arbeit"></a>

## Example: Agent starts and takes over work

A process can look as described by the product:

1. Agent reports `(JOB, START, PENDING)` for its pending start.
2. If his starting requirements are met and he actually starts, he reports `(JOB, START,
   SUCCESS)`.
3. He takes over a certain work step or resource and reports after successful allocation `(JOB,
   ACQUIRE, SUCCESS)`.
4. It performs the protected step and reports `(JOB, EXECUTE, SUCCESS)`.
5. After all necessary steps, he reports `(JOB, COMPLETE, SUCCESS)`.

The order results from the dependencies: If the job start has to be exclusively assigned, the
successful takeover is before START/SUCCESS. If the start only means uncritical initialization,
the resource takeover can take place afterwards and before the protected work. Each specific
profile determines one of these variants; an agent must not reinterpret the order independently.

Mynofy does not currently provide jobs or leases; for a single fixed exporter, its takeover
agreement may be sufficient; for competing exporters, a reliable contracting authority is
required; two self-published ACQUIRE/SUCCESS-messages do not produce atomic exclusivity; a
notification of award does not replace the relevant award.

<a id="kontext-der-ein-tripel-eindeutig-macht"></a>

## Context that makes a triple clear

Each flow profile shall describe at least:

- catalog name and version and its own fixed Meaning/Version; the concrete profile identifier has
  not yet been decided as a wire field.
- Exact topics for the steps and their Schreib-/Leserechte; no implicit wildcards or reply
  fields.
- Run, test, step and, where applicable, unchangeable artifact status; a new test or changed
  status must be distinguishable from old results.
- Responsible producers or authorized applications for each message and which consumers are
  allowed to act.
- Fixed types, order, unity and meaning of the parameters. Suggest for simple jobs: up to three
  U64 values for run, trial and step. If a case requires more context, the profile must already
  know it in the Topic/externen state or use the free payload; do not leave anything silent.
- Requirements, allowed transitions, completion criteria and response to errors, duplicates and
  unknown exits.

This information is the semantics, no additional fields that each message must transmit
repeatedly. A single Category/Action/Status-Kombination without such a profile is not sufficient
for secure interpretation. The catalog retrieval currently contains the values, no installed
workflow profiles or the complete UC list.

Current ACLs can limit writer roles over separate topics; they do not authorize individual
triples within a commonly describable topic. Messages do not currently contain a
consumer-certified individual producer proof. A future skill may not derive execution
authorization from any role claims in the content. For decision-relevant steps, the application
and topic contract must ensure that the authorized producer can write there.

<a id="reaktionsregeln-für-einen-späteren-ai-skill"></a>

## Response rules for a later AI skill

| Eingang / Situation | Reaktion |
| --- | --- |
| Unknown catalog, profile or parameter significance | Do not guess and do not trigger a side effect. Get available definition or clarify configuration. |
| PENDING for your own step | Check the context and current conditions; only then take over/execute according to defined responsibility. |
| PENDING for another exporter | Watch or wait by profile, do not get started additionally. |
| SUCCESS for a requirement | Check the run, test, stand and all other dependencies; only start the step that has been released. |
| SUCCESS to an old Versuch/Artefakt | Do not use as current release. |
| The same professional step will be delivered again | Check your own progress/authoritative state before taking effect again. A replayed message does not mean a new task. |
| FAILED, PERMISSION_DENIED, INVALID_ARGUMENT or other negative output | Blocking the path of success and applying the specific error rule. |
| DEFERRED or BUSY | deferral or assignment report or continue to wait by profile; no unlimited poll or publish loop. |
| RETRY | Interpret as information; only in the case of explicit retry rule and decision of the producer to start a new attempt. |
| Timeout, expiration or disconnection | Own active content/Status query if necessary, check the relevant state and deliberately decide repetition. |
| Poll succeeds | The reply contains active history. Application work, approval and completion are reported separately. |

The RAM history of mynofy and the client cursor do not guarantee permanent deduplication,
current device status and atomic processing and follow-up publish. A later skill must obtain its
allowable actions from the specific process, not from a blanket list of all known action names.

<a id="was-vor-der-skill-erstellung-noch-geprüft-werden-muss"></a>

## What else needs to be checked before skill creation

The 50 UCs are deliberately broader than the first standard profile. The next step is three
fully played profiles: JOB with a fixed executor, Code/Tests/Review with several permanent
agents and human release of a concrete artifact status. Valid and invalid sequences of events
and Timeout-/duplicate cases must be described for each profile.

<a id="gemeinsame-kommunikation-und-lokale-vereinbarungen"></a>

## Joint communication and local agreements

The [Agent coordination with three topics](agent-coordination.md) complements this model: a
communication topic for tuning including human participation, a status topic for compact events
and an interpretation topic for regularly renewed client rules. The catalog is the common basis;
agents are allowed to efficiently agree on their own running-related application protocols.
Broker rules and catalog basic meanings are not changed. Unknown protocol statuses or
unexplained consent are not execution clearance.
