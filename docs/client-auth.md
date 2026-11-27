# Client-Authentifizierung und Service-ACL

Im lokalen Owner-Prototyp implementiert; das Wire-Format bleibt veränderlich. Remote-/Proxy-Kanäle sind noch Entwurf. Ein Client erzeugt lokal ein Schlüsselpaar. Der Service bekommt ausschließlich den öffentlichen Schlüssel, ordnet ihn einer Client-Identität zu und prüft bei jeder Verbindung den Besitz des privaten Schlüssels. Danach bestimmt die ACL des jeweiligen Topic-Owners die Topic-Rechte. Das Schlüsselpaar wird im Produkt als Token bezeichnet; es ist kein Bearer-Token, das durch bloßes Mitsenden zur Anmeldung genügt.

## Token und stabile Identität

- Für den Prototyp ist **Ed25519** vorgesehen, mit etablierten Bibliotheken gemäß [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032.html). Der Public Key besteht aus 32 Bytes; Signaturen aus 64 Bytes. Private Schlüssel bleiben beim Client und gehören weder in Nachrichten noch in die Service-Konfiguration oder Logs.
- `token_id` ist `ed25519:` gefolgt von Base64 mit Padding über SHA-256 des rohen 32-Byte-Public-Keys. Das ist eine Schlüsselkennung, kein Geheimnis. Sie unterscheidet sich vom SPKI-Fingerprint der TLS-Zertifikate zwischen Diensten.
- Der Betreiber hinterlegt den Public Key über einen vertrauenswürdigen Verwaltungsweg. Der Service berechnet die Token-ID selbst und bindet sie an eine stabile `client_id`. Ein Public Key darf lokal nur einer Client-ID zugeordnet sein. Der Handshake nimmt keine Schlüssel automatisch auf.
- Die stabile `client_id` wird beim ersten Einrichten als `client:` plus Token-ID des ersten Public Keys gebildet. Sie wird beim [kontrollierten Schlüsselwechsel](key-rotation.md) beibehalten; der neue Token-Fingerprint identifiziert nur den neuen Schlüssel. Ein Anzeigename ist keine Eigentumskennung. Ein neues, unabhängig registriertes Schlüsselpaar erhält eine eigene Identität. Die Bindung eines neuen Schlüssels an eine vorhandene Identität erfordert den autorisierten Wechsel oder eine ausdrückliche Betreiberentscheidung.
- Client-IDs gelten zunächst im Namensraum des jeweiligen Services. Sie sind keine automatisch netzweit eindeutigen Identitäten. Die ACL gehört dem Topic-Owner. Auch über Zwischeninstanzen authentifiziert sich der Client selbst beim Owner; keine Delegation anhand einer Peer-Behauptung. Der Owner führt Challenge und Proof in einer vermittelten logischen Sitzung aus; siehe [Owner-Authentifizierung](topic-routing.md).

## Anmeldung je Verbindung

Der Service wird vom Client zuerst über TLS authentifiziert; vorgesehen ist `wss` mit Zertifikatsprüfung oder bewusst konfiguriertem Server-Pin. Die Client-Signatur ersetzt weder Transportverschlüsselung noch die Prüfung des Servers. Für den ersten Entwurf gibt es keinen automatischen Fallback auf unverschlüsseltes WebSocket. Als ausdrücklich gewählter lokaler Transport ist WebSocket über einen Unix-Socket ohne TLS implementiert: geschützter Pfad, Socket-Modus 0600 und Peer-UID-Prüfung ersetzen dort die Zertifikatsprüfung. Keypair-Handshake und ACL bleiben erforderlich. Details stehen in [operating guide](getting-started.md). Keine Anmeldung oder Commands als TLS-0-RTT-Daten zulassen.

Das [Auth-Schema](../protocol/v1/client-auth.schema.json) beschreibt diese Frames. Die [Beispiele](../protocol/v1/client-auth-examples.json) enthalten einen signierten Ablauf mit ausschließlich öffentlichem Schlüssel und Testdaten. Anwendungsframes bleiben im [Client-Schema](../protocol/v1/frame.schema.json) definiert.

| Typ | Richtung | Pflichtfelder zusätzlich zu `version` und `type` |
| --- | --- | --- |
| `auth_hello` | Client → Service | `id`, `token_id` |
| `auth_challenge` | Service → Client | `request_id`, `service_id`, `nonce` |
| `auth_proof` | Client → Service | `id`, `auth_id`, `signature` |
| `auth_welcome` | Service → Client | `request_id`, `client_id`, `service_id` |
| `error` | Service → Client | `code`, `detail`; optional `request_id` |

1. Nach Aufbau des gewählten geschützten Transports und WebSockets sendet der Client genau ein Hello. Der Service sucht einen aktiven, vorab registrierten Public Key zu dessen Token-ID.
2. Der Service erzeugt eine kryptografisch zufällige 32-Byte-Nonce und speichert sie zusammen mit Verbindung, Hello-ID, Schlüssel, erwarteter Identität und Frist. `auth_challenge.request_id` referenziert die Hello-ID. `nonce` ist Standard-Base64 mit Padding.
3. Der Client prüft die Zuordnung zur eigenen Hello-ID und die erwartete `service_id` zusätzlich zur TLS-Serverprüfung. Er signiert die unten definierten Bytes mit seinem privaten Schlüssel. `auth_proof.auth_id` ist die Hello-ID; `id` ist eine neue Command-ID.
4. Der Service prüft den Zustandsbezug, die Frist, den weiterhin zugelassenen Schlüssel und dessen Signatur. Die Challenge wird bei genau einem Proof-Versuch verbraucht, auch bei Fehlern. Es darf nur eine Challenge pro Verbindung ausstehen.
5. Bei Erfolg bindet der Service die Verbindung an die konfigurierte Client-ID und sendet Welcome mit der Proof-ID als `request_id`. Der Client prüft Request-ID, Service-ID und die erwartete Client-ID. Erst danach sind Anwendungsframes zulässig. Es wird kein weiteres Session-Geheimnis ausgegeben; die authentifizierte WebSocket-Verbindung ist die Sitzung.

### Signierte Bytes

Damit keine JSON-Kanonisierung nötig ist, signiert der Client mit gewöhnlichem Ed25519 exakt diese Konkatenation:

```text
ASCII("mynofy-client-auth-v1") || 0x00
|| nonce_raw_32_bytes
|| public_key_raw_32_bytes
|| u32_big_endian(length(service_id_utf8)) || service_id_utf8
```

`service_id` ist dieselbe stabile Kennung wie im Service-Handshake, maximal 256 UTF-8-Bytes. Der Client baut die Bytes aus den geprüften Feldern selbst auf; er signiert keinen beliebigen, vom Service gelieferten Datenblock. Die Signatur wird als Standard-Base64 mit Padding übertragen. Die Version ist über den festen Präfix gebunden. Nonce und Verbindungskontext verhindern das Wiederverwenden eines Proofs in einer anderen Sitzung. Der Client muss außerdem die Service-ID anhand seiner lokalen Zielkonfiguration erwarten; die Behauptung im Challenge-JSON allein authentifiziert den Service nicht.

### Fristen und Fehler

Vorgesehener Standard sind fünf Sekunden für den gesamten Auth-Handshake ab WebSocket-Aufbau; die konkrete Konfiguration bleibt festzulegen. Unbekannte oder gesperrte Tokens, falsche Signaturen und nicht passende Challenges ergeben ein generisches `unauthorized` und Verbindungsende. Strukturfehler ergeben `invalid_frame`, falsche Versionen `unsupported_version`. Commands vor Anmeldung, erneutes Hello oder Proof im falschen Zustand beenden die Verbindung. Bei Fristende kann der Service `auth_timeout` senden und schließt die Verbindung.

TLS-Fehler erzeugen keine JSON-Antwort. Rate-Limits gelten pro stabiler Client-ID gemeinsam über alle authentifizierten Verbindungen, mit eigenem Budget für Schlüsselwechsel. Vor Authentifizierung gelten zusätzliche Quellen-/Listener-Limits, weil eine behauptete Identität nicht vertrauenswürdig ist. Testwerte stehen in der [Konfiguration](configuration.md); Überschreitung ergibt `rate_limited` mit `retry_after_ms`. Nach einem Reconnect wird immer neu authentifiziert. Ein aufgezeichneter Proof ist kein wiederverwendbares Token.

## Service-ACL

Authentifizierung bestimmt **wer**, ACL bestimmt **was**. Ohne passende Freigabe ist eine Operation verboten. Für den Prototyp gibt es nur exakte Topic-Regeln, keine impliziten Präfixe, Wildcards, Regelreihenfolge oder Admin-Rechte durch Token-Besitz. Pro Client-ID und Topic gibt es eine Regel; doppelte Regeln und unbekannte Aktionen sind Konfigurationsfehler.

| Aktion | Erlaubt |
| --- | --- |
| `publish` | Neue Nachrichten auf dem Topic veröffentlichen, einschließlich bewusster Wiederholungen. |
| `subscribe` | Aktive Nachrichten des Topics per `poll` abfragen. Keine dauerhafte serverseitige Anmeldung oder spontane Zustellung. |
| `inspect_own` | Kompatibilitätswert für bestehende ACLs. Eigene aktive Nachrichten sind über die authentifizierte Producer-Identität lesbar. |

Die [Konfigurationsvorlagen](configuration.md) enthalten Public Keys und Topic-ACLs für lokale, zentrale und Edge-Szenarien. ACL-Regeln stehen beim jeweiligen Owner unter dessen Topics; eine Edge-Route besitzt keine zweite ACL für dasselbe Remote-Topic. Dynamische Workflow-Topics benötigen zunächst exakte konfigurierte Regeln. Präfixe und Wildcards sind keine implizite Erweiterung.

Ein authentifizierter Client kann seine aktuell erlaubten Aktionen mit `get_permissions` abfragen. Die [Berechtigungsauskunft](topic-routing.md) kommt vom Topic-Owner und ist keine Berechtigung für spätere Commands; der Owner prüft jede Operation erneut.

- `publish`, `poll` (ACL-Aktion `subscribe`) und Statusabfragen prüfen die aktuell wirksame ACL. Publish-Recht allein erteilt kein Subscribe- oder Inspect-Recht. Die Producer-ID einer neuen Nachricht kommt aus der authentifizierten Client-ID.
- `get_message` und `list_messages` erlauben weiterhin authentifizierten Clients das Lesen ihrer eigenen ursprünglichen aktiven Nachrichten, auch nach Verlust der Topic-Rechte. Eigentum allein erlaubt keine fremden Inhalte, Antworten anderer Producer, neuen Publishes oder erneuten Publishes. Fremde, unbekannte und abgelaufene IDs ergeben einheitlich `not_found`. Topic-Filter begrenzen die eigenen Inhalte und erfordern dafür kein Subscribe-Recht.
- Eine fachliche Bestätigung läuft als Anwendungsereignis über ein vereinbartes Topic. Publish bestätigt die Annahme der Nachricht durch den Owner. Poll liefert Historie zum Lesen aus.
- Der einfache Entwurf schließt bei Entzug eines Schlüssels dessen aktive Sitzungen. Eine ACL-Änderung schließt alle Sitzungen der betroffenen Client-ID, invalidiert deren Producer-Listen-Cursor und beendet laufende Commands. Es gibt keine dauerhaften Topic-Anmeldungen. Poll-Cursor sind keine Zugangsrechte und werden bei jeder neuen Abfrage erneut durch Authentifizierung und ACL ergänzt. Neue Anmeldung und Commands werden gegen den aktuellen Stand geprüft. Die Änderung wird mit der Command-Verarbeitung serialisiert: Bereits akzeptierte Arbeit kann wirksam bleiben; noch nicht freigegebene Poll-Antworten dürfen keine entzogenen Rechte nutzen.
- Schlüsselentzug und Session-Ende löschen keine aktiven Producer-Nachrichten. Eigentümer bleibt die Client-ID. Ein anderer weiterhin zugelassener Schlüssel derselben Identität kann sie mit den nötigen ACL-Rechten bis zum Ablauf einsehen. Bereits empfangene Inhalte können durch Rechteentzug nicht zurückgerufen werden.

Der [Schlüsselwechsel-Entwurf](key-rotation.md) beschreibt Gültigkeitsmetadaten und den bestätigten Wechsel per Shared Secret. Der clientseitige Ablauf ist implementiert; seine Frames stehen im [Protokoll](protocol.md). Gesperrte oder bereits abgelaufene Credentials benötigen eine neue Einrichtung.

Der Betreiber pflegt Schlüssel und ACL; hierfür gibt es zunächst keine Client-API. Ein Service als Producer nutzt eine eigene konfigurierte Client-Identität und einen eigenen Schlüssel, wenn er am Client-Eingang arbeitet. Seine Peer-TLS-Identität erteilt nicht automatisch Client-Rechte. Ein Tunnel darf sich nicht als beliebiger Client ausgeben; der Owner authentifiziert den ursprünglichen Client selbst; die Zwischeninstanzen vermitteln nur den Kanal. Die genaue Tunnel-Rahmung ist noch offen.

## Verbleibende Grenzen

Das Verfahren authentifiziert die Sitzung am Service. Es signiert keine einzelnen Nachrichten Ende zu Ende. Private-Key-Speicherung, Provisionierungswerkzeuge, der vollständige Wechselablauf, sichere Konfigurationsänderungen, konkrete Auth-Limits und die technische Umsetzung der Owner-Sitzung müssen vor produktivem Netzwerkbetrieb ausgestaltet werden. Der Entwurf ersetzt keine Sicherheitsprüfung der späteren Implementierung.

## Neustarterkennung

`auth_welcome` enthält zusätzlich den verpflichtenden U32-`restart_counter` aus dem Security-Speicher. Die bestehende `service_id` bleibt die stabile Identität. Ungleichheit gegenüber dem zuvor beobachteten Zähler löst im verwalteten Client den nächsten Poll ohne alten Cursor aus; bei U32-Umlauf gilt dieselbe Regel. Alte Gegenstellen ohne Feld werden vom aktuellen Auth-Client abgewiesen. Die optionale Protobuf-Repräsentation dient nur der Anwesenheitsprüfung. [SDK, CLI und Speicherregeln](restart-and-storage.md).
