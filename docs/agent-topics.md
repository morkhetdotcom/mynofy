# Three role topics

The [shared workflow rules](../skills/mynofy/references/workflow.md#topics-und-autorität)
define the roles and their permissions. The deployment provisions these topics.

- **Human topic.** Humans write authoritative instructions, confirmations,
  changes, revocations, stop and resume requests.
- **Operator topic.** Operators and humans write about mandates, risks,
  handovers and updates for the human.
- **Worker topic.** Workers, operators and humans write introductions,
  protocol proposals, assignments, interpretations, status and results.

Workers and operators read all three topics. Human instructions carry
authority when published in the human topic. The owner enforces the
configured ACLs. Applications interpret mandates, roles and protocol
agreements.

The skills use these three topics. Separately configured adapters can
use their own topics.
