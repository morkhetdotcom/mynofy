# mynofy Bash-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Shell-Bibliothek für die vorhandene Rust-CLI. Bash ab 3.2, `mynofy`, `jq` und die
üblichen Unix-Werkzeuge (`head`, `base64`, `tr`, `mktemp`, `cat`, `rm`). Keine neue
Rust-Abhängigkeit, kein eigener WebSocket-/Protobuf- oder Kryptografie-Code.

## Einrichten

```bash
source /absoluter/pfad/mynofy/sdks/bash/mynofy.sh
MYNOFY_BIN=/absoluter/pfad/mynofy/target/debug/mynofy
MYNOFY_OWNER=local
MYNOFY_KEY=/absoluter/pfad/mynofy/.local/unix/client.pk8
MYNOFY_SOCKET=/absoluter/pfad/mynofy/.local/unix/run/mynofy.sock
```

Die Variablen müssen nicht exportiert sein. Der Public Key und die Topic-Rechte
müssen beim Owner registriert sein. Bei Unix Client und Service als denselben
normalen OS-Benutzer ausführen. Die CLI prüft Pfad, Modus und Peer-UID.

Für WSS statt `MYNOFY_SOCKET` die Variablen `MYNOFY_URL=wss://HOST:PORT` und
`MYNOFY_CA=/pfad/ca.pem` setzen. Die CLI verwendet TLS 1.3 und prüft Zertifikat,
Hostname und Owner-Identität. Keine unsichere Option oder Klartext-Fallback.
Nach Rotation optional `MYNOFY_CLIENT_ID` auf die ursprüngliche stabile ID setzen.

Für Owner-Tunnel zusätzlich `MYNOFY_VIA_TOPIC`, `MYNOFY_OWNER_CA` und optional
`MYNOFY_OWNER_NAME` setzen. Die äußere CA prüft den Proxy, die Owner-CA den
Endpunkt. Der Rust-Client übernimmt den vollständigen Tunnel und die Owner-Auth.

## Senden, pollen und bestätigen

```bash
mynofy_publish_text local/events 'Review für TASK-001 angefragt'
mynofy_publish_file local/events ./trigger.bin application/octet-stream
mynofy_publish_event local/events 12 42 2 \
    u64:18446744073709551615 i64:-9223372036854775808 bool:false

page=$(mynofy_poll local/events '' 20)
printf '%s\n' "$page"
# Nach Verarbeitung/Übernahme speichern; bei has_more sofort weitere Seite lesen:
cursor=$(printf '%s' "$page" | jq -r '.next_cursor')
mynofy_poll local/events "$cursor" 20
```

Bytes sind auf 4096 begrenzt. Dateien können NUL-Bytes enthalten, Bash-Strings
nicht. Text wird unverändert einschließlich abschließender Zeilenumbrüche kodiert.
Ereignisse verwenden den vorhandenen Katalog 1/v1; IDs und Parameterbedeutung aus
Katalog/Anwendungsvereinbarung entnehmen. Bis zu drei Parameter: `u32`, `u64`,
`i32`, `i64`, `f32`, `f64`, `bool`. Bei Events als letzte Option
`--expires 2030-01-01T00:00:00Z` möglich; ohne Ablauf entscheidet die Topic-Policy.
Float-Werte und Katalog-IDs werden abschließend durch die CLI validiert.

| Funktion | Argumente |
| --- | --- |
| `mynofy_publish_text` | `TOPIC TEXT [EXPIRES]` |
| `mynofy_publish_file` | `TOPIC FILE [CONTENT_TYPE] [EXPIRES]` |
| `mynofy_publish_event` | `TOPIC CATEGORY ACTION STATUS [TYPE:VALUE ...] [--expires TIME]` |
| `mynofy_poll` | `TOPIC [AFTER] [LIMIT]` |
| `mynofy_list_messages` | `[TOPIC] [CURSOR] [LIMIT]` |
| `mynofy_get_message` | `MESSAGE_ID` |
| `mynofy_get_permissions` | `TOPIC` |
| `mynofy_get_catalog` | `[NAME] [VERSION]`, Standard `mynofy 1` |
| `mynofy_keygen` | `NEW_KEY_FILE` |
| `mynofy_rotate` | `NEW_KEY_FILE`, darf noch nicht existieren |
| `mynofy_session` | JSONL-Befehle über stdin |

Leere optionale Filter/Cursor als `''`, Limits 1..100, Standard 20. Beim Paging
eigener Nachrichten Filter **und Seitengröße** beibehalten. The broker keeps no reader state. No automatic
Veröffentlichung, Polling-Schleife oder lokale Fortschrittsdatei im SDK.

Integer werden als geprüfte Dezimalstrings direkt ins CLI-JSON geschrieben und
Antworten unverändert ausgegeben. Keine Bash-Arithmetik oder `jq`-Rundung auf
`u64`/`i64`. Zum weiteren Auslesen ebenfalls einen Parser mit exakten Integern
verwenden: beispielsweise `jq` mit unveränderter Dezimalzahlenerhaltung; ältere
Versionen/Builds können große Zahlen runden. Nicht auf solchen IDs rechnen.

## Rückgaben und Fehler

Erfolg liefert die vollständige CLI-JSON-Antwort auf stdout, Diagnose auf stderr.
Die Bibliothek verändert beim Sourcen keine Shell-Optionen, Traps, IFS oder umask.

| Exitstatus | Bedeutung |
| --- | --- |
| 0 | Erfolgreiche Antwort |
| 10 | Broker-Fehler; vollständiger Error-Frame auf stdout, etwa ACL oder Rate-Limit |
| 20 | CLI-/Transport-/lokaler I/O-Fehler; kein automatischer Retry |
| 21 | Ungültige oder falsch zugeordnete Antwort |
| 64 | Argument-/Konfigurationsfehler oder lokale Payload-Prüfung fehlgeschlagen |
| 129/130/143 | Operation durch HUP/INT/TERM unterbrochen |

Bei 20/21 oder Abbruch kann eine Mutation bereits angenommen worden sein. Erst
mit eigenen Nachrichten und lokalem Zustand abgleichen, nicht blind erneut senden.
Bei `set -e` erwartete Fehler ausdrücklich behandeln:

```bash
if result=$(mynofy_publish_text local/events 'Prüfen'); then
    printf '%s\n' "$result"
else
    status=$?
    printf 'Status %s, Antwort: %s\n' "$status" "$result" >&2
fi
```

Jeder Komfortaufruf öffnet eine neue authentifizierte CLI-Verbindung. Er verwendet
eine private temporäre Antwortdatei (Verzeichnis 0700, Datei 0600), die beim Ende
der Operation gelöscht wird. Bei Signalen an den Operationsprozess wird dessen
CLI-Kind beendet und abgewartet. Der aufrufende Prozess verwaltet seine eigene
Signalbehandlung und Prozessgruppe; die Bibliothek überschreibt dessen Traps nicht.
`SIGKILL` ermöglicht keine Aufräumarbeiten.

Es gibt keine Nebenläufigkeits-Queue: parallele Komfortaufrufe sind unabhängige
Verbindungen. Der Request-ID-Text `bash-sdk` dient der Antwortzuordnung innerhalb
dieser Verbindung und ist **kein** Deduplizierungsschlüssel.

## Langlebiger Zugang

Für mehrere Befehle ohne wiederholten Prozess-/Auth-Aufwand:

```bash
mynofy_session <<'JSONL'
{"version":1,"type":"get_catalog","id":"catalog","name":"mynofy","catalog_version":1}
{"version":1,"type":"poll","id":"poll","topic":"local/events","limit":20}
JSONL
```

Das ist der JSONL-Passthrough der CLI: Protobuf auf dem Netzwerk, lokale JSON-
Ein-/Ausgabe. Broker-Fehler sind Antwortzeilen; CLI-Exitstatus bleibt unverändert.
Die Komfort-Exitcodes und zusätzliche Antwortzuordnungsprüfung gelten hier nicht.
EOF schließt die Verbindung; keine automatische Wiederverbindung oder Wiederholung.
Die CLI hat feste Timeouts pro Verbindungsphase/Austausch, typischerweise zehn
Sekunden, keinen Bash-konfigurierbaren Gesamt-Timeout. Welcome-/Neustart-Metadaten
werden in dieser CLI-Schnittstelle nicht separat ausgegeben. Nach bekanntem
Neustart bewusst ohne Cursor neu lesen; keine automatische Neustarterkennung.

## Schlüsselwechsel

`mynofy_keygen NEW.pk8` erzeugt eine neue private Datei und liefert die Daten zur
Registrierung. `mynofy_rotate NEW.pk8` verwendet den kontrollierten Client-Wechsel:
die CLI erzeugt und speichert den neuen Schlüssel **vor** dem Commit, verweigert
das Überschreiben vorhandener Dateien und behält ihn auch bei unklarem Ausgang.
Nach Erfolg selbst `MYNOFY_KEY` umstellen und die ursprüngliche Client-ID über
`MYNOFY_CLIENT_ID` erhalten. Bei unklarem Commit beide Dateien behalten.

## Tests

Als normaler Benutzer mit Rust/Cargo, Python 3 (nur Standardbibliothek), jq und
OpenSSL:

```sh
./scripts/test-bash-sdk.sh
# Optional eine andere Bash-Version testen:
MYNOFY_BASH=/pfad/zu/bash ./scripts/test-bash-sdk.sh
```

Echte Unix-/WSS-Dienste, Bytes/Events, Integergrenzen, Cursor, ACL,
Keygen/Rotation, JSONL-Sitzung und TLS-Abweisung. Kontrollierte CLI-Doubles prüfen
Quoting, große Ganzzahlen, falsch zugeordnete/defekte Antworten, Exitcodes,
Signalabbruch und temporäre Dateien. Die Request-Builder nutzen die gemeinsamen
SDK-Vektoren; Wire-/Auth-/Rotationstests bleiben beim delegierten Rust-Protokoll.
Tunnel-Argumente werden geprüft; echte Mehrhop-Tunneltests liegen bei Rust/CLI.
Auf Linux mit Bash 3.2 und 5.2 geprüft. Native macOS-Prüfung bleibt für die CI
offen. Weitere Schritte: [SDK-Übersicht](../../docs/sdks.md).

## Owner-authenticated producer identity

Read `.items[].message.producer_id` in the JSON result for the stable client identity stamped by the owner.
It is immutable, survives authorized key rotation and storage restore, and is
separate from any sender or actor claim inside the payload. Match workflow roles
to this value. This is an assertion over the protected owner connection, not a
portable cryptographic message signature.

Early broker rate limits may omit `request_id` because rejection happens before
request parsing. With `rate_limited` and a valid integer `retry_after_ms`, Bash
preserves the response and returns exit 10. Wrong IDs and other missing-ID errors
still return 21. The SDK never retries a mutation automatically.
