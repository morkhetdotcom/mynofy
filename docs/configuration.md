# Konfiguration und RAM-Betrieb des Prototyps

Bevorzugtes Format ist YAML (`.yaml`/`.yml`); bestehendes JSON bleibt unterstützt. Beide Formate haben dieselben Typen und Prüfungen. [YAML-Profil, Export und Konvertierung](yaml-config.md).

Current model: [stateless polling and upgrade notes](message-client-confirmation.md). ACK/NACK, delivery IDs, confirmation counters and pair limits have been removed; update services and clients together.

Nachrichten, Bestätigungen, Sessions, unvollständige Schlüsselwechsel und Rate-Limiter bleiben im RAM. Clients/Keys/Sperren und lokale ACLs werden dagegen nach jeder Änderung automatisch asynchron im separaten [Security-Journal](security-journal.md) gespeichert. Die CFG liefert Betriebsparameter und die erstmalige Security-Baseline. Ein separater optionaler [Content-Save](content-storage.md) speichert Nachrichten und Bestätigungen manuell oder periodisch; Restore und optionale Entschlüsselung passieren ausschließlich vollständig beim Start.

Die Dateien unter [config/scenarios](../config/scenarios/local.yaml) sind vom Konfigurationsparser prüfbare Baselines. Lokaler Owner und CA/mTLS-Peer-/Edge-Betrieb sind implementiert. Service-Modi benötigen passende Zertifikate und Trust-Zuordnungen; das Drei-Dienste-Setup ist im [Netzwerk-Leitfaden](service-network.md) beschrieben. Das [operating guide](getting-started.md) erzeugt eine tatsächlich ausführbare lokale Variante:

| Datei | Szenario |
| --- | --- |
| [unix.yaml](../config/scenarios/unix.yaml) | Lokaler Unix-Socket ohne TLS-Dateien; Linux/macOS. |
| [local.yaml](../config/scenarios/local.yaml) | Einzelner Owner mit lokalem Test-Topic. |
| [central.yaml](../config/scenarios/central.yaml) | Owner für ein gemeinsam genutztes Topic mit eigener ACL. |
| [edge.yaml](../config/scenarios/edge.yaml) | Eigenes lokales Topic und Remote-Topic über den zentralen Dienst. |

Mit Ausnahme der festen maximalen 4096 Payloadbytes sind die Zahlen konfigurierbare Testwerte, keine gemessenen Kapazitätsgrenzen oder unveränderlichen Produktdefaults. Die Datei lädt Service-ID, Listener/TLS, bekannte Client-Public-Keys, ACLs der selbst verantworteten Topics, Topic-Policies, einen optionalen Upstream, Vertrauenszuordnungen und Ressourcenlimits.

Der Beispiel-Client verwendet ausschließlich den öffentlichen Schlüssel aus den Auth-Testdaten. Sein privater Schlüssel wurde verworfen. Für einen ausführbaren Test ist ein neues Client-Schlüsselpaar zu erzeugen und dessen Public Key einzutragen; Client-ID und Token-ID sind entsprechend abzuleiten. TLS-Dateien sind lokale Dateireferenzen und müssen vor einem späteren Start bereitgestellt werden. Es werden keine Client-Private-Keys oder Shared Secrets in Szenariodateien hinterlegt.

## Start- und Reset-Regeln

- Ein neuer Start lädt Betriebskonfiguration und Topic-Definitionen aus der CFG. Clients/Keys/Sperren und lokale ACLs werden aus einem vorhandenen gültigen Security-Snapshot mit Journal wiederhergestellt; die CFG dient dafür nur beim ersten Start als Baseline. Ein bereits initialisierter, beschädigter Speicher wird nicht stillschweigend durch die CFG ersetzt.
- Noch nicht synchronisierte Sicherheitsänderungen können bei einem Absturz verloren gehen. Der Admin-Status unterscheidet eingereihte und dauerhaft gespeicherte Sequenzen. Die CFG wird weiterhin nicht automatisch zurückgeschrieben.
- Ohne Content-Speicher starten Nachrichten und sonstiger RAM-Zustand leer. Ein konfigurierter Content-Checkpoint stellt den gespeicherten Nachrichten-/Bestätigungsstand mit unveränderten Ablaufzeiten wieder her. Eine neue Owner-Generation macht alte Producer-Listen-Cursor ungültig. Kompakte Poll-Indizes erkennen den Neustart nicht zuverlässig; nach bekanntem Neustart ohne `after` neu beginnen.
- `security_storage.sync: true` waits for durable journal synchronization before successful service security-mutation replies; default `true` confirms durable writes; explicit `false` allows asynchronous acknowledgement. Message persistence is independent. See [security journal](security-journal.md).
- Bei konfigurierter Content-Persistenz verlangt der Dienst eine `content_storage.store_id` und ein mit `mynofy content-init --config FILE` eingerichtetes Volume. Ein fehlendes oder falsch zugeordnetes Volume verhindert den Start. [Content-Storage](content-storage.md).
- Der CLI-Dienst verlangt `security_storage.directory` als expliziten absoluten oder CFG-relativen Pfad und einen bereits initialisierten Store. Die Ersteinrichtung erfolgt mit `mynofy security-init --config FILE`. Queue- und Kompaktierungsparameter sowie Dateirechte stehen im [Journal-Leitfaden](security-journal.md).

## Rate-Limits

Jeder authentifizierte Client erhält ein gemeinsames Rate-Limit über alle seine Verbindungen. Schlüsselwechsel und Reconnect setzen dieses Budget nicht zurück, weil es an die stabile Client-ID gebunden ist. Zusätzlich gibt es ein eigenes Budget für gestartete Schlüsselwechsel; pro Client darf nur ein Wechsel gleichzeitig aktiv sein. Wiederholte Versuche nach Fristende verbrauchen ebenfalls Budget.

Vor erfolgreicher Authentifizierung ist eine behauptete Client-ID noch kein vertrauenswürdiger Rate-Limit-Schlüssel. Diese Phase benötigt deshalb zusätzliche Begrenzung je Verbindungsquelle und für den gesamten Listener. Unbekannte Token-IDs dürfen nicht beliebig viele getrennte Budgets erzeugen oder ein echtes Client-Budget ohne Identitätsnachweis verbrauchen.

Vorgesehen ist `rate_limited` mit `retry_after_ms`; abgewiesene Commands erzeugen keine Nachrichten oder Zustellversuche. Implementiert ist ein Token-Bucket pro stabiler Client-ID für Commands (ein Token je Frame, auch bei fehlerhaften Requests); vor Auth greifen Quellen-/Listener-Grenzen. Schlüsselwechsel haben zusätzlich ein gleitendes Versuchsfenster. Client-Budget und Ablauf verwenden die Core-Zeit in Millisekunden, Quellenlimits monotone Transportzeit. Eine Grenzüberschreitung bereits vor TLS/Auth führt zum Verbindungsabbruch, weil dort noch keine JSON-Antwort möglich ist. Raten begrenzen Arbeit, ersetzen aber nicht Limits für Bytes, parallele Verbindungen und RAM. Sie sind keine Garantie gegen beliebig großen verteilten Verkehr.

## Weitere Testszenarien

Aus den Baselines sollen Tests für Ablaufzeiten, leere Polls, Rechteverlust bei weiterhin lesbarem Eigentum, Wechsel-Timeout, Rate-Limit über mehrere Sessions, Owner-Ausfall und veraltete Routingtabellen abgeleitet werden. Die Dateien definieren Eingaben; entsprechende Service-Integrationstests sind erst mit der Implementierung möglich.

## Lokaler Unix-Listener

`service.unix_socket` ersetzt `service.listen` und `service.tls`; beide Varianten zugleich sind ungültig. Relative Socket-Pfade werden relativ zur CFG aufgelöst. [setup-unix.sh](../scripts/setup-unix.sh) erzeugt einen getrennten lokalen Teststand unter `.local/unix`, ohne eine bestehende WSS-Konfiguration zu verändern. Keypair-Auth und Topic-ACL gelten weiterhin. Unix-Socket-Betrieb wird auf Nicht-Unix-Plattformen bei der Konfigurationsprüfung abgewiesen.

## Payload-Migration

`limits.max_payload_bytes` liegt jetzt zwischen 1 und 4096; alte Konfigurationen mit 65536 müssen vor dem Neustart angepasst werden. Das begrenzt Nutzbytes, nicht die vollständige Protobuf-/JSON-Nachricht oder eine Poll-Seite. Der Server überschreibt bestehende `.local`-Konfigurationen nicht.

Für den [separaten Admin-Socket](admin-api.md) ist `service.admin_socket` optional. Er muss vom Daten-Socket getrennt sein und wird relativ zur CFG-Datei aufgelöst. Client-Einträge unterstützen `disabled: true`; ohne Angabe gilt false. Nach vollständigem Schlüsselwiderruf ist ein Identitätseintrag mit `keys: []` zulässig, aber ohne Datenanmeldung. Ein expliziter Admin-Export enthält den aktuellen Identitäts-/ACL-Stand; es erfolgt keine automatische Rückschreibung.

## Speicherstrategie pro Topic

`content_storage.directory` stellt den optionalen Inhaltsbackend bereit. Lokale Topics bleiben ohne Angabe `storage: {mode: ram}`. Für ausdrücklich gespeicherte Topics `storage: {mode: manual, encrypted: true}` oder `storage: {mode: interval, seconds: 60, encrypted: false}` ergänzen. Verschlüsselte Topics verwenden das gemeinsame Start-Secret aus `content_storage.encryption: password_stdin` beziehungsweise `key_stdin`. Der frühere globale `save_interval_seconds` entfällt. Gruppen-Saves und Migration alter Checkpoints: [Content-Speicherung](content-storage.md).


Der [Service-Netzwerk-Leitfaden](service-network.md) beschreibt den implementierten CA/mTLS-Betrieb. `scripts/setup-links.sh` erzeugt ein vollständiges lokales Drei-Dienste-Beispiel mit passenden Zertifikaten und YAML-Dateien. `limits.max_service_channels` begrenzt Kanalressourcen je Instanz (Default 64, 1–4096, effektiv höchstens max_connections); durchlaufende Clients benötigen an einer Zwischenstation einen eingehenden und einen ausgehenden Kanal. Ein Unix-Einstieg darf bei vorhandenem Upstream TLS-Zugangsdaten für die ausgehende Service-Verbindung führen.


## Publisher and topic capacity

`limits.max_publisher_active_bytes`, `limits.max_publisher_active_messages`,
`limits.max_topic_active_bytes` and `limits.max_topic_active_messages` take
positive integers. Omitted limits default to half of the corresponding owner
limit, rounded up. The default byte limit allows one valid message even if the
owner limit was set unusually low. Publishers are counted across topics and
topics across publishers. The owner limit still applies to every publish.
A rejected publish leaves existing messages unchanged. Restored content counts
against the current limits, and existing excess drains through normal expiry.


TCP admission retains inactive source rate history for 60 seconds and
expires it in bounded batches. If the source table is full, new sources share
a small overflow rate and connection budget. IPv6 sources also share a /64
connection limit, `limits.max_connections_per_ipv6_prefix` (default 64).
Setting the field to null disables the prefix quota. The prefix also shares
a rate budget equal to four times the per-source rate and burst.


### Optional transport response budget

`limits.max_egress_bytes` reserves one `max_response_bytes` allowance before
processing a client frame. Unix, direct WSS and tunnels terminating at the
owner share the allowance. Durable rotation commits use four separate
small response slots while waiting for the Security journal. Ordinary
requests and responses keep using the normal allowance. This mode
releases the encoding buffer after each
response. Requests wait up to 100 ms for a slot and execute only after
acquiring one. A separate reserve can send four simultaneous small rejection
frames. Authenticated requests with a valid request ID receive a correlated
`resource_exhausted` response when the wait expires, subject to normal rate
limits. Admin status exposes `egress_rejections`. If the reserve is occupied
or delivery fails, the connection can close. The allowance covers logical
encoded-response capacity. Allocator slack, kernel and TLS
buffers, peer relay buffers, content capture and admin export have their own
resource usage. Omitting the field preserves the existing per-session limits.

Producer-list continuation expires after 60 seconds without advancing. Every
successful page with a continuation issues a new deadline. Topic, identity,
scope, authorization revision and the original producer watermark stay bound
to the continuation.


Authenticated WebSocket commands consume one identity-rate token before full
Protobuf decoding. Admission can still read bounded frame metadata. The
owner checks current credentials again immediately before processing the command.
