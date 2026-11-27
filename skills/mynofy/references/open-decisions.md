<a id="stand-und-verbleibende-umsetzung"></a>

# Status and remaining implementation

The basic rules are decided in [workflow.md](workflow.md): three role topics, operator as main
number transmitter, local states, one-time mandate, reused protocol with coordinated revision
change, state transfer to new workers as well as human stop and resumption.

The [MCP client helper](mcp.md#polling-und-exakte-referenzen) provides paced polling
and local checkpoints. The runtime owns deadlines, execution stops and resumption.
The workflow still defines the status exchanged with other participants.

Automated error tests use deterministic software clients. Additional interfaces
and a possible custom broker wire format are deferred until after v1.0.0.
