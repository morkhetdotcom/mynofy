# Stand und verbleibende Umsetzung

Die Grundregeln sind in [workflow.md](workflow.md) entschieden: drei Rollen-Topics,
Operator als Hauptnummerngeber, lokale Zustände, einmaliges Mandat, wiederverwendetes
Protokoll mit abgestimmtem Revisionswechsel, Zustandstransfer an neue Worker sowie
menschlicher Stopp und Wiederaufnahme. Hierfür ist keine weitere Grundsatzrunde offen.

Noch umzusetzen bei der Client-Anpassung:

- Gemeinsame lokale Ablagemechanik für Arbeitsstand und Übergabe.
- Polling-/Nachrichtenübergabe, Interpretationstimer, Fristen und Abbruch als
  technische Unterstützung; Subprozesse sind erlaubt, ein gemeinsamer Helfer fehlt.
- Unterstützung des beim Start vorgegebenen Stopp-/Wiederaufnahmemodus durch die
  konkrete Laufzeit. Es wird kein bereits vorhandener Supervisor vorausgesetzt.

MCP und CLI sind implementiert. Die Rollen wurden in zwei Demos praktisch benutzt;
Bedienung und Grenzen stehen im [MCP-Leitfaden](../../../docs/mcp.md) und im [Adapter-Leitfaden](../../../docs/client-adapters.md). Automatisierte
Fehlertests verwenden deterministische Softwareclients, keine allgemeine Agenten-
Verhaltensprüfung. Ein eigenes Broker-Nachrichtenformat bleibt spätere Iteration.
