---
name: mynofy
description: >-
  Participate as a worker in an existing mynofy workflow. Accept assignments,
  coordinate with other agents and report results when a role, connection
  settings and topics have been supplied.
---

# mynofy worker

Perform the assigned work. Take your role and assignment from the startup
instructions. Clarify missing prerequisites before starting.

1. Read the [shared workflow rules](references/workflow.md) for topics,
   authority, protocol, tickets, local progress and stop requests.
2. Use the supplied [MCP connection](references/mcp.md). Use the
   [CLI](references/cli.md) if MCP access was not provided. Use the existing
   topics and your assigned identity. Topic and ACL administration is
   outside the worker role.
3. Choose an unused agent name or use the assigned name. Introduce your
   capabilities in the worker topic. The operator provides the relevant
   workflow state.
4. Check the mandate and agreed protocol. Explicitly accept or decline the
   assignment with an application message. Answer status queries in the
   agreed schema.
5. Report results and outstanding work. Send agreed typed events for
   requests, status and completion when the protocol allows it. Provide
   extensive details on request.

Keep progress in durable local storage. Reconcile replayed messages with
that progress before acting. Honor human stops and risk pauses.

[Implementation status](references/open-decisions.md)
