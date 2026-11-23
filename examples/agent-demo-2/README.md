# Demo 2: Protokollaushandlung

Wie Demo 1, aber mit eigenem Service, Schlüsseln und persistenten Topics unter
`.local/agent-demo-2/`. Die erste Demo wird nicht verändert. Linux/macOS,
Python 3, Rust und normaler OS-Benutzer; alle Clients unter derselben UID.

```sh
# Einmalig:
./scripts/agent-demo-2.sh setup
# Terminal 1, auch bei späterem Neustart:
./scripts/agent-demo-2.sh start
# Terminal 2:
./scripts/agent-demo-2.sh watch
# Terminal 3, eine Nachricht pro Zeile:
./scripts/agent-demo-2.sh human
```

Die generierte Datei `.local/agent-demo-2/AGENT-COMMANDS.md` enthält die menschlichen
Start-/Bestätigungsbefehle und alle Rollen. Gib stattdessen jeder der drei
Agentensitzungen **ihre eigene vollständige Startdatei**:

- `.local/agent-demo-2/operator-START.md`
- `.local/agent-demo-2/worker-1-START.md`
- `.local/agent-demo-2/worker-2-START.md`

Jede Datei enthält gemeinsamen Ablauf, eindeutige Rolle und passenden MCP-/CLI-
Befehl. MCP vorher im Host bereitstellen; ohne MCP ist die vorhandene CLI zulässig.
Generierte absolute Pfade gelten für die Setup-Umgebung. Wenn der Agent anders
mountet, die Pfade in seinen Startdateien **vorher** passend zuordnen. Keine
Schlüssel anderer Rollen an eine Sitzung geben.

## Was diesmal anders ist

Kein vorgegebenes Semikolon-Schema und keine vorab festgelegte Ereignis-/Parameter-
belegung. Alle drei rufen den vorhandenen Katalog ab. Sie handeln auf dem
Agententopic eine konkrete Version mit Ereignisbedeutung und Zuordnungen aus.
Beide Worker müssen ausdrücklich zustimmen; technisches ACK ist keine Zustimmung.
Maximal zwei Überarbeitungen und zwei Minuten Aushandlung. Keine Einigung bedeutet
Blockade, keinen stillen Rückfall auf ausschließlich Text.

Nach Einigung werden echte typisierte Ereignisse für wiederkehrende Status-/
Lebenszyklusmeldungen verwendet, darunter mindestens eine je Worker und eine
Statusabfrage mit Antwort. Freitext bleibt für Aushandlung, Erklärungen und
Fachdetails zulässig. Nicht vorgegeben ist, welche konkrete Ereignisabbildung
sie wählen. Das Broker-Protokoll wird nicht verändert.

Das Mandat wird einmal bestätigt und gilt bis Widerruf oder Aufgabenabschluss;
es gibt keine Laufzeit und keine periodische Erneuerung. Änderungen benötigen
weiterhin menschliche Bestätigung. Anforderungen, Zuweisungen und Abschluss
dürfen ohne ergänzenden Text im vereinbarten Protokoll erfolgen.

Die kurze Fachaufgabe kommt diesmal über den Operator: Zeitformatierungsfunktion,
kleine Tests und unabhängiges Review. Worker-Startdateien liefern die Lösung oder
konkreten Anforderungen nicht vorab. Zielgröße 5–10 Minuten.

## Zustand und Auswertung

Topics: `demo2/human`, `demo2/operator`, `demo2/agents`; Owner `agent-demo-2`.
Rechte und Persistenz wie Demo 1: alle lesen, nur Mensch schreibt ins Menschentopic,
Operator schreibt in Operator-/Agententopic, Worker nur ins Agententopic.
Inhalte werden sekündlich bei Änderungen gesichert und standardmäßig 24 Stunden
aufbewahrt; lokaler Agentenzustand bleibt separat. Beobachter zeigt Text und echte
Event-Payloads als JSON. Nach Neustart Beobachter neu starten.

Für die spätere Auswertung Zustand, Verhandlungsnachrichten und endgültige
Vereinbarung erhalten. Der Operator hält Revision, Zustimmungsnachrichten und
Event-Nachrichten lokal nachvollziehbar fest; es geht um beobachtete Einigung und Anwendung,
nicht um behauptete Performancewerte. Der Testaufbau selbst enthält keine
Simulation der AI-Einigung.

Vor Beenden ohne weitere Publishes:

```sh
./scripts/agent-demo-2.sh save
./scripts/agent-demo-2.sh status
```

Warten auf `save_in_progress: false`, `dirty: false`, `last_error: null`, dann
Service mit Ctrl-C beenden. Ein Publish bestätigt zunächst RAM, keinen Disk-Commit.

Technischer Integrationstest (keine AI-Simulation):

```sh
cargo build --locked -p mynofy-cli
MYNOFY_DEMO_VARIANT=2 python3 examples/agent-demo/test_demo.py
```

`MYNOFY_DEMO_DIR` und `MYNOFY_BIN` funktionieren wie bei Demo 1. Für eigene Pfade
bei allen Befehlen dieselben Umgebungsvariablen verwenden.
