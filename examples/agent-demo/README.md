# Lokales Operator-Demo (Linux/macOS)

Voraussetzungen: Rust/Cargo, Python 3 und drei Agentensitzungen mit lokaler
Dateifreigabe und MCP- oder CLI-Zugang. Alle Prozesse unter demselben normalen
OS-Benutzer starten; der Unix-Socket akzeptiert keine fremde UID. Nicht als root.

Im Repository:

```sh
# Einmalig bauen, Schlüssel/ACLs/YAML und Agenten-Anweisungen erzeugen:
./scripts/agent-demo.sh setup

# Terminal 1: Service im Vordergrund starten (auch für spätere Neustarts):
./scripts/agent-demo.sh start
```

In Terminal 2 alle Topics mitlesen:

```sh
./scripts/agent-demo.sh watch
```

Der Beobachter hat einen eigenen Schlüssel mit ausschließlich Leserechten. Er
zeigt beim Start alle noch aktiven Nachrichten, danach neue. Er bestätigt keine
Nachrichten fachlich. Nach Service-Neustart den Beobachter neu starten; damit
beginnt er bewusst erneut beim aktiven Bestand. Standard-Pollintervall 1 Sekunde,
mit `watch --interval 2` änderbar. Worker-/Operator-Intervalle sind davon unabhängig.

Die generierte Datei **`.local/agent-demo/AGENT-COMMANDS.md`** enthält absolute
MCP-Befehle und kopierbare Anweisungen für Operator und beide Worker sowie die
menschlichen Start-/Bestätigungsbefehle. Den gemeinsamen Text und den jeweiligen
Rollentext an jede Sitzung geben. Das Projekt ist nur eine kleine Python-Funktion
mit Tests und Review; keine Änderungen an mynofy.

In Terminal 3 mit dem Menschentopic kommunizieren:

```sh
./scripts/agent-demo.sh human 'Meine Anweisung'
# Oder interaktiv, eine Nachricht je Zeile; Ctrl-D beendet den Client:
./scripts/agent-demo.sh human
```

Rechte: Mensch schreibt überall, Operator in `demo/operator` und `demo/agents`,
Worker nur in `demo/agents`. Alle lesen alle drei Topics. Verbindliche menschliche
Anweisungen ausschließlich in `demo/human`.

## Persistenz und Beenden

Alle drei Topics haben `storage.mode: interval`, `seconds: 1`. Inhalt und
Security liegen getrennt unter `.local/agent-demo/content` bzw. `security`.
Nachrichten sind unverschlüsselt lokal gespeichert, standardmäßig 24 Stunden
aktiv (Ablauf läuft auch während Service-Stillstand weiter). Das Setup verändert
vorhandene Daten nicht und verweigert eine zweite Einrichtung am gleichen Pfad.
Für Neustarts nur `start` verwenden. Topic-Konfiguration liegt in `config.yaml`.

Vor dem Service-Ende einen gemeinsamen Save aller Topics auslösen:

```sh
./scripts/agent-demo.sh save
./scripts/agent-demo.sh status
```

Warten, bis `content_storage.save_in_progress` false und `dirty` false sind und
`last_error` null ist. Währenddessen keine neuen Nachrichten erzeugen. Erst dann
Terminal 1 mit Ctrl-C beenden. Publish bestätigt zunächst RAM; ein harter Abbruch
kann noch nicht gespeicherte Änderungen verlieren. Ein regulärer Shutdown wartet
auf angenommene Saves, startet aber selbst keinen zusätzlichen Save.

Die Topic-Persistenz ersetzt nicht die lokalen Arbeitsstände der Agenten unter
`agent-state/`. Das Skript führt selbst keine Tickets oder Workflows.

Optional vor **jedem** Befehl denselben absoluten `MYNOFY_DEMO_DIR` setzen, um ein
anderes privates Zustandsverzeichnis zu verwenden. `MYNOFY_BIN` wählt ein eigenes
Binary; beim Standard-Setup wird `cargo build --locked -p mynofy-cli` ausgeführt.

Der Integrationstest verwendet ein eigenes temporäres Verzeichnis und prüft die
echten Topic-Rechte, Texteingabe, Beobachterausgabe und Save/Neustart:

```sh
cargo build --locked -p mynofy-cli
python3 examples/agent-demo/test_demo.py
```
