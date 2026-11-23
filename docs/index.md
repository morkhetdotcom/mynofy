# Documentation

## Start and operate

- [Project overview](../README.md) and [getting started](getting-started.md)
- [Configuration](configuration.md), [YAML](yaml-config.md), and [privilege handling](privileges.md)
- [Networking](networking.md), [service links](service-links.md), [owner tunnels](service-network.md), and [routing](topic-routing.md)
- [Security model](security.md), [client authentication](client-auth.md), and [key rotation](key-rotation.md)
- [Content storage](content-storage.md), [security journal](security-journal.md), and [restart/offline storage tools](restart-and-storage.md)
- [Admin API](admin-api.md), [runtime topics](runtime-topics.md), and [runtime peers](runtime-peers.md)

## Interfaces

- [Protobuf protocol](../protocol/protobuf/mynofy.proto), [protocol guide](protocol.md), and [binary client transport](broker-client-transport.md)
- [CLI JSON schema](../protocol/v1/frame.schema.json) and [service-control schema](../protocol/v1/service-frame.schema.json)
- [Event catalog](event-catalog.md) and [catalog definitions](../protocol/catalogs/mynofy-v1.json)
- [SDK overview](sdks.md): [Python](../sdks/python/README.md), [Go](../sdks/go/README.md), [Java](../sdks/java/README.md), [Bash](../sdks/bash/README.md), [C](../sdks/c/README.md), and [Rust](../crates/mynofy-client)
- [MCP](mcp.md) and [executable adapters](client-adapters.md)
- [Stateless polling](message-client-confirmation.md) and [expiration](budgeted-expiration.md)
- [Shared binary layout](binary-message-layout.md) and [Protobuf generation](protobuf.md)

## Workflows and examples

- [Codex/Claude code-review example](../examples/code-review/README.md)
- [Agent demo](../examples/agent-demo/README.md) and [protocol-negotiation demo](../examples/agent-demo-2/README.md)
- [Agent coordination](agent-coordination.md), [role topics](agent-topics.md), [workflow semantics](workflow-semantics.md), and [use cases](use-cases.md)
- [Worker skill](../skills/mynofy/SKILL.md) and [operator skill](../skills/mynofy-operator/SKILL.md)

## Develop and test

- [Contributing](../CONTRIBUTING.md), [security reports](../SECURITY.md), and [dependencies](dependencies.md)
- [Core functional tests](core-functional-tests.md), [client failure tests](client-failure-tests.md), and [storage interruption tests](storage-interruption-tests.md)

The schemas and source define the interfaces. Earlier language in a technical
guide does not add delivery guarantees beyond the current protocol.
