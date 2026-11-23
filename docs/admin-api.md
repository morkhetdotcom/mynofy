# Lokale Admin-API

Die Verwaltungs-API ist implementiert und verwendet einen eigenen Unix-Socket. Sie verwaltet Identitäten, Schlüssel und ACLs im laufenden Owner. Sie ist weder Teil des Nachrichtenprotokolls noch über Topics, REST oder den WSS-Port erreichbar. Sie funktioniert neben einem WSS-Datenlistener oder einem lokalen Daten-Unix-Socket.

## Basic status and explicit storage diagnostics

```sh
# Routine monitoring: no full message-storage scan.
mynofy admin status --socket .local/unix/run/mynofy-admin.sock

# On-demand record, payload, arena and index statistics.
mynofy admin storage stats --socket .local/unix/run/mynofy-admin.sock
```

`status` reports service identity, client/key/session/topic counts,
`resident_messages`, charged message bytes, limits, security/content persistence
health and service links. `resident_messages` is the constant-time resident record
count, including expired records awaiting normal bounded cleanup. It is not an
exact active-message count. Session validation and configured metadata processing
remain; this is not a constant-time claim for arbitrary metadata sizes.

`storage_stats` scans resident messages to report active/expired counts, payload
bytes, record layout, allocated arena/pages and index sizes. It holds the owner
lock for a consistent view, so use it on demand rather than as a frequent health
check. Both commands are read-only and do not wait for security durability.

Admin JSON request (on the separate protected Unix socket):

```json
{"version":1,"command":{"op":"storage_stats"}}
```

The former `status.result.storage` object moves to `storage_stats.result`;
the CLI prints that result directly. Update scripts that consumed detailed storage
fields to use the explicit command. Routine save/journal monitoring keeps using
`status` and its `content_storage` / `security_storage` fields.

## Aktivieren

In der bestehenden Startkonfiguration unter `service` ergänzen:

```json
"admin_socket": "run/mynofy-admin.sock"
```

Die übrigen Service-Felder für den Datenlistener bleiben erforderlich. Der Admin-Pfad muss vom Daten-Socket getrennt sein. Relative Dateipfade werden beim Laden relativ zum Konfigurationsverzeichnis aufgelöst; Export-Snapshots enthalten aufgelöste absolute Pfade. Ohne `admin_socket` startet kein Admin-Listener. Die Setup-Skripte `setup-local.sh` und `setup-unix.sh` setzen den Pfad bei neu erstellten Demos; vorhandene lokale Konfigurationen werden nicht geändert.

```sh
cargo run --locked -- serve --config .local/unix/config.json
cargo run --locked -- admin status --socket .local/unix/run/mynofy-admin.sock
cargo run --locked -- admin help
```

Der Socket hat Modus `0600`. Neu angelegte Verzeichnisse haben Modus `0700`; ein vorhandenes Elternverzeichnis darf nicht für Gruppe oder andere schreibbar sein. Der Dienst prüft zusätzlich die UID des verbundenen Prozesses gegen die Socket-Owner-UID. Vorhandene Dateien, Symlinks und alte Socket-Einträge werden beim Start nicht überschrieben. Beim Beenden werden nur selbst angelegte, anhand Gerät/Inode identifizierte Socket-Dateien entfernt; Ersatzdateien bleiben erhalten. Ein nicht bindbarer konfigurierter Admin-Socket lässt den Dienststart fehlschlagen.

Die Verwaltungsberechtigung folgt damit der Betriebssystem-Identität: Prozesse unter derselben UID wie der Dienst dürfen administrieren. Es gibt zunächst keine weiteren Admin-Rollen oder Admin-Tokens. Ein normales Client-Keypair oder eine Topic-ACL erteilt keinen Admin-Zugriff. Der CLI-Client prüft Socket-Dateityp, Modus und die UID des verbundenen Servers. Die Implementierung nutzt Unix-APIs unter `cfg(unix)`; die Tests liefen auf Linux, kein nativer macOS-Test wurde durchgeführt.

## Kommandos

Alle Aufrufe benötigen `--socket PATH`; die folgenden Beispiele verwenden einen verkürzten Platzhalter `PATH`.

| CLI | Funktion |
| --- | --- |
| `mynofy admin clients list --socket PATH` | Client-Identitäten mit Name, Sperre, Revision, Schlüsselanzahl und Zahl gültiger Sessions auflisten. |
| `mynofy admin clients get CLIENT --socket PATH` | Public Keys, Token-IDs, Ablauf/Gültigkeit und Sessioninformationen ansehen. |
| `mynofy admin clients add --public-key FILE --socket PATH` | Neue stabile Identität aus dem ersten Public Key registrieren, zunächst ohne Topic-Rechte. |
| `mynofy admin clients disable CLIENT --socket PATH` | Client sperren und dessen Sessions invalidieren. |
| `mynofy admin clients enable CLIENT --socket PATH` | Client wieder zulassen; neue Anmeldung erforderlich. |
| `mynofy admin keys revoke TOKEN --socket PATH` | Einen registrierten Schlüssel widerrufen und dessen Sessions/Challenges invalidieren. |
| `mynofy admin keys add CLIENT --public-key FILE --socket PATH` | Einen vom Betreiber ausdrücklich zugeordneten Schlüssel hinzufügen, etwa zur Neueinrichtung nach Schlüsselverlust. |
| `mynofy admin acl get CLIENT TOPIC --socket PATH` | Rechte eines Clients auf einem exakten lokalen Topic abfragen. |
| `mynofy admin acl set CLIENT TOPIC --allow publish,subscribe --socket PATH` | Das vollständige Rechte-Set für dieses Paar ersetzen. |
| `mynofy admin acl set CLIENT TOPIC --allow none --socket PATH` | Alle Rechte dieses Paars entfernen. |
| `mynofy admin status --socket PATH` | Client-/Session-/Topiczahlen, Ressourcenbelegung, Limits und Bereinigungsrückstand abfragen. |
| `mynofy admin export --socket PATH --out NEW_FILE` | Aktuellen Identitäts-/ACL-Stand als neue Startkonfiguration exportieren. |

`clients list` unterstützt `--limit 1..100` (Standard 20) und `--after SLOT`. Die Antwort enthält `items` und gegebenenfalls `next_after`. Dieser administrative Fortsetzungsindex ist keine Client-ID und kein Topic-Cursor; neue Abfragen nach einem Dienstneustart beginnen ohne `--after`. `clients get` zeigt höchstens 100 Session-IDs sowie Gesamtanzahl und `sessions_truncated`.

Öffentliche Schlüsseldateien enthalten entweder das JSON-Ergebnis von `keygen` oder den rohen Base64-kodierten 32-Byte-Public-Key. PEM/Zertifikate und private Schlüsseldateien sind dafür nicht vorgesehen. Bei JSON wird `public_key` gelesen; eine vorhandene `algorithm`-Angabe muss `ed25519` sein. Die Schlüsselbefehle akzeptieren optional `--valid-until RFC3339_UTC_Z`; ohne Angabe gilt die konfigurierte `rotation.key_validity_seconds` ab Owner-Zeit. Ein Datum muss in der Zukunft liegen. `clients add` unterstützt zusätzlich `--name NAME` mit höchstens 256 UTF-8-Bytes.

```sh
cargo run --locked -- keygen --out review-agent.pk8 > review-agent-public.json
cargo run --locked -- admin clients add \
  --socket .local/unix/run/mynofy-admin.sock \
  --public-key review-agent-public.json --name review-agent
```

Die Antwort liefert die neue `client_id`. Danach dieser Identität die benötigten Rechte geben und den normalen Client mit dessen privater Schlüsseldatei anmelden. Die Registrierung ersetzt nicht den Schlüsselbesitznachweis im normalen Auth-Handshake. Höchstens 10.000 Identitäten und acht registrierte Schlüssel pro Identität sind zulässig. Doppelte Identitäten beziehungsweise bereits registrierte Public Keys werden abgewiesen.

`keys add` fügt einen Schlüssel hinzu und widerruft keinen anderen automatisch. Für verlorene/kompromittierte Schlüssel empfiehlt sich die explizite Folge: Client sperren, alten Token widerrufen, neuen Public Key zuordnen, Client wieder zulassen. Die stabile Client-ID und ACL bleiben erhalten. Bei der Datenanmeldung mit einem administrativ zugeordneten neuen Key muss `--client-id` die bestehende stabile Identität angeben. Die gewöhnliche authentifizierte Rotation bleibt das bevorzugte Verfahren, wenn der bisherige Schlüssel noch verfügbar und gültig ist.

## Atomare Wirkung

Mutationen laufen unter demselben Core-Lock wie Nachrichtenoperationen. Eingaben und Kapazitäten werden vor der Änderung geprüft. ACL-Änderungen erhöhen die Revision und invalidieren betroffene Client-Sessions und Schlüsselwechsel. Eine Sperre verwirft ebenfalls die bisherigen Sessions und ausstehenden Wechsel; Entsperren stellt keine Sitzung wieder her. Die Session-Parallelitätslimits werden je Client im Core verwaltet, sodass auch nachträglich registrierte Clients sofort dieselben Grenzen erhalten.

Ein Schlüsselwiderruf entfernt nur den ausgewählten Schlüssel und dessen Sessions. Ausstehende Auth-Challenges werden ebenfalls verworfen, damit sie nach einer späteren ausdrücklichen Neuregistrierung desselben Keys nicht wieder gültig werden. Andere Schlüssel derselben Identität bleiben verwendbar, solange der Client nicht insgesamt gesperrt ist. Auch nach Widerruf des letzten Keys bleibt die Identität erhalten; `keys: []` ist deshalb in einer Startkonfiguration erlaubt. Ohne registrierten gültigen Key ist keine Datenanmeldung möglich.

Bereits gespeicherte Nachrichten, Eigentum und Bestätigungspaare werden durch diese Verwaltungsaktionen nicht gelöscht. Ein gültig neu angemeldeter Client kann seine ursprünglichen aktiven Inhalte und vorher erhaltenen Paarreferenzen entsprechend den bestehenden Regeln weiter verwenden. Nachrichtenablauf bleibt maßgeblich.

Die Invalidierung wirkt unmittelbar für neue Core-Operationen. Netzwerkverbindungen schließen bei weiterer Nutzung oder beim Auth-Check; ein bereits laufender Versand ist zusätzlich durch den bisherigen Netzwerk-Timeout begrenzt. Bereits vor der Änderung gebildete beziehungsweise übertragene Antworten lassen sich nicht zurückrufen.

Lokale Topics und TTL-Policies können über die [Topic-Verwaltung](runtime-topics.md) geändert werden. Ressourcenlimits, Listener und Routing bleiben Startkonfiguration; es gibt keinen beliebigen Datei-Hot-Reload. Client-Identitäten werden nicht gelöscht oder Slots umnummeriert, solange ihnen noch RAM-Inhalte zugeordnet sein können.

## Expliziter Export

Sicherheitsmutationen wirken sofort im RAM und werden automatisch im separaten [Security-Journal](security-journal.md) eingereiht. By default (`security_storage.sync: true`), successful security-mutation replies wait for the captured journal sequence outside the core lock. Explicit `security_storage.sync: false` allows asynchronous confirmation. `security_storage_unsynced` means RAM was changed but durable confirmation failed; inspect status before retrying. Read-only requests remain immediate. Ein Neustart stellt den synchronisierten Sicherheitszustand wieder her; noch ausstehende Writes können bei einem Absturz verloren gehen. Der Status enthält dafür `security_storage` mit eingereihter/dauerhafter Sequenz, Queue-Belegung und Speicherfehler. Eine volle Queue weist neue Mutationen vor ihrer Wirkung mit `security_storage_busy` ab. Die ursprüngliche CFG-Datei wird nie automatisch zurückgeschrieben. Ein Export enthält die aktuell registrierten Keys einschließlich abgeschlossener Rotation, Laufzeit-Registrierungen, Sperrflags und aktuelle ACLs:

```sh
cargo run --locked -- admin export \
  --socket .local/unix/run/mynofy-admin.sock \
  --out config.snapshot.json
cargo run --locked -- check-config --config config.snapshot.json
```

`--out` erzeugt eine neue Datei mit privaten Dateirechten und überschreibt keine bestehende Datei. `.yaml`/`.yml` erzeugt YAML, andere Endungen JSON. Ohne `--out` bleibt stdout JSON. [YAML-Profil und Konvertierung](yaml-config.md). Der Benutzer kann sie prüfen und später ausdrücklich mit `serve --config config.snapshot.json` verwenden. Dateipfade sind absolut; beim Umzug auf einen anderen Host müssen sie gegebenenfalls angepasst werden.

Exportiert werden keine privaten Schlüsseldatei-Inhalte, Rotations-Secrets, Nachrichten, Statuspaare, Sessions, Cursor, Rate-Limiter oder laufenden Wechsel. TLS-Dateipfade bleiben Teil der Konfiguration. Revisionen und Owner-Generation sind neuer RAM-Zustand nach Neustart. Der Export ist ein konsistenter Metadaten-Snapshot, keine Nachrichtenpersistenz. Er enthält den Security-Pfad und ersetzt dessen Journal nicht; für eine unabhängige Kopie einen eigenen Pfad setzen. Kopieren der Metadaten erfolgt unter dem Core-Lock; Validierung, Serialisierung und Socket-/Datei-I/O danach. Sehr große Metadatensnapshots und Diagnose-Scans sind keine konstantzeitigen Message-Operationen.

## Separates Protokoll und Grenzen

Auf dem Admin-Socket gilt ein kleines JSONL-Protokoll mit genau einer Anfrage und einer Antwort pro Verbindung. Danach schließt der Dienst die Verbindung. Es ist kein WebSocket und kein Bestandteil der normalen Protobuf-Frames.

```json
{"version":1,"command":{"op":"status"}}
```

```json
{"version":1,"ok":false,"error":{"code":"not_found","detail":"Client, key or local topic not found."}}
```

Erfolg liefert `{"version":1,"ok":true,"result":{...}}`. Die CLI gibt nur `result` aus; bei Fehlern endet sie mit einem Fehlerstatus. Unbekannte Felder/Operationen und doppelte JSON-Schlüssel werden abgewiesen. Die Rust-Eingabetypen, das [Schema](../protocol/admin/v1.schema.json) und die [Beispiele](../protocol/admin/examples.json) beschreiben die aktuelle Version.

Der Listener begrenzt gleichzeitig bearbeitete Verbindungen auf vier, neue zugelassene Verbindungen auf zehn pro Sekunde mit Burst 20, Requests auf 16 KiB und Antwort-JSON auf 32 MiB. Eine Verbindung hat fünf Sekunden Netzwerkdeadline. Der Serializer stoppt an der Antwortgrenze, statt einen unbeschränkt großen Antwortpuffer aufzubauen. Ein zu großer Export liefert `resource_exhausted`. Die Metadaten-Snapshotkopie selbst ist proportional zum bestehenden Register-/ACL-Bestand.

Es gibt keine automatischen Mutations-Retries. Geht eine Antwort verloren, kann eine Änderung bereits erfolgt sein; vor Wiederholung den aktuellen Stand abfragen. Die Grenzen sind für eine lokale Kontrollschnittstelle gedacht, nicht als Masseneinschreibungsprotokoll für hohe Datenraten. Die Datenpfad-Limits bleiben getrennt.

## Verifikation

Unit-Tests prüfen Registrierung ohne implizite Rechte, geteilte Session-Limits für neue Clients, Sperre/Entsperre, ungültige atomar abgewiesene Änderungen, Widerruf einschließlich ausstehender Challenges sowie Export nach administrativer Neueinrichtung und nach normaler Rotation. Integrationstests verwenden Daten-Unix-Socket und WSS neben dem Admin-Socket, sämtliche CLI-Routen, Snapshot-Neustart, Dateimodi, andere Peer-UID, unbekannte/doppelte/übergroße Requests und den Schutz bestehender beziehungsweise ausgetauschter Socket-Dateien. Ein echter CLI-Server wird per SIGTERM beendet; beide eigenen Socket-Dateien werden entfernt.

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked -p mynofy-client
cargo check --locked -p mynofy-protocol --no-default-features
```

Keine neue Registry-Abhängigkeit. Die übrigen zurückgestellten Prototypschritte wurden damit nicht vorgezogen.

## Content speichern

`mynofy admin content save --socket PATH` sendet `{"version":1,"command":{"op":"content_save"}}`. Die Antwort `{"queued":true,"revision":N}` bestätigt die erfasste und eingereihte RAM-Kopie. Schreiben/Verschlüsseln läuft danach ohne Core-Lock; im `status` unter `content_storage` `saved_revision`, `save_in_progress`, `dirty` und `last_error` prüfen. Höchstens ein Save gleichzeitig, sonst `content_save_busy`; ohne Konfiguration `content_storage_disabled`. Der Pfad kommt ausschließlich aus der CFG. Es gibt kein Admin-Load/Restore/Reload. [Einrichtung und Grenzen](content-storage.md).

Mit `--topics TOPIC,TOPIC` beziehungsweise `{"op":"content_save","topics":["jobs/status","machines/status"]}` wird eine ausdrücklich ausgewählte Gruppe zum selben Zeitpunkt erfasst. Ohne Auswahl werden alle persistenten Topics gemeinsam gesichert; RAM-Topics sind ausgeschlossen. Intervall-Saves erfassen nur fällige, veränderte Intervall-Topics und lassen manuelle Stände unverändert. Status `content_storage.topics` zeigt Strategie, aktuelle/gespeicherte Revision, dirty und loaded pro persistentem Topic. `saved_revision` auf Owner-Ebene ist die letzte erfolgreiche Save-Auftragsrevision und bedeutet bei einer Teilauswahl nicht, dass alle Topics sauber sind. Leere, doppelte, unbekannte oder RAM-Auswahlen ergeben `invalid_content_topics`.

## Topic-Speicher aktivieren

`mynofy admin topics storage TOPIC --mode manual|interval [--seconds N] [--encrypted true|false] --socket PATH` sendet `topic_storage`. Nur lokale RAM-Topics können aktiviert werden; Rückwechsel und weitere Änderungen persistenter Strategien werden online abgewiesen. Der Wechsel ist zunächst ausschließlich RAM-Zustand und wird erst mit erfolgreichem Content-Save dauerhaft. Bereits vorhandene aktive Nachrichten und Paare werden dann vollständig mitgesichert. Offline-Rückwechsel, Backup, Restore und Rekey: [Leitfaden](restart-and-storage.md).


Im [Peer-/Proxy-Betrieb](service-network.md) enthält Status zusätzlich `service_links` mit registrierten Peer-IDs, aktiven Kanalressourcen und befristeten Routen. Das ist eine lokale Diagnose; es gibt darüber noch keine Peer-/Routing-Mutation.

## Service-Peers

`peers list|set|delete|disable|enable` und
`peers revoke-certificate|revoked-certificates` erweitern denselben getrennten
Admin-Socket. [Befehle, Sicherheitswirkung und Persistenz](runtime-peers.md).

All completed responses use the same JSONL framing: one JSON object followed by
LF and connection shutdown, including `security_storage_unsynced` errors. A
transport interruption or deadline can still truncate a response; never interpret
such a partial frame as a successful mutation confirmation.
