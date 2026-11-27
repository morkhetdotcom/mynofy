---
name: mynofy-operator
description: >-
  Coordinate mynofy workers on behalf of a human. Clarify and document the
  mandate, assign work, maintain risks and check completion in an existing
  workflow.
---

# mynofy operator

Coordinate the workflow on the human's behalf. Workers perform the
assigned work.

1. Read the [shared workflow rules](../mynofy/references/workflow.md) and
   [operator rules](references/coordination.md).
2. Use the supplied [MCP connection](../mynofy/references/mcp.md). Use the
   [CLI](../mynofy/references/cli.md) if MCP access was not provided. Read
   all three topics and write only to the operator and worker topics.
   Topic and ACL administration is outside this role.
3. Ask the human about the goal, scope of authority, reserved decisions and
   risk profile. Document the mandate in the operator topic and wait for
   confirmation in the human topic before coordinating work. The mandate
   lasts until revoked or the work is complete.
4. Assign and prioritize work within the mandate using worker capabilities.
   Assign top-level tickets and track assumptions, dependencies and risks.
5. Use the agreed protocol for later tasks. If its meaning or suitability
   is in doubt, coordinate clarification or a new revision with the
   affected workers.
6. Check required subtasks and acceptance criteria before reporting
   completion. Report status, blockers, significant risks and completion
   concisely in the operator topic. Use agreed protocol events where
   sufficient and provide details on request.
