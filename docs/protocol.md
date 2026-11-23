# Protokoll v1 – Pull-Prototyp

Der lokale Owner-Prototyp implementiert dieses Pull-Protokoll. Das Wire-Format bleibt veränderlich; Peer-/Proxy-Tunneling ist noch Entwurf. [operating guide](getting-started.md). Der Client pollt ein Topic und nennt seine zuletzt erreichte Position. Der Service hält Nachrichten einmal pro Topic vor und beantwortet konkrete Abfragen; er erzeugt bei Publish keine Warteschlange oder Zustellung pro Subscriber.

Das [Schema](../protocol/v1/frame.schema.json), der [Agenten-Ablauf](../protocol/v1/examples.json), die [Polling-Beispiele](../protocol/v1/poll-examples.json) und das [Producer-Status-Beispiel](../protocol/v1/status-examples.json) gehören zusammen. Die Konzeptprüfungen dokumentieren auch frühere, inzwischen ersetzte Push-Annahmen.

## Umfang und Verbindung

Ein Topic-Owner ist zugleich der ACL-Holder. Er verwaltet aktive Nachrichten im RAM, einen geordneten Topic-Index und dessen höchsten Annahmestand. Der Prototyp verwendet binäre Protobuf-Frames über authentifiziertes WSS oder explizit konfigurierte lokale Unix-Sockets, exakte Topic-Namen, sofort beantwortetes Polling, zeitbasierten Ablauf und eigene Producer-Abfragen. Optionale Content-Persistenz ist implementiert. Serverseitiges Long-Polling und Scheduling sind nicht geplant; spontane Push-Zustellung, automatische Retries, Wildcards und exklusive Job-Vergabe sind nicht enthalten. Eine spätere REST-Anbindung kann dieselben Operationen abbilden.

Ein Protobuf-Frame pro binärer WebSocket-Nachricht. JSON-Textcommands werden mit Code 1003 abgewiesen. Die folgenden Beispiele zeigen die lesbare CLI-/SDK-Darstellung: Jedes JSON-Objekt hat `version: 1` und `type`. Zusätzliche Felder und unbekannte Typen werden abgewiesen. Optionale Felder entfallen statt `null`. IDs sind opake, nicht leere Strings; Command-IDs sind pro Verbindung eindeutig. Message- und Delivery-IDs sind pro Owner-Prozesslauf eindeutig. Ein Topic ist eine nicht leere, groß-/kleinschreibungssensitive Zeichenfolge; `/` hat keine eingebaute Hierarchie- oder Wildcard-Semantik.

Vor Anwendungsframes muss der [Client-Keypair-Handshake](client-auth.md) auf derselben Verbindung abgeschlossen sein. Der Owner bindet Pubishes an die daraus bestimmte stabile Client-ID. Die ACL-Aktion `subscribe` bedeutet in diesem Modell die Berechtigung, ein Topic zu pollen. Es gibt kein `subscribe`-/`unsubscribe`-Command und keine dauerhaft registrierte Topic-Anmeldung. Alle Anwendungsbeispiele beginnen nach erfolgreicher Authentifizierung.

Die [Konfigurationsdatei](configuration.md) liefert beim Start Schlüssel, ACL, Topic-Policies, Routen und Rate-Limits. Laufzeit-Zustand bleibt im RAM und wird nicht automatisch zurückgeschrieben. Die [Berechtigungs-/Routingbeschreibung](topic-routing.md) trennt die Entscheidung des Owners von zwischengespeicherten Wegweisern.

## Frames

Alle Felder sind Pflichtfelder, sofern nicht als optional bezeichnet. `version` und `type` kommen jeweils hinzu.

| Typ | Richtung | Felder | Bedeutung |
| --- | --- | --- | --- |
| `publish` | Producer → Owner | `id`, `topic`, `payload`; optional `expires` | Neue Nachricht erzeugen. |
| `result` | Owner → Client | `request_id`; bei Publish zusätzlich `message_id`, `expires` | Command erfolgreich angenommen/ausgeführt. |
| `poll` | Client → Owner | `id`, `topic`; optional `after`, `limit` | Aktive Nachrichten nach einer Position abfragen. |
| `messages` | Owner → Client | `request_id`, `items`, `next_cursor`, `has_more` | Begrenzte Poll-Antwort, auch leer. |
| `list_messages` | Producer → Owner | `id`; optional `topic`, `limit`, `cursor` | Eigene aktive Nachrichten auflisten. |
| `messages_page` | Owner → Producer | `request_id`, `items`; optional `next_cursor` | Seite eigener Nachrichtenübersichten. |
| `get_message` | Producer → Owner | `id`, `message_id` | Eigene aktive Nachricht samt Payload abfragen. |
| `message_status` | Owner → Producer | `request_id`, `message`, `created_at`; optional `deliveries` | Eigener Inhalt; technische Zustellzähler nur mit Freigabe. |
| `get_permissions` | Client → Owner | `id`, `topic` | Eigene aktuelle Topic-Rechte abfragen. |
| `permissions` | Owner → Client | `request_id`, `topic`, `owner_id`, `acl_revision`, `actions` | Aktuelle Auskunft, kein Capability-Token. |
| `error` | Owner → Client | `code`, `detail`; optional `request_id` | Fehler ohne zugesicherte Verarbeitung. |

Abfrageantworten sind unmittelbar die Bestätigung des Commands, ohne zusätzliches `result`. Spontane `message`- oder `receipt`-Frames sind im Pull-Prototyp nicht vorgesehen; Producer fragen Status selbst ab.

## Topic-Polling mit Cursor

```json
{"version":1,"type":"poll","id":"poll-2","topic":"events","after":"00000002","limit":20}
```

```json
{"version":1,"type":"messages","request_id":"poll-2","items":[],"next_cursor":"00000002","has_more":false}
```

Poll-Cursor bestehen aus genau acht ASCII-Zeichen des Alphabets `0–9A–Za–z`, etwa `00000002`. Clients speichern den zurückgegebenen Wert pro Owner und Topic und senden ihn unverändert zurück.

- **Ohne `after`:** Ab Beginn des noch aktiven Topic-Bestands lesen. Auch zuvor bestätigte Nachrichten können wieder erscheinen. Dies ist eine bewusste neue Abfrage, kein serverseitiger Push beim Reconnect.
- **Mit `after`:** Aktive Nachrichten strikt nach der im Cursor enthaltenen Annahmeposition lesen. Die Client-Software speichert den zuletzt erfolgreich verarbeiteten Fortschritt und sendet ihn beim nächsten Poll wieder.
- **Antwort:** `items` sind aufsteigend nach Annahmeposition geordnet. Jeder Eintrag enthält `message`; es gibt keine Zustellungs-ID. Die fachliche Nachricht enthält `id`, `topic`, `payload` und den wirksamen Ablauf `expires.time`.
- **Seitengröße:** `limit` liegt zwischen 1 und 100, Standard ist 20. Zusätzlich gilt das konfigurierte Antwort-Byte-Limit. Weniger Einträge als `limit` sind zulässig. Ein einzelner akzeptierter Inhalt muss einschließlich sämtlicher Ausgabe-Metadaten in eine Antwort passen.
- **Fortschritt:** Der Owner bildet beim Poll einen konsistenten Annahmestand H. `next_cursor` liegt hinter den ausgegebenen oder übersprungenen Positionen. Nach Prüfung bis H ist `has_more: false` und der Cursor darf bis H vorrücken. Bei der [budgetierten Ablaufbereinigung](budgeted-expiration.md) werden höchstens 256 Positionen geprüft: Eine leere Seite mit `has_more: true` und fortgeschrittenem Cursor ist möglich. Das Flag garantiert dann keine weitere aktive Nachricht, sondern noch ungeprüfte Positionen. Die nächste Abfrage verwendet den erhaltenen Cursor.
- **Leere Antwort:** Es gibt keine aktuell aktiven neueren Nachrichten. Das bedeutet nicht, dass seit dem alten Cursor nie etwas veröffentlicht wurde: Nachrichten können bereits abgelaufen sein. Es gibt keine Vollständigkeitsgarantie für einen länger offline gewesenen Client.

### Quick-Check und geordneter Index

Der Topic-Owner vergibt intern fortlaufende Annahmepositionen und hält pro Topic den letzten Stand vor. Ein gültiger Cursor am aktuellen Stand ermöglicht nach Auth-/ACL-Prüfung einen Vergleich ohne Durchlaufen des Nachrichtenbestands: sofort leer antworten. Für Positionen hinter dem aktuellen Stand oder als ungültig erkannte Cursor beginnt der Service wieder am aktiven Anfang; Antwort- und Seitenlimits sowie Auth/ACL gelten unverändert.

Für einen älteren Cursor wird im geordneten Index direkt nach der Position angesetzt, statt jede aktive Nachricht zu prüfen. Ziel ist ein logarithmisches Aufsuchen plus Arbeit proportional zur gelesenen Seite; das ist eine Architekturabsicht, noch keine gemessene Performanceaussage. Ablaufbereinigung benötigt einen dazu passenden Zeitindex und darf nicht aus jedem leeren Poll einen vollständigen Scan machen.

Der Poll-Cursor ist ein einfacher Index ohne Signatur, MAC oder Bindung an einen Client. Die Topic-Auswahl kommt aus `topic`; Authentifizierung und aktuelle ACL werden bei jeder Abfrage separat geprüft. Ein Client darf bewusst eine andere gültige Position wählen oder eine Position mit einem anderen berechtigten Client teilen. Der Server speichert keinen Fortschritt pro Subscriber. Eine Message-ID ist kein Poll-Cursor: Positionen gelten pro Topic und bleiben auch bei abgelaufener Ankernachricht nutzbar.

Intern zählt der Owner pro Topic mit `u64`; auf dem Wire erscheinen die letzten acht alphabetischen Stellen. `00000000` bezeichnet zunächst den Stand vor der ersten Nachricht. Nach `zzzzzzzz` folgt `00000000`. Bei der Auswertung wählt der Owner die neueste passende Position, die nicht hinter seinem aktuellen Stand liegt. Ein nicht darstellbarer Rückstand vor Position 0 sowie ungültige Länge/Zeichen setzen den Poll-Start auf den aktiven Anfang zurück. Der aktuelle Cursor braucht keine Bestandssuche, ältere Positionen werden im geordneten Index gesucht.

**Grenze des kompakten Index:** Nach 62^8 Annahmen ist ein alter Index nicht mehr eindeutig. Clients dürfen ihren Fortschritt nicht über einen vollständigen Umlauf hinweg aufbewahren. Neustart und Topic-Neuanlage setzen Positionen zurück. Eine alte Position löst zunächst als zukünftig einen Rücksprung zum aktiven Anfang aus, kann aber nach Aufholen des neuen Bestands wieder gültig erscheinen. Der Cursor erkennt solche Neustarts nicht zuverlässig; nach einem bekannten Neustart muss die Anwendung bewusst ohne `after` neu beginnen. Keine Generationserkennung oder MAC-Garantie für Poll-Cursor behaupten. Vorherige lange Poll-Cursor lösen ebenfalls einen Rücksprung zum aktiven Anfang aus.

Producer-Listen-Cursor (`list_messages.cursor` / `messages_page.next_cursor`) bleiben davon getrennt: Sie tragen Snapshot-Wasserstand, Filter und Ablauf und sind weiterhin Base64URL(JSON) plus HMAC-SHA256, an Owner-Generation und Client gebunden. Dafür gilt weiterhin ein Limit von 4096 Bytes. Topic-Namen sind auf 256 UTF-8-Bytes begrenzt.

## Stateless polling and local progress

Poll reads active messages without creating reader state, confirmation pairs or delivery IDs. ACK/NACK commands are removed. Acceptance, rejection and completion are application events published to the agreed topics. See [upgrade notes](message-client-confirmation.md).

Der Client darf einen zurückgegebenen Cursor erst speichern, nachdem er die zugehörige Seite gemäß seiner Anwendung verarbeitet oder zuverlässig lokal übernommen hat. Speichert er vorher und stürzt ab, kann er Nachrichten überspringen. Speichert er erst danach und stürzt zuvor ab, kann dieselbe Arbeit erneut erscheinen. Bei mehreren parallelen Polls desselben Topics muss der Client seine Fortschrittsverwaltung serialisieren oder die Verarbeitung korrekt zusammenführen. Dieser Prototyp definiert keine atomare Kopplung aus Verarbeitung und lokalem Checkpoint.

Die [Polling-Beispiele](../protocol/v1/poll-examples.json) zeigen Seiten, leere Antworten, einen bereits abgelaufenen Anker und anschließend einen neuen Publish. Sofortige Polls sollten mit clientseitiger Pause/Backoff bei leerer Antwort kombiniert werden. Konkrete Intervalle und Limits sind noch festzulegen. Der Service hält Poll-Anfragen nicht bis zu einer neuen Nachricht offen; Warteintervalle und Wiederholungen liegen beim Client.

## Ablauf und Aufbewahrung

Der Producer kann `expires: {"time":"2026-12-01T12:00:00Z"}` setzen. Ohne Angabe bestimmt der Topic-Owner den Zeitpunkt aus seiner Topic-Policy mit endlicher Standard-TTL. Ungültige, bereits erreichte oder nach Policy zu lange Vorgaben werden abgewiesen, nicht stillschweigend gekürzt. Publish-Result, Poll-Ergebnis und eigene Abfragen enthalten immer den wirksamen Zeitpunkt. Standard-TTL und maximale Laufzeit werden pro Topic in der CFG gesetzt. Zeitauflösung ist Millisekunden.

Jede akzeptierte Nachricht bleibt bis zum Ablauf im RAM aktiv. Fehlende Leser löschen sie nicht. Policy-Änderungen verändern nicht rückwirkend akzeptierte Zeitpunkte. Maßgeblich ist die Owner-Uhr; ein absoluter RFC-3339-UTC-Zeitpunkt mit `Z` wird vorgeschrieben. Uhrabweichungen bleiben ein Betriebsrisiko.

Beim Ablauf wird der Inhalt sofort logisch ungültig. Die physische Bereinigung erfolgt budgetiert in mehreren Schritten. Quotenplätze werden erst bei tatsächlicher Bereinigung freigegeben. Bereits übertragene Inhalte können nicht zurückgerufen werden. Der Topic-Annahmestand bleibt erhalten, auch wenn alle Nachrichten abgelaufen sind. Topic-Neuanlage und ein Neustart ohne Content-Checkpoint setzen den Topic-Annahmestand zurück. Ein vollständiger Content-Restore erhält gespeicherte Topic-Kopfstände und entfernt abgelaufene Nachrichten vor Ausgabe. Ohne gespeicherten Stand wird der RAM-Bestand verworfen. Jeder Neustart erzeugt eine neue Owner-Generation für Producer-Listen-Cursor. Es gibt keine Historie abgelaufener Inhalte.

## Eigene aktive Nachrichten und Status

Aktiv bedeutet akzeptiert, noch nicht abgelaufen und seitdem nicht durch Neustart verloren. Producer – auch selbst publizierende Dienste – können ihre ursprünglichen eigenen aktiven Inhalte bei gültiger Anmeldung unabhängig von verbliebenen Topic-Rechten weiter lesen. Fremde Antworten bleiben separate Nachrichten und erfordern `subscribe` zum Pollen; erneutes Publish erfordert aktuelle Publish-Rechte.

`get_message` returns the original content and `created_at`. Foreign, unknown or expired IDs return `not_found`. `list_messages` returns the producer's own active messages, optionally filtered by exact topic, with `id`, `topic`, `created_at` and `expires`. Neither response contains reader or confirmation counters. These queries remain available to authenticated producers after losing topic rights.

Producer-Listen können wegen des Scanbudgets auch leere Seiten mit `next_cursor` liefern; allein die leere Eintragsliste ist kein Endekriterium. Producer-Listen verwenden ein eigenes Paging: `limit` 1–100, Standard 20, plus Antwort-Byte-Limit. Der Listen-Cursor bindet Identität, Topic-Filter, Seitengröße und einen Annahme-Wasserstand. Folgeseiten überspringen inzwischen abgelaufene Einträge und enthalten keine späteren Publishes. Zwischen Seiten können sich Statusdaten ändern. Topic-Filter und Seitengröße bleiben unverändert. Der Cursor einer Producer-Liste ist kein Topic-Poll-Cursor und umgekehrt. `next_cursor` entfällt bei Listenende. Producer-Listen-Cursor gelten im Prototyp 60 Sekunden und sind zusätzlich an die ACL-Revision gebunden.

## Bewusst erneut veröffentlichen

Ein Producer kann anhand eigener Abfragen entscheiden, per neuem `publish` erneut zu veröffentlichen. Dies erzeugt eine neue Message-ID und einen neuen Ablauf; die alte Nachricht bleibt aktiv. Es gibt keine Löschoperation, keinen automatischen Retry und kein `resend` unter derselben ID. Die fachliche Laufkennung kann für denselben Auftrag erhalten bleiben, damit die Anwendung Wiederholungen erkennt. Für einen unabhängigen Workflow ist eine neue Kennung nötig.

Abfrage, Retry und ein eventuell spätes Ergebnis sind nicht atomar. RAM-Aufbewahrung, Cursor und Status garantieren keine At-least-once-Verarbeitung oder Exactly-once-Wirkung. Bei verlorenem Publish-Result bleibt der Ausgang möglicherweise unklar. Die [Status-Beispiele](../protocol/v1/status-examples.json) zeigen bewussten Retry und Inhaltsabfrage nach Rechteentzug. Alle Uhrzeiten und Cursor der Beispiele sind illustrative Werte, keine unmittelbar ausführbare Konfiguration.

## Ressourcen und Vernetzung

Publish erzeugt einen aktiven Inhalt mit initialem Status, ohne Subscriber-Fan-out. Erst ein Poll erzeugt begrenzte Ausgabe und Zustellungsmetadaten. Clients pollen unabhängig voneinander; es gibt keine vorab gefüllten Sendewarteschlangen für alle möglichen Leser. N Inhalte und S tatsächliche vollständige Leser können weiterhin N × S Übertragungen benötigen, aber dieser Aufwand entsteht durch deren Abfragen und nicht vorab bei jedem Publish.

Begrenzt werden müssen aktive Bytes, Poll-Seitengröße/Antwortbytes, gleichzeitig offene Commands, tatsächlich offene Zustellversuche und Anfragehäufigkeit. Konfigurierbare Testwerte stehen in den Szenariodateien; sie sind noch keine produktiven Defaults. Annahme eines Publish samt Status und einer nicht leeren Poll-Seite samt Zustellversuchen müssen bei Kapazitätsmangel konsistent mit `resource_exhausted` scheitern. Ein langsamer Leser blockiert keine Warteschlange anderer Leser. Bei Rechteentzug sind noch nicht freigegebene Antworten gegen die aktuelle ACL abzusichern; bereits übertragene Bytes können nicht zurückgerufen werden.

[Peer-Identität und Handshake](service-links.md) bleiben vom Client-Protokoll getrennt. Owner-/Next-Hop-Auflösung und Berechtigungsauskunft sind in [topic-routing.md](topic-routing.md) beschrieben. Der ursprüngliche Client authentifiziert sich selbst beim Owner über eine vermittelte, Owner-terminierte Sitzung. The implemented tunnel carries the original client protocol to the owner without reader-confirmation state. Eine Zwischeninstanz darf sich durch Weiterleitung nicht zum Producer machen oder einen Hop als bestätigten Endempfang ausweisen. Globale Producer-Abfragen über mehrere Owner sind nicht Teil des Prototyps.

## Fehlercodes

| Code | Bedeutung |
| --- | --- |
| `invalid_frame` | Ungültiges JSON, Feldstruktur oder Ablauf außerhalb der Policy. |
| `unsupported_version` | Version wird nicht unterstützt. |
| `unknown_type` | Typ wird nicht unterstützt. |
| `invalid_direction` | Typ wurde in falscher Richtung gesendet. |
| `unknown_delivery` | Kein auf dieser Verbindung offener Versuch dieser Delivery-ID. |
| `not_found` | Eigene aktive Nachricht nicht auffindbar. |
| `invalid_cursor` | Producer-Listen-Cursor ungültig, falscher Scope, nicht mehr gültige Generation oder unzulässige Position. Ungültige Topic-Poll-Cursor starten am aktiven Anfang. |
| `unauthorized` | Client-Identität fehlt oder ist ungültig. |
| `forbidden` | Topic-Operation ist nicht erlaubt. |
| `resource_exhausted` | Ressourcenlimit verhindert die Operation. |
| `security_storage_busy` | Security-Schreibqueue voll; keine Änderung des Sicherheitszustands. |
| `security_storage_unavailable` | Security-Writer ausgefallen; Details zum RAM-/Speicherstand liefert die Admin-Diagnose. |
| `rate_limited` | Client-/Quellenbudget erreicht; `retry_after_ms` gibt die Wartezeit an. |
| `owner_unavailable` | Verantwortlicher Owner nicht erreichbar; keine lokale Ersatzfreigabe. |
| `route_conflict` | Widersprüchliche Topic-Zuständigkeiten oder unzulässiger Pfad. |
| `internal_error` | Interner Fehler ohne sensible Details. |

## Implementiertes Protobuf-Format

[Schema und Migration](protobuf.md) beschreiben binäre Protobuf-Frames mit oneof-Nachrichtenkörpern, Bytes-/Event-Payloads und die lesbare JSON-Darstellung für CLI/SDK. Die hier gezeigten JSON-Beispiele bleiben die lesbare Darstellung der CLI. Identität, ACL, Cursor, Ablauf und technische Statusregeln gelten für den Protobuf-Clienttransport und die interne lesbare Darstellung.


## Implementierter clientseitiger Schlüsselwechsel

Auf einer bereits authentifizierten WSS- oder lokalen Unix-Socket-Verbindung gilt:

| Request | Response | Felder zusätzlich zu Version/Typ und Command-/Request-ID |
| --- | --- | --- |
| `rotation_start` | `rotation_offer` | Antwort: `rotation_id`, `expires_at`, `new_valid_until`. |
| `rotation_confirm` | `rotation_ready` | Request: `rotation_id`, `shared_secret`; Antwort: `rotation_id`, `nonce`, `challenge`, `aad`, `expires_at`, `new_valid_until`. |
| `rotation_commit` | `rotation_complete` | Request: `rotation_id`, `encrypted_public_key`, `signature`; Antwort: `rotation_id`, `client_id`, `token_id`, `valid_until`. |

Der Client ist hier Initiator und erzeugt nach dem Offer das 32-Byte-Shared-Secret. Es wird kanonisch Base64-kodiert im Confirm übertragen und vom Service nicht zurückgesendet. Der Service liefert eine zufällige 12-Byte-AES-GCM-Nonce und eine eigene 32-Byte-Proof-Challenge. Der neue Public Key wird mit AES-256-GCM verschlüsselt; der Authentifizierungs-Tag wird an den Ciphertext angehängt, dann Base64-kodiert.

AAD ist das kompakte UTF-8-JSON-Tupel `["mynofy-rotation-v1", owner_id, stable_client_id, rotation_id, old_token_id, new_valid_until_unix_ms]`. Clients konstruieren dieses Tupel selbst und vergleichen die vom Service zur Orientierung Base64-kodiert zurückgegebenen Bytes. Die Proof-Signatur mit dem neuen privaten Schlüssel gilt über `ASCII("mynofy-rotation-proof-v1") || 0x00 || aad_raw || new_public_key_raw_32 || challenge_raw_32`.

Fehlgeschlagener Commit verbraucht den passenden Vorgang samt Secret/Nonce. Ein neuer Versuch muss mit neuer Wechsel-ID beginnen. Fristende und Widerruf verhindern einen noch offenen Commit; ein abgeschlossener Commit wird nicht zurückgerollt. Die neue Gültigkeit folgt der Owner-CFG (`key_validity_seconds`). Serverinitiierte Offers sind nicht vorgesehen; die Rotation initiiert der Client. `rotation_in_progress` und `rotation_invalid` ergänzen die Fehlercodes. Auch `security_storage_busy`/`security_storage_unavailable` können den Commit verhindern; ein bereits konsumierter Vorgang muss neu begonnen werden. Ein erfolgreicher Commit wirkt sofort im RAM und wird asynchron im [Security-Journal](security-journal.md) gespeichert; die Antwort ist keine Datenträgerbestätigung. [Fehlerbeispiele](../protocol/v1/security-storage-examples.json).

Der lokale Prototyp vergibt Message-IDs als achtstellige ASCII-Zähler aus dem Alphabet `0–9A–Za–z`, Owner-lokal aus reservierten Bereichen. Sie sind innerhalb des aktiven Owner-Bestands eindeutig, können nach Ablauf und Umlauf sowie nach Neustart wiederverwendet werden und sind keine Autorisierungsnachweise. Anwendungen dürfen aus ihrer Reihenfolge keinen Poll-Fortschritt ableiten. Delivery-ID und Cursor besitzen weiterhin eigene Formate.

## Ereigniskatalog abrufen

`get_catalog` ist eine authentifizierte Metadatenabfrage mit `id`, `name` und `catalog_version` (numerisch). Der Owner antwortet mit `catalog`, `request_id` und der vollständigen Definition unter `catalog`. Aktuell verfügbar: `mynofy`, Version `1` (Entwurf). Kein Topic-Recht erforderlich; Client-Rate-Limit und Antwortbudget gelten unverändert. Unbekannter Name/Version: `not_found`. Server-Frames vom Typ `catalog` sind keine Client-Commands. Dies ist weder ein automatischer Handshake-Abgleich noch eine Registrierung eigener Kataloge.

Die Definition einschließlich Parametertypen ist im [Ereigniskatalog](event-catalog.md#vom-client-abrufen) beschrieben. Die dortigen 4096 Byte begrenzen freie Payloads; sie begrenzen nicht diese Metadatenantwort. Bisherige JSON-Payloads werden kompatibel als JSON-Bytes gespeichert und unterliegen ebenfalls dem 4096-Byte-Limit.
