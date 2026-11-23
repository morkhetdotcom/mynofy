# MCP-Zugang für mynofy

`mynofy mcp` stellt einen Tools-only-MCP-Server über stdio bereit. Er verwendet
unverändert die vorhandene Client-Bibliothek mit Keypair-Authentifizierung,
Protobuf und Owner-ACLs. Er startet keinen Broker oder HTTP-Listener und bietet
keine Admin-, Topic-Erstellungs-, Schlüsselregistrierungs- oder Rotations-Tools.
Es werden keine zusätzlichen Registry-Bibliotheken benötigt.

## Start und Host-Konfiguration

Lokaler Daten-Socket:

```sh
mynofy mcp --socket /path/mynofy.sock --key /path/agent.pk8 --owner local
```

Direkt über WSS:

```sh
mynofy mcp --url wss://host:8092 --ca /path/ca.crt --key /path/agent.pk8 --owner owner
```

Über die [Service-Kette](service-network.md):

```sh
mynofy mcp --url wss://127.0.0.1:8090 --ca /path/proxy-ca.crt \
  --key /path/agent.pk8 --owner owner --via-topic team/events \
  --owner-ca /path/owner-ca.crt
```

Für einen Unix-Einstieg ebenfalls `--socket` zusammen mit `--via-topic` und
`--owner-ca` verwenden. Optional sind `--owner-name` für einen abweichenden
Owner-Zertifikatsnamen und `--client-id` für eine bestehende stabile Identität.
Zugangsdaten werden beim Prozessstart bereitgestellt, nicht als Tool-Argumente.
Der bisherige explizite UID-/GID-Zwang bei privilegiertem CLI-Start gilt weiter.

Ein MCP-Host startet das Binary als Unterprozess mit command/args, zum Beispiel:

```json
{
  "command": "/absolute/path/to/mynofy",
  "args": ["mcp", "--socket", "/path/mynofy.sock", "--key", "/path/agent.pk8", "--owner", "local"]
}
```

Die Einbettung dieses Eintrags richtet sich nach dem verwendeten Host. Es wird
keine globale Host-Konfiguration automatisch verändert. stdout enthält nur
JSON-RPC-Zeilen; Diagnose geht nach stderr. Schließen von stdin beendet den
Adapter und bricht einen noch laufenden Aufruf ab.

## Verfügbare Tools

| Tool | Aufgabe |
| --- | --- |
| `mynofy_connection_info` | Bei Bedarf anmelden; Owner, stabile Client-ID und Neustartzähler melden. Bereits geladene Sitzungsdaten sind kein eigener Liveness-Nachweis. |
| `mynofy_publish` | Eine Nachricht mit vorhandener Payload-Syntax und optionalem `expires.time` veröffentlichen. |
| `mynofy_poll` | Eine Seite sofort lesen; `topic`, optional `after` und `limit` 1–100. |
| `mynofy_list_messages` | Eigene aktive ursprüngliche Nachrichten und verfügbare Statusdaten auflisten. |
| `mynofy_get_message` | Eine eigene aktive ursprüngliche Nachricht lesen. |
| `mynofy_get_permissions` | Aktuelle eigene Rechte auf einem exakten Topic abfragen. |
| `mynofy_get_catalog` | Eingebauten Katalog abfragen; Defaults `name: mynofy`, `catalog_version: 1`. |

Das Toolset ist fest und wird deterministisch aufgelistet. Nicht unterstützte
Tool-Namen, zusätzliche Argumente und unzulässige Werte werden vor einem
Broker-Aufruf abgewiesen. Der Owner bleibt für tatsächliche ACL-, Quoten-,
Katalog- und Ablaufentscheidungen maßgeblich. Insbesondere erteilt der Adapter
keine Schreibrechte auf das geschützte Menschentopic.

Erfolgreiche Broker-Antworten erscheinen als `structuredContent` und zusätzlich
als JSON-Text in `content`. Der Datenumschlag enthält `owner_id`, `client_id`,
`restart_counter` und das ursprüngliche mynofy-Objekt unter `response`.
Brokerfehler werden als Tool-Ergebnis mit `isError: true` geliefert; fehlerhafte
MCP-/Tool-Argumente als JSON-RPC-Fehler. Nachrichteninhalte sind Anwendungsdaten,
keine zusätzlichen Tool-Berechtigungen.

## Exakte 64-Bit-Werte

ACK/NACK tools and delivery IDs have been removed. Application acceptance and rejection are published events. Poll is read-only.

Typisierte Eventparameter `u64` und `i64` werden in MCP-Ergebnissen ebenfalls als
Dezimalstrings dargestellt. Publish akzeptiert dafür kanonische Dezimalstrings
und wandelt sie vor dem Protobuf-Aufruf verlustfrei zurück. Große numerische
64-Bit-Eingaben werden abgewiesen; normale kleine Zahlen bleiben zulässig.
Beliebige Legacy-JSON-Payloads werden dabei nicht rekursiv umgeschrieben; für
beliebige bytegenaue Daten steht weiterhin die Bytes-/Base64-Variante bereit.

Beispiele für Tool-Argumente:

```json
{"topic":"project/work","payload":"TASK-001: Code zur Prüfung bereit"}
```


## MCP-Protokoll und Betriebsgrenzen

Unterstützt werden die Revisionen 2026-07-28 mit Metadaten pro Anfrage und
`server/discover` sowie 2025-11-25 mit `initialize` und
`notifications/initialized`. Grundlage sind die offiziellen Spezifikationen zu
[Versionierung](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning),
[stdio](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio),
[Discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)
und dem [älteren Lebenszyklus](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

Bei modernen Anfragen sind `params._meta["io.modelcontextprotocol/protocolVersion"]`
und `params._meta["io.modelcontextprotocol/clientCapabilities"]` in jeder Anfrage
erforderlich. Erfolgreiche moderne Ergebnisse tragen `resultType: complete` und
Server-Information unter `_meta`. Eine moderne Anfrage übernimmt keine
Protokoll-Metadaten aus vorherigen Aufrufen. Nur der ältere Modus nutzt den
expliziten Initialisierungsschritt. MCP-Clientinformationen sind keine
mynofy-Clientidentität; die Authentifizierung nutzt allein die beim Start
bereitgestellten mynofy-Zugangsdaten.

Discovery, Ping und Tool-Liste funktionieren ohne erreichbaren Broker. Die erste
fachliche Tool-Anfrage stellt bei Bedarf die Owner-Verbindung her. Nach einem
Transportfehler wird sie verworfen; ein späterer expliziter Aufruf kann neu
verbinden. Die fehlgeschlagene Operation wird dabei **nicht wiederholt**. Bei
verlorener Antwort einer Mutation meldet der Adapter `outcome_unknown: true`.
Auch Cancellation und Prozessabbruch rollen einen bereits angenommenen Publish
nicht zurück. Der Aufrufer muss den Zustand vor einem neuen Versuch prüfen.

Pro Prozess läuft höchstens eine Owner-Operation zugleich; weitere fachliche
Aufrufe erhalten ein `busy`-Tool-Ergebnis und werden nicht eingereiht. Metadaten-
Requests bleiben währenddessen bedienbar. `notifications/cancelled` beendet den
betroffenen lokalen Aufruf, schließt dessen Verbindung und unterdrückt die
anschließende Antwort. EOF oder SIGINT/SIGTERM lässt keine abgetrennte
Owner-Operation im MCP-Prozess weiterlaufen.

Grenzen: eine JSON-RPC-Zeile bis 64 KiB, begrenzte Eingangsqueue, Antwort bis
16 MiB und Schreibdeadline auf stdout. Überlange Zeilen werden vollständig
verworfen, sodass die nächste Anfrage weiterhin korrekt gerahmt ist. Die
bestehenden technischen Verbindungsdeadlines bleiben gültig; sie sind keine
fachliche Wartefrist für den Agenten-Workflow.

Der Adapter erzeugt keine Poll-Schleife, Interpretationserneuerung, Ticketvergabe,
automatischen ACKs, gespeicherten Business-Fortschritt oder Workflow-Automatik.
Cursor und Neustartzähler liefert er an den Aufrufer; `after` ist explizit.
Poll reads active content without creating message/client state and is annotated read-only. Die fachlichen Regeln des [Skill-Entwurfs](../skills/mynofy/SKILL.md)
bleiben davon getrennt. Die [Operator-Regeln](../skills/mynofy-operator/SKILL.md) beschreiben die menschliche Koordination.

## Verifikation

Unit-Tests prüfen Protokollmodi, Metadaten, strikte Argumente, Nichtausführung von
Tool-Notifications, verlustfreie 64-Bit-Werte und begrenztes Framing. Echte
CLI-/Broker-Tests prüfen alle sieben Tools, ACL-Verweigerung, zustandsloses Polling,
Offline-Discovery, parallele Metadaten/Busy, Cancellation, EOF und einen bereits
angenommenen Publish mit absichtlich verlorener Antwort ohne Retry. Zusätzlich
ist der MCP-Zugriff über die Drei-Dienste-Owner-TLS-Kette getestet.

```sh
cargo test --locked -p mynofy-cli mcp
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Eine konkrete Chat-/MCP-Host-Anwendung wurde nicht verbunden und keine
Netzwerk-Performancemessung vorgenommen. Die Tests nutzen einen eigenen
JSON-RPC-Testclient auf dem echten stdio-Transport und vorhandene Broker-/SDK-Pfade.

## Payload typing and process shutdown

Integer-to-string conversion uses the original Protobuf event variant. Legacy
JSON Data stays opaque even when it contains `kind: event` or event-shaped
parameters. MCP shutdown completes active-operation cancellation before bounded
CLI runtime shutdown, so SIGTERM can terminate the process while stdin stays open.
See the [key rotation guide](key-rotation.md).
