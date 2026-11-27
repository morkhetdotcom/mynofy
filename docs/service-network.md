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

Zusätzlich muss dessen SAN zur konfigurierten Peer-Identität passen. Standard ist `accepted_peers[].service_id`; ein abweichender Zertifikatsname lässt sich über `certificate_identity` ausdrücklich zuordnen. Doppelte Peer-IDs oder gleichwertige TLS-Namen werden abgewiesen. Das gilt auch für DNS-Namen mit anderer Großschreibung oder abschließendem Punkt und für alternative Schreibweisen derselben IPv6-Adresse. Ausgehend prüft TLS `upstream.trust.expected_certificate_identity`, anschließend muss das Welcome exakt die konfigurierte `service_id` bestätigen. Ein JSON-Name allein authentifiziert nichts. Ein Dienstzertifikat sollte beide Verwendungen `serverAuth,clientAuth` enthalten, wenn der Dienst beide Rollen ausübt.

Die Peer-CA muss für getrennte Peer-Policies getrennte Dienstschlüssel und
Zertifikate ausstellen. Verwende einen peer-spezifischen SAN und prüfe vor
der Bereitstellung, dass kein weiterer konfigurierter Peer-Name durch
zusätzliche SANs oder Wildcards abgedeckt wird. Ein Dienst kann denselben
eigenen Schlüssel für seine eingehenden und ausgehenden Links verwenden.


Beim Start wird die Upstream-WSS-Adresse einschließlich Host und Port geprüft. Der erste erfolgreiche Remote-Zugriff baut den gemeinsamen Upstream-Link auf; ein ausgefallener Upstream verhindert den lokalen Start nicht. Pro Peer ist nur eine aktive Registrierung erlaubt. Ein nach beendeter Verbindung neuer Zugriff kann einen neuen Link aufbauen. Bereits gestartete Commands oder Client-Sitzungen werden dabei nicht wiederholt beziehungsweise wiederaufgenommen.

`ca` ist der implementierte Vertrauensmodus. Listener, TLS-Material, Upstream und statische Remote-Routen werden beim Start geladen. Die lokale Peer-Zulassung und ihre Topic-Freigaben lassen sich zur Laufzeit ändern. Der administrative Widerruf eines einzelnen Peer-Zertifikats beendet betroffene aktive Links und verhindert neue Registrierungen mit diesem Zertifikat. [Laufzeit-Peer-Verwaltung](runtime-peers.md)

Die Laufzeit-Topic-Verwaltung bearbeitet lokale Topics. Ein neues lokales Topic wird nicht automatisch im Netz angekündigt. [Laufzeit-Topic-Verwaltung](runtime-topics.md)

## Routing und Owner-Sitzungen

`route_query`/`route_info` laufen nur auf registrierten Peer-Verbindungen. Der letzte `visited`-Eintrag muss dem authentifizierten Nachbarn entsprechen; eigene, doppelte oder ungültige Knoten werden abgewiesen. Jede Weiterleitung reduziert das Hop-Limit. Bei 1 darf nur lokal geantwortet werden. Die Konfiguration begrenzt es auf 1–16.

Jeder Hop muss das Topic ausdrücklich für den anfragenden Peer freigeben. Die Antwort muss mit dem unmittelbaren nächsten Hop beginnen und beim konfigurierten Owner enden. Eigentümerkonflikte, Schleifen und abgelaufene Routen führen zum Fehlschlag, niemals zu lokaler Übernahme. Der Edge kann eine gültige gelernte Route für einen neuen Tunnel desselben Topics wiederverwenden. Ohne gültigen Eintrag fragt er die Route neu ab. Ein abgelehnter Kanalaufbau verwirft den Eintrag. Die Freigaben aller Hops werden beim Kanalaufbau erneut geprüft. Zwischenstationen verlängern die empfangene Ablaufzeit nicht. Peer-Abbruch verwirft dessen gelernte Wege, abgelaufene Einträge werden bereinigt. Keine Routing-Persistenz und kein Broadcast.

Der Client wählt mit `--via-topic` das Topic und den Weg zum Owner. Die getunnelte Sitzung ist an dieses Topic gebunden. Für ein weiteres Topic öffnet der Client einen eigenen Tunnel. Der Owner prüft zusätzlich seine Client-ACL und das Producer-Eigentum. Betriebssystem-Zugriff und Peer-Freigaben erteilen keine Datenrechte beim Owner. Producer-Listen gelten für das Tunnel-Topic, auch wenn der Client kein Topic angibt. Einzelabfragen außerhalb des Topics melden die Nachricht als nicht gefunden. Katalog und Schlüsselrotation bleiben für angemeldete Clients verfügbar. Mehrere Tunnel können denselben Peer-Link nutzen.

Der Owner reserviert für ausstehende Tunnel höchstens ein Viertel seiner Kanalplätze (mindestens einen) und pro authentifiziertem Peer höchstens ein Achtel (mindestens einen). Ein TCP-Edge begrenzt zusätzlich die ausstehenden Tunnel je Quell-IP auf ein Sechzehntel seiner Kanalplätze (mindestens einen). Nach erfolgreicher Broker-Anmeldung bestätigt der Owner den Kanal über den Peer-Link. Zwischenstationen geben die Bestätigung bis zum Edge weiter. Abbruch und Verbindungsende geben die Aufnahmeplätze frei. Der Edge validiert den angeforderten Owner vor der Routenabfrage. Der Peer-Handshake verlangt auf beiden Seiten `link_version: 2`. Eine fehlende oder abweichende Version wird vor der Registrierung abgelehnt. Dienste eines Peer-Netzes werden gemeinsam aktualisiert.

Nachrichteninhalt, Ablaufzeit, Cursor und Producer-Eigentum bleiben an den Owner gebunden. Message-IDs gelten im Kontext des Owners. Fachliche Verarbeitung melden Anwendungen über eigene Nachrichten. Der Owner verarbeitet Berechtigungs- und Producer-Abfragen sowie clientinitiierte Schlüsselrotation.

## Drahtformat und Grenzen

[Schema](../protocol/v1/service-frame.schema.json), [Handshake-Beispiele](../protocol/v1/service-examples.json), [Routen-Beispiele](../protocol/v1/route-examples.json) und [Tunnel-Beispiele](../protocol/v1/tunnel-examples.json).

- Kontrolle: strikt geprüfte JSON-Textframes, Version 1, maximal 16 KiB; `service_hello`, `service_welcome`, `route_query`, `route_info`, `tunnel_open`, `tunnel_ready`, `tunnel_authenticated`, `tunnel_close`, `tunnel_credit` und korrelierte Fehler.
- Je Peer-Datenframe: WebSocket-Binary mit vier Byte LE-Kanalnummer, danach 1–8192 opaque TLS-Bytes. Kanalnummern werden innerhalb einer Verbindung nicht wiederverwendet. Der innere Nachrichteninhalt bleibt Protobuf beziehungsweise JSON wie bei einem direkten Owner-Zugang.
- Client-Einstieg: `client_tunnel`/`client_tunnel_ready`, danach binäre TLS-Bytes in Blöcken bis 8192 Byte ohne Peer-Kanalpräfix. Der Proxy kopiert diese in den gewählten logischen Kanal.
- `limits.max_service_channels`: standardmäßig 64, zulässig 1–4096, effektiv höchstens `max_connections`. Das globale Budget je Instanz zählt eingehende und ausgehende Kanäle, auch bis deren Worker vollständig beendet sind. Ein Zwischenknoten benötigt für einen durchlaufenden Client zwei Plätze.
- Pro Richtung und Kanal vier Credits, maximal vier gepufferte Datenframes sowie begrenzte Duplex-Puffer. Ein Credit wird erst nach Übernahme durch den lokalen Stream zurückgegeben. Ein blockierter Leser hält den eigenen Kanal an; er blockiert nicht das Einlesen anderer Kanäle. Falsche Credits und wiederverwendete Kanalnummern sind Protokollfehler.
- Begrenzte Event-Queue und höchstens 32 ausstehende beziehungsweise parallel bearbeitete Routenabfragen pro Link. TCP-/TLS-/WS-Aufbau, Writes, Abfragen und Stream-Transfers verwenden Deadlines; Auth- und Idle-Timeouts bleiben aktiv. Ein Peer-Protokollfehler beendet dessen Link und alle zugehörigen Kanäle.

Bei Timeout/Abbruch gibt es keine automatische Command-Wiederholung, kein Store-and-forward und kein nachträgliches Abrufen verlorener Antworten. Ein Publish kann beim Owner angekommen sein, auch wenn der Aufrufer keine Antwort mehr erhält. Der Client entscheidet über einen neuen Verbindungsaufbau und Wiederholungen. Netzwerk-I/O hält keinen Engine-Lock.

## Diagnose und geprüfter Umfang

`admin status` zeigt unter `service_links` die konfigurierte Upstream-ID sowie
`upstream_connecting`, `upstream_connected` und `upstream_last_error`.
Der letzte Fehler enthält nur Phase und UTC-Zeit in Unix-Millisekunden. Nach
einem fehlgeschlagenen Aufbau drosselt der Edge neue Verbindungsversuche für
250 ms. Weitere Verbindungsversuche entstehen durch neue Anfragen.


```sh
cargo run --locked -- admin status --socket .local/links/run/edge-admin.sock
```

`service_links` zeigt registrierte Peer-IDs, aktive Kanalressourcen und gelernte Routen mit Owner, nächstem Hop, Quelle, Pfad und `expires_at_ms`. Es enthält keine Payloads oder Schlüssel. Fehlgeschlagene Registrierung oder Owner-TLS-Prüfung kann als Verbindungsabbruch erscheinen; es gibt keine automatische Lockerung der Prüfung.

Unit-Tests prüfen strikte Kontrollframes, dokumentierte Beispiele, Hop-/Schleifenregeln und die Bindung an konfigurierte Owner. Echte Integrationstests prüfen zwei/drei Dienste, parallele Kanäle, große Poll-Seiten, langsame Leser, Kanalbudget und Freigabe nach Disconnect, Owner-ACL/Client-Registrierung, Topic-Bindung getunnelter Sitzungen, Zertifikats-/SAN-/CA-Fehler, doppelte Peer-Registrierung, zyklische Routen, Schlüsselwechsel und einen CLI-Client über Unix-Einstieg. Zusätzlich wurde das generierte Drei-Dienste-YAML-Beispiel mit drei echten CLI-Prozessen, getrennten Security-Journalen und expliziter unprivilegierter UID lokal ausgeführt.

Die vorhandenen Funktionstests und Speicherabbruchtests bleiben Teil von `cargo test --locked --workspace`. Native macOS-Laufzeitprüfung und Netzwerk-Performance-Messungen wurden in diesem Schritt nicht durchgeführt.


Tunnel setup shares an in-flight connection attempt per upstream. Each
request uses one deadline through route lookup, channel opening and readiness.
Disconnecting the requesting socket releases its pending admission early.
The established link ends at the remote peer certificate's expiry, including
any attached tunnels. The next request needs a fresh TLS handshake.
