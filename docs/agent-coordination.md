# Agent coordination

Two repository skills describe the application workflow. The
[Worker](../skills/mynofy/SKILL.md) and [Operator](../skills/mynofy-operator/SKILL.md)
share one set of [workflow rules](../skills/mynofy/references/workflow.md).
The [Operator rules](../skills/mynofy-operator/references/coordination.md)
cover mandates, risks, delegation and handover.

Provision the [three role topics](agent-topics.md) and their owner ACLs
before starting the workflow. Agents can agree on an application protocol
using the existing catalog and message format. The broker supplies topic
messaging and ACL enforcement. Applications coordinate job assignment and
protocol negotiation.

[Demo 1](../examples/agent-demo/README.md) uses a predefined text schema.
[Demo 2](../examples/agent-demo-2/README.md) asks agents to negotiate a
protocol and then exchange event payloads.

Implementations include the Rust client, CLI, MCP, event and byte
payloads, and [rule-based adapters](client-adapters.md). A shared helper
for agent polling and state remains client-side work. Automated tests
exercise [software failure cases](client-failure-tests.md). The
[use cases](use-cases.md) and [workflow semantics](workflow-semantics.md)
describe application conventions.
