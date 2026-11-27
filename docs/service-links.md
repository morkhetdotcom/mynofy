# Verbindungen zwischen mynofy-Diensten

CA-basierter Handshake, Routensuche und begrenzte Datentunnel sind implementiert. [Ausführbares Beispiel, Drahtformat und Betriebsgrenzen](service-network.md). SPKI-Pinning bleibt ein Entwurf. Eine Instanz kann lokale Topics selbst verwalten und konfigurierte Remote-Topics über einen anderen Dienst vermitteln. Ihre Rolle ist damit je nach Topic unterschiedlich. Für den ersten Ausbau ist höchstens ein Upstream pro Instanz vorgesehen; eine Instanz kann zugleich mehrere untergeordnete Dienste bedienen.

Die [security model](security.md) beschreibt offene Routing-, Identitäts- und Betriebsfragen. Ein Upstream verhindert noch keinen Zyklus. Empfohlen ist zunächst ein statischer, azyklischer Baum ohne automatische Übernahme von Remote-Topics. Statische Owner-Bindung, Pfad-/Hop-Prüfung, ownergebundene Bestätigungen und begrenzter Verbindungsabbau sind inzwischen implementiert. [Live-Peer-Verwaltung und lokaler Zertifikatswiderruf](runtime-peers.md) sind implementiert; CA-/TLS-Datei-Reload und Upstream-Adressänderung bleiben außerhalb dieses Ausbaus. Der Handshake allein ersetzt weder Owner-Authentifizierung noch Routingprüfung. Die [Topic-Routensuche](topic-routing.md) ergänzt nun befristete Tabellen, Schleifengrenzen und den allein entscheidenden ACL-Holder beim Owner; die [Konfiguration](configuration.md) liefert die Ausgangsszenarien.

Pro Topic gibt es genau einen verantwortlichen Dienst. Er verwaltet aktive Nachrichten, Producer-Zuordnung, Zustellstatus und den wirksamen Ablaufzeitpunkt. Wenn der Producer keine Zeit vorgibt, bestimmt seine Topic-Policy den Ablauf. Zwischeninstanzen vermitteln Topic-Polls, deren Antworten und Bestätigungen; der Transport zwischen Diensten ist kein bestätigter Endempfang. Lokale Topics bleiben bei einem Upstream-Ausfall nutzbar, Remote-Topics sind vorübergehend nicht erreichbar. Eigene ACLs und statische Topic-Zuständigkeiten stammen aus der Konfiguration. Nach dem Handshake können konfigurierte vertrauenswürdige Peers befristete Next-Hop-Routen lernen. Diese Tabelle verändert weder Topic-Eigentum noch ACL-Rechte.

## Zwei Schritte zur Verbindung

1. **Authentifizieren:** TLS prüft die Identität beider Gegenstellen und den Besitz ihrer privaten Schlüssel.
2. **Registrieren:** Auf der authentifizierten WebSocket-Verbindung identifizieren sich die Dienste mit `service_hello` und `service_welcome`. Beide gleichen die angegebene `service_id` mit ihrer lokalen Zuordnung zur kryptografischen Identität ab. Die Registrierung verlangt auf beiden Seiten `link_version: 2`.

Der [Client-Keypair-Handshake](client-auth.md) und die Client-ACL sind hiervon getrennt: Ein Peer-Zertifikat erteilt keine Client- oder Producer-Rechte. Der ursprüngliche Client authentifiziert sich selbst beim Topic-Owner; Zwischeninstanzen vermitteln den Kanal und dürfen keine Client-Identität delegieren. Die [Owner-Sitzung](topic-routing.md) ist vom Peer-Handshake getrennt.

Ein im JSON behaupteter Name oder Fingerprint ist kein Identitätsnachweis. Ein erfolgreicher Handshake erlaubt nur die konfigurierte Verbindung; er erteilt keine pauschalen Publish-, Subscribe- oder Routingrechte.

## Vertrauensmodelle

Beide Varianten verwenden gegenseitige TLS-Zertifikatsauthentifizierung; sie unterscheiden sich darin, wie Zertifikaten vertraut wird. TLS bleibt außerhalb des JSON-Protokolls. TLS 1.3 beschreibt den Nachweis des privaten Schlüssels über `CertificateVerify` in [RFC 8446, Abschnitt 4.4.3](https://www.rfc-editor.org/rfc/rfc8446.html#section-4.4.3).

| Modus | Vertrauen | Einsatz |
| --- | --- | --- |
| `ca` | Beide Seiten prüfen Zertifikatskette, Gültigkeit, zulässige Verwendung und konfigurierte Peer-Identität gegen eine vertrauenswürdige CA. | Verwaltete Netze mit eigener Zertifikatsausstellung. |
| `pinned_key` (geplant) | Beide Seiten prüfen einen vorab konfigurierten SHA-256-Fingerprint des öffentlichen Schlüssels im Peer-Zertifikat. Selbstsignierte Zertifikate sind möglich; TLS-Signaturprüfung, Gültigkeit und zulässige Verwendung bleiben erforderlich. | Kleine Installationen ohne gemeinsame CA. |

Für `pinned_key` ist das lokale Format `SHA256:<Base64>` vorgesehen: SHA-256 über die DER-kodierte SubjectPublicKeyInfo-Struktur des Blattzertifikats, Base64 mit Padding. Die SPKI-Fingerprint-Konstruktion ist in [RFC 7469, Abschnitt 2.4](https://www.rfc-editor.org/rfc/rfc7469.html#section-2.4) beschrieben; dessen HTTP-Header-Protokoll wird hier nicht verwendet. Der Fingerprint wird aus dem tatsächlich im TLS-Handshake geprüften Zertifikat berechnet, nicht aus JSON übernommen. Derselbe Schlüssel behält bei einer Zertifikatserneuerung denselben Fingerprint.

Die Konfiguration bindet eine stabile `service_id` an die erlaubte Zertifikatsidentität beziehungsweise den Pin. IDs müssen innerhalb des Kommunikationsnetzes eindeutig sein und über Neustarts erhalten bleiben. Zertifikate und private Schlüssel sind lokale Dateieingaben. Routingzustand bleibt im RAM; Topic-/Content- und Security-Speicherung folgen ihren getrennten Regeln.

Der implementierte Prototyp verwendet ausschließlich `ca`; Konfiguration mit `pinned_key` wird abgewiesen. Für einen späteren Pin-Modus würden Pins vorab auf beiden Seiten eingetragen. Eine spätere SSH-artige Einrichtung kann beim Erstkontakt den Fingerprint zur Prüfung anzeigen und nach ausdrücklicher Bestätigung lokal speichern. Bis dahin wird keine Service-Verbindung freigegeben. Ein unbekannter oder geänderter Schlüssel wird nicht automatisch akzeptiert. Schlüsselwechsel benötigen eine bewusste Aktualisierung der Zuordnung; ein anderer Vertrauensmodus wird bei Fehlern nicht automatisch ausprobiert.

## JSON-Handshake

Das separate [Peer-Schema](../protocol/v1/service-frame.schema.json) beschreibt die Handshake-Frames. Sie sind keine Topic-Nachrichten und gelten ausschließlich auf Verbindungen zwischen Diensten. Normale Clients brauchen diesen Handshake nicht.

| Typ | Richtung | Pflichtfelder zusätzlich zu `version` und `type` |
| --- | --- | --- |
| `service_hello` | Verbindungsinitiator → Gegenstelle | `id`, `service_id`, `link_version` |
| `service_welcome` | Gegenstelle → Verbindungsinitiator | `request_id`, `service_id`, `link_version` |
| `error` | Gegenstelle → Verbindungsinitiator | `code`, `detail`; optional `request_id` |

Beispiel nach erfolgreichem TLS-Handshake:

```json
{"version":1,"type":"service_hello","link_version":2,"id":"hello-1","service_id":"workstation-a"}
```

```json
{"version":1,"type":"service_welcome","link_version":2,"request_id":"hello-1","service_id":"office-hub"}
```

`request_id` muss dem `id` des Hello entsprechen. `version: 1` bezeichnet die akzeptierte Protokollversion; eine andere Version wird abgewiesen. Es gibt zunächst keine Versionsaushandlung, Rollenmeldung, automatische Topic-Ankündigung oder globale Dienstregistrierung.

## Zustände und Fehler

- Vor dem JSON-Handshake muss TLS beide Gegenstellen erfolgreich authentifiziert haben. TLS-Fehler beenden den Verbindungsaufbau ohne JSON-Antwort.
- Der Initiator sendet genau ein Hello. Die Gegenstelle prüft Version, Struktur, Identitätszuordnung und lokale Freigabe. Sie weist die eigene Service-ID sowie eine bereits aktive zweite Verbindung derselben Peer-ID ab.
- Bei Annahme sendet sie Welcome. Der Initiator prüft dessen Struktur, Version, `request_id` und die Bindung der Peer-ID an das TLS-Zertifikat. Erst dann gilt die Verbindung auf seiner Seite als registriert. Die Gegenstelle kann direkt nach Welcome Folgeframes senden; sie werden beim Initiator erst nach erfolgreicher Welcome-Prüfung verarbeitet.
- Der Handshake muss innerhalb eines konfigurierten Zeitlimits abgeschlossen sein; vorgeschlagener Standard sind fünf Sekunden nach WebSocket-Aufbau. Vor Abschluss sind nur Handshake-Frames zulässig. Wiederholtes Hello und Topic-Frames vor Abschluss sind Protokollfehler.
- Ein Fehler führt zum Verbindungsende. Nach Möglichkeit wird vorher `error` gesendet. Codes sind `invalid_frame`, `unsupported_version`, `identity_mismatch`, `peer_not_allowed`, `duplicate_service`, `handshake_required`, `handshake_timeout` und `internal_error`. Details enthalten keine Schlüssel oder internen Vertrauenslisten.
- Registrierung gilt nur für die aktuelle Verbindung. Beim Abbruch werden deren aktive Registrierung und verbindungsbezogene Zustände entfernt. Vertrauenszuordnungen und Topic-Konfiguration bleiben erhalten. Ein Reconnect durchläuft TLS und Registrierung erneut; eine alte Verbindung gilt erst nach ihrer Beendigung als ersetzt.

## Nachrichtenrouting und Endempfang

Die [Routensuche](topic-routing.md) und [implementierten Tunnel-Frames](service-network.md) verbinden konfigurierte Owner. Eine innere TLS-Sitzung schützt die Client-Kommunikation bis zum Owner. Kanalnummern identifizieren nur lokale Transportströme; sie sind keine Message- oder Bestätigungs-IDs. Jeder Client authentifiziert sich eigenständig beim Owner. Der Owner führt weiterhin Nachricht/Client-Paare, auch wenn viele Clients denselben Peer-Link benutzen. Ein Hop-Transport ersetzt keinen Empfänger-ACK und überträgt kein Producer-Eigentum.


Der reguläre Dienststopp bricht einen laufenden gemeinsamen Upstream-Aufbau
ab und wartet auf dessen Task sowie auf den Upstream-Manager. Das gilt auch
bei weiterlaufender Runtime eines eingebetteten Owners.
