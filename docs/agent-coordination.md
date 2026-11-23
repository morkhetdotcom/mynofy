# Agentenkoordination: aktueller Stand

Zwei Repo-Skills beschreiben die Anwendungsebene:
[Worker](../skills/mynofy/SKILL.md) und [Operator](../skills/mynofy-operator/SKILL.md).
Die verbindlichen [gemeinsamen Ablaufregeln](../skills/mynofy/references/workflow.md)
werden an einer Stelle gepflegt; [Operatorregeln](../skills/mynofy-operator/references/coordination.md)
ergänzen Mandatsklärung, Risiken, Delegation und Übergabe.

Die [drei Rollen-Topics](agent-topics.md) werden vorab mit Owner-ACLs provisioniert.
Vorhandene Katalogwerte und das Nachrichtenformat sind die gemeinsame technische
Basis; Agenten können darauf ein eigenes Anwendungsprotokoll vereinbaren. Das
ändert weder Wireformat noch ACLs und installiert keine Kataloge im laufenden Service.
Der Broker bietet keine atomare Jobvergabe oder Protokollvereinbarung.

[Demo 1](../examples/agent-demo/README.md) verwendet ein vorgegebenes Textschema.
[Demo 2](../examples/agent-demo-2/README.md) fordert echte Aushandlung und danach
Event-Payloads. Die aktuellen Rollen- und Koordinationsregeln stehen in der gemeinsamen Referenz.


Implementiert sind Rust-Client, CLI, MCP, Events/freie Bytes und
[regelbasierte Adapter](client-adapters.md). Ein gemeinsamer Agenten-Polling-/
Zustandshelfer ist noch ausstehende Client-Arbeit. Automatisierte Tests prüfen
[Softwarefehler mit Testclients](client-failure-tests.md), nicht allgemeine
Modellzuverlässigkeit. Die [UCs](use-cases.md) und [Workflow-Semantik](workflow-semantics.md)
sind weiterführende Anwendungsideen, keine zusätzliche Broker-Zustandsmaschine.
