# Deterministische Client-Fehlertests

```sh
./scripts/test-client-failures.sh
```

Der Lauf startet eigene lokale Testdienste und Testclients in temporären
Verzeichnissen. Keine LLM, keine laufende Demo und keine neuen Abhängigkeiten
sind nötig. Getestet werden Softwareverhalten und Fehlergrenzen, nicht die
Befolgung eines Skills durch ein Modell. Unter Linux/macOS; ein privilegierter
Testprozess führt die Szenarien unter der vorhandenen unprivilegierten
Testidentität aus, ohne die Root-Startregel zu lockern.

## Neue Fälle

`crates/mynofy-cli/tests/failure_clients.rs` prüft über echte Unix-/WebSocket-
Verbindungen und den vorhandenen Rust-Client:

| Fall | Erwartetes Softwareverhalten |
| --- | --- |
| Reader-Verbindung nach Poll abbrechen | Poll nach Reconnect ohne Cursor liefert die weiterhin aktive Nachricht mit derselben Message-ID. Mit dem gespeicherten Cursor setzt der Consumer beim nächsten Eintrag fort. Dauerhafter fachlicher Fortschritt liegt beim Consumer. |
| Publish mit gleicher Request-ID nach Reconnect erneut senden | Zwei unterschiedliche Message-IDs; eine Request-ID ist kein Idempotenzschlüssel. |
| Ungültige Protobuf-Daten und Payload über 4096 Bytes | Fehler ohne Veröffentlichung; unabhängiger Client kann weiter lesen und schreiben. Der rohe Oversize-Frame umgeht bewusst die SDK-Vorprüfung. |
| ACL während bestehender Sitzung entziehen | Alte Verbindung wird geschlossen, neue Publish/Poll-Aktionen werden verweigert. Eigener ursprünglicher Inhalt bleibt für den authentifizierten Producer lesbar. Andere berechtigte Clients bleiben nutzbar. |
| WebSocket ohne Authentifizierung und authentifizierter Idle-Client | Verbindungen werden nach konfigurierter Frist geschlossen; anschließend funktioniert ein neuer Client. |

Die Tests prüfen konkrete Antworten und Zustände. Warteoperationen besitzen
Test-Deadlines, damit ein ausbleibender Abbruch den Test fehlschlagen lässt.
Die Timeout-Szenarien verwenden kurze konfigurierte Servicefristen; sie sind
keine Performance- oder exakten Timingtests.

## Vorhandene MCP-Fälle im selben Lauf

`crates/mynofy-cli/tests/mcp.rs` ergänzt unter anderem:

- Ein Owner akzeptiert den Publish, schließt aber vor der Antwort: MCP meldet
  `outcome_unknown`; nach Reconnect gibt es genau eine Nachricht und keinen
  automatischen erneuten Publish.
- Cancellation und EOF beenden ausstehende Owner-Arbeit; Metadaten bleiben
  während einer blockierten fachlichen Operation erreichbar.
- Clientoperationen und ACL- und Poll-Semantik über echte stdio-MCP-Prozesse.
- Offline-Discovery und Trennung der unterstützten MCP-Protokollmodi.

Das Skript umfasst derzeit 14 Integrationstests. Es ersetzt weder die
funktionalen Core-/Mutationstests (`scripts/test-core-functional.sh`) noch die
separate Speicher-Abbruchmatrix (`scripts/test-storage-crashes.sh`). Fachliche
Workflow-Entscheidungen, menschlicher Stopp und Mandat bleiben Clientlogik;
der Broker interpretiert ihre Inhalte nicht als eigene Zustandsmaschine.
