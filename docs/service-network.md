# Service-Vernetzung: implementierter Stand

Eine Instanz kann lokale Topics besitzen und Remote-Topics über einen konfigurierten Upstream vermitteln. Implementiert sind CA-basierte gegenseitige TLS-Authentifizierung, Registrierung, befristete Routenauskünfte und mehrere logische Client-Kanäle auf einem gemeinsamen Peer-WebSocket. Zwei- und Drei-Dienste-Ketten sind getestet. [Runtime-Peer-Verwaltung](runtime-peers.md)
ergänzt eingehende Peer-Freigaben, Sperren und lokalen Zertifikatswiderruf;
Listener, CA-Dateien und Upstream-Adresse bleiben Startkonfiguration.

```text
Client ── WebSocket zu edge ── mTLS-Link zu hub ── mTLS-Link zu owner
       ╰──────────────── innere TLS-Sitzung ────────────────────╯
                   Client-Keypair-Auth und Nachrichten
```

Die äußeren Verbindungen identifizieren benachbarte Dienste. Die innere TLS-Sitzung endet erst beim Owner; darin laufen der unveränderte Client-Keypair-Handshake und JSON-/Protobuf-Nachrichten. Der Client benötigt eine Registrierung und ACL beim Owner. Eine Registrierung am Proxy ist für diesen Zugang nicht erforderlich und ersetzt keine Owner-Freigabe. Ein Peer erhält keine Client-/Producer-Rechte.

## Lokal ausprobieren

Als unprivilegierter Benutzer im Repository:

```sh
./scripts/setup-links.sh
```

Das Skript erzeugt `.local/links/` einmalig und überschreibt nichts. Es erstellt eine Demo-CA, drei Zertifikate mit `serverAuth,clientAuth`, eindeutigen DNS-SANs, einen Client-Key und YAML-Konfigurationen mit getrennten Security-Verzeichnissen. `MYNOFY_BINARY` kann auf ein bereits gebautes Binary zeigen, `MYNOFY_LINKS_DIR` auf ein neues Zielverzeichnis. Ports: edge 8090, hub 8091, owner 8092. Es startet selbst keine Dienste.

In drei Terminals starten:

```sh
cargo run --locked -- serve --config .local/links/owner.yaml
cargo run --locked -- serve --config .local/links/hub.yaml
cargo run --locked -- serve --config .local/links/edge.yaml
```

Der bereits geltende explizite UID-/GID-Zwang bei privilegiertem CLI-Start bleibt erhalten. Das Setup wählt keinen Dienstkonto-Fallback.

Client über die gesamte Kette zum Owner:

```sh
cargo run --locked -- client \
  --url wss://127.0.0.1:8090 \
  --ca .local/links/ca.crt \
  --key .local/links/client.pk8 \
  --owner owner \
  --via-topic team/events \
  --owner-ca .local/links/ca.crt
```

Danach beispielsweise JSONL auf stdin:

```json
{"version":1,"type":"publish","id":"p","topic":"team/events","payload":"review requested"}
{"version":1,"type":"poll","id":"q","topic":"team/events"}
{"version":1,"type":"get_permissions","id":"r","topic":"team/events"}
```

`--ca` prüft den Einstiegspunkt. `--owner-ca` prüft den Owner innerhalb des Tunnels; beide können unterschiedliche CAs sein. Der erwartete Owner-DNS-/IP-Name ist standardmäßig `--owner`, alternativ `--owner-name DNS_OR_IP`. Die erwartete logische Service-ID bleibt unabhängig davon `--owner`. Ein falscher Owner, Zertifikatsname oder Trust-Anchor beendet den Aufbau. Routeninformationen des Proxys sind kein Ersatz für diese Prüfung.

Ein lokaler Einstieg über `--socket PATH` unterstützt dieselben `--via-topic`, `--owner-ca` und `--owner-name`-Optionen. Dafür darf die Unix-Konfiguration zusätzlich `service.tls` mit den ausgehenden Peer-Zugangsdaten enthalten, wenn ein Upstream existiert. Der lokale Socket bleibt ein Dateisystem-geschützter Zugang ohne äußeres TLS; die innere Owner-TLS-Sitzung bleibt bestehen. `rotate`, `adapter` und `--progress` verwenden denselben generischen Client-Kanal.

## Konfiguration und Identitätsbindung

Beispiel eines vermittelnden Dienstes:

```yaml
upstream:
  service_id: owner
  url: wss://127.0.0.1:8092/
  trust:
    mode: ca
    ca_file: ca.crt
    expected_certificate_identity: owner
topics:
  - name: team/events
    owner_id: owner
    via: owner
```

Eigene Topics können zusätzlich in derselben Liste stehen. Remote-Einträge tragen keine lokale Policy oder ACL. Pro Instanz gibt es höchstens einen Upstream. Beim nächsten Hop:

```yaml
accepted_peers:
  - service_id: hub
    topics: [team/events]
```

Der annehmende Dienst benötigt einen TCP-/TLS-Listener mit `service.tls.peer_ca_file`. Er prüft Client-Zertifikat, Gültigkeit und Client-Verwendung durch rustls. Normale Clients dürfen ohne TLS-Client-Zertifikat zum öffentlichen Datenlistener verbinden; `service_hello` setzt jedoch ein tatsächlich verifiziertes Client-Zertifikat voraus.

Zusätzlich muss dessen SAN zur konfigurierten Peer-Identität passen. Standard ist `accepted_peers[].service_id`; ein abweichender Zertifikatsname lässt sich über `certificate_identity` ausdrücklich zuordnen. Doppelte Peer-IDs oder Zertifikatsidentitäten werden abgewiesen. Ausgehend prüft TLS `upstream.trust.expected_certificate_identity`, anschließend muss das Welcome exakt die konfigurierte `service_id` bestätigen. Ein JSON-Name allein authentifiziert nichts. Ein Dienstzertifikat sollte beide Verwendungen `serverAuth,clientAuth` enthalten, wenn der Dienst beide Rollen ausübt.

Der erste erfolgreiche Remote-Zugriff baut den gemeinsamen Upstream-Link auf; ein ausgefallener Upstream verhindert den lokalen Start nicht. Pro Peer ist nur eine aktive Registrierung erlaubt. Ein nach beendeter Verbindung neuer Zugriff kann einen neuen Link aufbauen. Bereits gestartete Commands oder Client-Sitzungen werden dabei nicht wiederholt beziehungsweise wiederaufgenommen.

`ca` ist der implementierte Vertrauensmodus. SPKI-Pinning, TOFU, automatische Peer-Erkennung, dynamische Peer-/Routen-Konfiguration und Zertifikatswechsel im Betrieb sind nicht implementiert. Änderungen an diesen Startparametern benötigen einen Neustart. Zertifikatsgültigkeit wird beim TLS-Aufbau geprüft; Live-Widerruf/CRL-/OCSP-Verwaltung sind nicht implementiert. Die [Laufzeit-Topic-Verwaltung](runtime-topics.md) verändert keine Peer-Freigaben oder statischen Remote-Routen und kündigt neue Topics nicht automatisch im Netz an.

## Routing und Owner-Sitzungen

`route_query`/`route_info` laufen nur auf registrierten Peer-Verbindungen. Der letzte `visited`-Eintrag muss dem authentifizierten Nachbarn entsprechen; eigene, doppelte oder ungültige Knoten werden abgewiesen. Jede Weiterleitung reduziert das Hop-Limit. Bei 1 darf nur lokal geantwortet werden. Die Konfiguration begrenzt es auf 1–16.

Jeder Hop muss das Topic ausdrücklich für den anfragenden Peer freigeben. Die Antwort muss mit dem unmittelbaren nächsten Hop beginnen und beim konfigurierten Owner enden. Eigentümerkonflikte, Schleifen und abgelaufene Routen führen zum Fehlschlag, niemals zu lokaler Übernahme. Jede neue Client-Verbindung löst die Route erneut auf; die gelernte Tabelle dient derzeit der Diagnose, nicht als Autorisierungs- oder Failover-Cache. Zwischenstationen verlängern die empfangene Ablaufzeit nicht. Peer-Abbruch verwirft dessen gelernte Wege, abgelaufene Einträge werden bereinigt. Keine Routing-Persistenz und kein Broadcast.

Der Client wählt mit `--via-topic` einen Weg zu einem Owner. Die entstehende Sitzung gehört dem Client und diesem Owner, nicht einer delegierten Proxy-Identität. Sie kann die beim Owner verfügbaren Operationen gemäß dessen aktuellen ACLs ausführen. Die Peer-Topic-Liste begrenzt die Routensuche und Kanalöffnung; sie ist keine zusätzliche ACL für innerhalb der TLS-Sitzung verborgene Commands. Wer die Erreichbarkeit eines Owners vermitteln darf, muss deshalb weiterhin auf dessen Client-Authentifizierung und ACLs vertrauen.

Nachrichteninhalt, Expires, Cursor, Producer-Eigentum und Bestätigungsreferenzen werden nicht umgeschrieben. Numerische Message-/ACK-Referenzen bleiben durch die authentifizierte Owner-Sitzung eindeutig zugeordnet; es entsteht keine globale ID-Vergabe. Ein Transport-Hop bestätigt keine fachliche Verarbeitung. `get_permissions`, Producer-Abfragen, ACK/NACK und clientinitiierte Rotation werden beim Owner verarbeitet.

## Drahtformat und Grenzen

[Schema](../protocol/v1/service-frame.schema.json), [Handshake-Beispiele](../protocol/v1/service-examples.json), [Routen-Beispiele](../protocol/v1/route-examples.json) und [Tunnel-Beispiele](../protocol/v1/tunnel-examples.json).

- Kontrolle: strikt geprüfte JSON-Textframes, Version 1, maximal 16 KiB; `service_hello`, `service_welcome`, `route_query`, `route_info`, `tunnel_open`, `tunnel_ready`, `tunnel_close`, `tunnel_credit` und korrelierte Fehler.
- Je Peer-Datenframe: WebSocket-Binary mit vier Byte LE-Kanalnummer, danach 1–8192 opaque TLS-Bytes. Kanalnummern werden innerhalb einer Verbindung nicht wiederverwendet. Der innere Nachrichteninhalt bleibt Protobuf beziehungsweise JSON wie bei einem direkten Owner-Zugang.
- Client-Einstieg: `client_tunnel`/`client_tunnel_ready`, danach binäre TLS-Bytes in Blöcken bis 8192 Byte ohne Peer-Kanalpräfix. Der Proxy kopiert diese in den gewählten logischen Kanal.
- `limits.max_service_channels`: standardmäßig 64, zulässig 1–4096, effektiv höchstens `max_connections`. Das globale Budget je Instanz zählt eingehende und ausgehende Kanäle, auch bis deren Worker vollständig beendet sind. Ein Zwischenknoten benötigt für einen durchlaufenden Client zwei Plätze.
- Pro Richtung und Kanal vier Credits, maximal vier gepufferte Datenframes sowie begrenzte Duplex-Puffer. Ein Credit wird erst nach Übernahme durch den lokalen Stream zurückgegeben. Ein blockierter Leser hält den eigenen Kanal an; er blockiert nicht das Einlesen anderer Kanäle. Falsche Credits und wiederverwendete Kanalnummern sind Protokollfehler.
- Begrenzte Event-Queue und höchstens 32 ausstehende beziehungsweise parallel bearbeitete Routenabfragen pro Link. TCP-/TLS-/WS-Aufbau, Writes, Abfragen und Stream-Transfers verwenden Deadlines; Auth- und Idle-Timeouts bleiben aktiv. Ein Peer-Protokollfehler beendet dessen Link und alle zugehörigen Kanäle.

Bei Timeout/Abbruch gibt es keine automatische Command-Wiederholung, kein Store-and-forward und kein nachträgliches Abrufen verlorener Antworten. Ein Publish kann beim Owner angekommen sein, auch wenn der Aufrufer keine Antwort mehr erhält. Der Client entscheidet über einen neuen Verbindungsaufbau und Wiederholungen. Netzwerk-I/O hält keinen Engine-Lock.

## Diagnose und geprüfter Umfang

```sh
cargo run --locked -- admin status --socket .local/links/run/edge-admin.sock
```

`service_links` zeigt registrierte Peer-IDs, aktive Kanalressourcen und gelernte Routen mit Owner, nächstem Hop, Quelle, Pfad und `expires_at_ms`. Es enthält keine Payloads oder Schlüssel. Fehlgeschlagene Registrierung oder Owner-TLS-Prüfung kann als Verbindungsabbruch erscheinen; es gibt keine automatische Lockerung der Prüfung.

Unit-Tests prüfen strikte Kontrollframes, dokumentierte Beispiele, Hop-/Schleifenregeln und die Bindung an konfigurierte Owner. Echte Integrationstests prüfen zwei/drei Dienste, parallele Kanäle, große Poll-Seiten, langsame Leser, Kanalbudget und Freigabe nach Disconnect, Owner-ACL/Client-Registrierung, getrennte ACK-Rechte, Zertifikats-/SAN-/CA-Fehler, doppelte Peer-Registrierung, zyklische Routen, Schlüsselwechsel und einen CLI-Client über Unix-Einstieg. Zusätzlich wurde das generierte Drei-Dienste-YAML-Beispiel mit drei echten CLI-Prozessen, getrennten Security-Journalen und expliziter unprivilegierter UID lokal ausgeführt.

Die vorhandenen Funktionstests und Speicherabbruchtests bleiben Teil von `cargo test --locked --workspace`. Native macOS-Laufzeitprüfung und Netzwerk-Performance-Messungen wurden in diesem Schritt nicht durchgeführt.
