# Topic-Owner, Berechtigungsabfrage und Routingtabelle

Projektentscheidung: **Der Topic-Owner ist der ACL-Holder und entscheidet über jede Topic-Operation.** Zwischeninstanzen übernehmen nicht die ACL eines Remote-Topics. Der Entwurf ergänzt den [Service-Handshake](service-links.md); CA/mTLS-Verbindungen und beim Owner terminierende Datentunnel sind [implementiert](service-network.md).

## Eigene Berechtigungen abfragen

Ein angemeldeter Client kann `get_permissions` mit `id` und einem exakten `topic` senden. Die Antwort `permissions` enthält `request_id`, `topic`, `owner_id`, `acl_revision` und `actions` aus `publish`, `subscribe`, `inspect_own`. Die Client-Identität stammt aus der authentifizierten Sitzung; es gibt kein vom Client frei setzbares Feld zur Abfrage fremder Identitäten.

Ein Owner kann eine leere Aktionsliste zurückgeben. Das fortbestehende Eigentumsrecht zum Lesen ursprünglicher eigener Inhalte ist davon getrennt und wird nicht durch eine fehlende Topic-Aktion aufgehoben. Eine Berechtigungsauskunft ist eine Momentaufnahme, kein übertragbares Capability-Token. Tatsächliches Publish, Poll und Statuszugriff werden erneut gegen die aktuelle Owner-ACL geprüft. Die Auskunft gewährt keine Topic-Verwaltung.

Ist der Owner nicht erreichbar, ist `owner_unavailable` vorgesehen. Die Zwischeninstanz darf in dieser Situation keine Freigabe erfinden oder anhand einer alten Auskunft neue Operationen autorisieren. Eine ACL-Revision dient zum Erkennen veralteter Auskünfte; sie ersetzt nicht die Prüfung durch den Owner.

## Service-to-Service-Abfrage

Nach gegenseitiger Authentifizierung können Dienste eine Route zum Topic abfragen und daraus eine Tabelle aufbauen. Vorgesehene Frames:

| Typ | Richtung | Felder zusätzlich zu `version`, `type` |
| --- | --- | --- |
| `route_query` | Dienst → Peer | `id`, `topic`, `visited`, `hop_limit` |
| `route_info` | Peer → Dienst | `request_id`, `topic`, `owner_id`, `path`, `expires_at` |

`visited` nennt bereits besuchte Dienste. Ein Dienst verwirft einen zyklischen Pfad; jede Weiterleitung reduziert das positive `hop_limit`. Der letzte `visited`-Eintrag muss dem unmittelbar anfragenden, authentifizierten Peer entsprechen. Es gibt zunächst höchstens einen konfigurierten Upstream und keinen Broadcast an beliebige Dienste. Bei `hop_limit: 1` darf nur lokal geantwortet, nicht weitergeleitet werden; ohne lokale Zuständigkeit scheitert die Auflösung. Ist der angefragte Dienst selbst Owner, antwortet er mit einem Pfad, der nur ihn enthält. Andernfalls kann er die Abfrage über den konfigurierten nächsten Hop weiterreichen und seinen Dienstnamen dem zurückgegebenen Pfad voranstellen. Eine Zwischenspeicherung verlängert die erhaltene Ablaufzeit nicht.

Beispiel: Arbeitsplatz → Firmendienst → Zentrale. Der Arbeitsplatz lernt für `team/events`: Owner `central`, nächster Hop `office`, Pfad `[office, central]`. Er schickt seine Topic-Operation später an `office`; die ACL-Entscheidung gehört weiterhin zu `central`.

Der erste Pfadknoten muss der antwortende, authentifizierte Peer sein, der letzte der angegebene Owner. Wiederholte Knoten oder ein Pfad zurück zum anfragenden Dienst sind unzulässig. Eine Route wird nur über konfigurierte vertrauenswürdige Peers und innerhalb freigegebener Topic-Zuständigkeiten gelernt. Ein Peer kann sich durch die Antwort nicht eigenmächtig zum Owner beliebiger Topics erklären. Statisch konfigurierte Zuständigkeiten sind maßgeblich; widersprüchliche Owner-Angaben ergeben `route_conflict`, keinen automatischen Wechsel.

## Tabelle im RAM

| Feld | Zweck |
| --- | --- |
| `topic` | Exakter Routing-Schlüssel. |
| `owner_id` | Verantwortlicher Dienst und ACL-Holder. |
| `next_hop` | Unmittelbar erreichbarer, authentifizierter Peer. |
| `path` | Gelernter Pfad für Konsistenz- und Schleifenprüfung. |
| `learned_from` | Peer, aus dessen Antwort die Route stammt. |
| `expires_at` | Ende der Gültigkeit des gelernten Eintrags. |

Eigene Topics werden direkt lokal aufgelöst. Remote-Tabellen sind befristete Wegweiser, keine Kopie der wirksamen Autorisierung. Bei Ablauf oder Peer-Abbruch werden betroffene Routen verworfen beziehungsweise vor neuer Nutzung erneut aufgelöst. Ein Neustart lädt nur statische Ausgangsrouten aus der [Konfiguration](configuration.md). Keine lokale Übernahme bei Owner-Ausfall.

Eigene Berechtigungsauskünfte können getrennt als Hinweis mit Topic, Client-ID, Owner und ACL-Revision im RAM gehalten werden. Sie gehören nicht in eine Routingzeile für alle Clients: Verschiedene Clients haben auf demselben Weg unterschiedliche Rechte.

## Client authentifiziert sich beim Topic-Owner

Projektentscheidung: Der Topic-Owner prüft den Client selbst. Es gibt keine delegierte Client-Authentifizierung durch Zwischeninstanzen. Eine vom Peer behauptete `client_id` oder dessen Service-Zertifikat genügt nicht.

```text
Client ── vermittelter Kanal über A, B, … ── Topic-Owner
       Auth-Challenge und Proof enden beim Owner
```

1. Die Routentabelle bestimmt den Weg zum konfigurierten Owner. Zwischenstationen öffnen beziehungsweise vermitteln den Kanal dorthin.
2. Der Client prüft die erwartete Owner-Identität. Vorgesehen ist eine beim Owner terminierende TLS-Verbindung innerhalb des vermittelten Kanals; die TLS-Verbindungen zwischen benachbarten Diensten bleiben davon getrennt. Die konkrete [Kanal-Rahmung](service-network.md#drahtformat-und-grenzen) ist implementiert.
3. Der Owner führt den bestehenden [Client-Keypair-Handshake](client-auth.md) aus: Er erzeugt die Challenge und prüft den Proof gegen seinen registrierten Client-Public-Key. Zwischenstationen leiten den Austausch weiter, erzeugen aber weder Proof noch Client-ID-Zuordnung im Namen des Clients.
4. Erst nach Erfolg bindet der Owner diese logische Client-Sitzung an die stabile Client-ID. Topic-Commands, Producer-Abfragen, ACK/NACK und Schlüsselwechsel laufen auf dieser authentifizierten Sitzung. Eine Sitzung gehört genau zu einem Client und einem Owner; eine gemeinsam genutzte Peer-Verbindung darf diese Zuordnung nicht vermischen.
5. Der Owner prüft seine aktuelle ACL bei jeder Operation. Eine lokale Anmeldung an einer Zwischeninstanz oder eine zwischengespeicherte Berechtigungsauskunft ersetzt dies nicht. `get_permissions` für ein Remote-Topic wird ebenfalls in der Owner-Sitzung ausgeführt.

Der Owner braucht den erlaubten Client-Public-Key und dessen stabile Identitätszuordnung in seiner eigenen Registrierung. Ein unbekannter Schlüssel wird nicht automatisch durch eine Registrierung auf einer Zwischeninstanz zugelassen. Bei mehreren Ownern authentifiziert sich der Client bei jedem angesprochenen Owner; deren ACLs sind unabhängig.

Der vermittelte Kanal muss Commands und Antworten nach der Authentifizierung gegen Austausch schützen. Nur eine gültige Signatur während der Anmeldung und anschließend frei vom Zwischenknoten zusammengesetzte Commands wären nicht ausreichend. Die vorgesehene innere TLS-Sitzung erfüllt diese Trennung im Architekturentwurf; sie ist implementiert. Es werden dadurch keine zusätzlichen Signaturfelder für einzelne Nachrichten beschlossen.

Owner-Rate-Limits sind an dessen authentifizierte Client-ID gebunden. Zwischenstationen begrenzen zusätzlich Tunnel-/Peer-Ressourcen, ohne fremde Topic-Rechte zu entscheiden. Nach Abbruch der Owner-Sitzung wird beim Owner neu authentifiziert. Ein bloßer Routenwechsel überträgt keine authentifizierte Sitzung oder Peer-Identität auf einen anderen Owner. Topic-Poll-Cursor bleiben Owner-gebunden; Zwischenstationen schreiben Positionen, Ablauf und Producer-Eigentum nicht um.

Die Tunnel-Rahmung, logische Kanalzuordnung und der Verbindungsabbau sind implementiert. ACK/NACK laufen innerhalb der authentifizierten Owner-Sitzung und benötigen dadurch keine globale ID oder fremde Client-Identitätsbehauptung. Routen-/Peer-Konfiguration bleibt statisch; neue lokale Topics werden nicht automatisch angekündigt.

## Proxy-Verhalten und Owner-spezifische Registrierung

Weitere Projektentscheidung: Zwischeninstanzen arbeiten als Proxy. Ein Verbindungs-Timeout beendet den betroffenen Kanal und dessen Owner-Sitzung. Es gibt durch den Proxy weder automatische Wiederholung von Commands noch Wiederaufnahme einer abgebrochenen Sitzung oder nachträgliches Abrufen verlorener Antworten. Ausstehende Client-Aufrufe haben gegebenenfalls einen unklaren Ausgang; ein Timeout beweist nicht, dass der Owner einen Publish nicht angenommen hat.

Der Client kann eine neue Verbindung aufbauen und sich erneut beim Owner authentifizieren. Das ist eine neue Sitzung, keine Proxy-Wiederaufnahme. Der normale Client-Poll mit einem noch gültigen Owner-Cursor und ein bewusstes erneutes Publish bleiben die bereits definierten Client-Operationen. Der Proxy erzeugt sie nicht selbst.

Client-Registrierung, Schlüsselgültigkeit, Widerruf und Schlüsselwechsel gelten jeweils pro Owner. Ein Wechsel bei A verändert weder Registrierung noch ACL bei B. Ein Client verwaltet seine aktuell gültigen Schlüssel deshalb nach Owner getrennt; ein Wechsel bei A darf nicht versehentlich den bei B noch benötigten privaten Schlüssel überschreiben. Eine globale Schlüsselsynchronisierung gehört nicht zum Prototyp.

Diese Regeln sind konzeptionell entschieden. Proxy-Frames, Deadlines und begrenzte Kanalverwaltung sind implementiert; Einzelheiten stehen im [Betriebsleitfaden](service-network.md).


Die gelernte Tabelle ist aktuell eine befristete Diagnose; jeder neue Client-Kanal prüft den Weg erneut per `route_query`. `admin status.service_links` zeigt Owner, nächsten Hop, Quelle, Pfad und `expires_at_ms`. Ein gewähltes Topic öffnet eine Owner-Sitzung, keine zusätzliche Topic-ACL innerhalb des verschlüsselten Kanals; die Peer-Freigabe beschränkt Routensuche/Kanalöffnung, der Owner autorisiert die tatsächlichen Commands.
