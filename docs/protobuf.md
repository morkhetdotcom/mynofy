# Protobuf und typisierte Payloads

Release **v0.1.0 is the first compatibility baseline**. Earlier development and
demo formats are not supported releases and require no migration support.
Within protocol version 1, preserve existing field numbers, meanings and required
fields. New optional fields must preserve behavior when absent; retired tags stay
reserved. Incompatible changes require a new protocol version and an explicit
upgrade plan. The listener rejects unsupported frame versions.

## Verfügbare Schnittstelle

Client und CLI verwenden ausschließlich binäre Protobuf-Nachrichten über denselben abgesicherten WebSocket-/Unix-Transport. Eine WebSocket-Nachricht enthält genau einen Protobuf-Frame. Es wird kein zusätzliches Längenpräfix innerhalb der WebSocket-Nachricht benötigt. JSON-Anwendungscommands als Textframes werden vor und nach Authentifizierung mit WebSocket-Code 1003 abgewiesen. Der separate Admin-Socket und JSON-Steuerframes für Peers/Tunnel sind eigene Schnittstellen.

Die CLI liest und schreibt weiter JSONL für Menschen und Terminalwerkzeuge. Dafür wandelt sie an der Grenze zwischen der lesbaren Darstellung und Protobuf um. Für direkten Zugriff ohne JSON-Konvertierung bietet das SDK `exchange_frame` mit `mynofy_protocol::pb::Frame`. `exchange` ist die bisherige JSON-förmige Komfort-API mit Protobuf auf dem Netzwerk; `exchange_json` wurde entfernt; ein JSON-Text-Wire-Modus für Client-Commands existiert nicht mehr.

Alle bestehenden lokalen Client-Commands haben Protobuf-Varianten: Auth, Publish/Poll, Producer-Abfragen, Permissions, Katalogabruf und Rotation. Peer/proxy control messages use a separate protocol; see [networking](networking.md). Protocol version 1 uses release v0.1.0 as its compatibility baseline.

## Schema und Generierung

[generate.py](../protocol/protobuf/generate.py) enthält die Felddefinitionen und erzeugt das [portable proto2-Schema](../protocol/protobuf/mynofy.proto), prost-Derives, JSON-Adapter und die statischen Katalog-Enums/Mitgliedschaftsprüfungen. Existing field numbers must not be reinterpreted or reused. Es gibt keinen zur Laufzeit benötigten Descriptor-Loader und keine Build-Abhängigkeit zu `protoc`/`prost-build`.

```sh
python3 protocol/protobuf/generate.py
cargo fmt --all -- --check
cargo test --locked -p mynofy-protocol
```

Der Generator ist Entwicklungstooling und benötigt Python/Rustfmt; normale Cargo-Builds verwenden die eingecheckten Dateien. Die Rust-Namen `pb::Result` und `payload::Body` sind Protokolltypen, keine Rust-Fehlerbehandlung.

Die Implementierung verwendet den offiziellen [prost-Upstream](https://github.com/tokio-rs/prost) und [Bytes](https://github.com/tokio-rs/bytes). `bytes` war bereits transitiv vorhanden. Die zusätzliche Laufzeitbibliothek ist prost; derive bringt notwendige Build-/Makroabhängigkeiten mit. Es gibt keine lokalen Forks oder Git-Patches.

## Zwei Payload-Varianten

Freie Bytes bestehen aus Bytes und optionalem `content_type` (1–128 UTF-8-Bytes). Harte Obergrenze: **4096 tatsächliche Datenbytes**, gegebenenfalls durch `limits.max_payload_bytes` weiter eingeschränkt. Es gibt keine automatische Fragmentierung, Kürzung oder große vorreservierte Payload pro Nachricht. Im RAM hält der Core Bytes als geteilte, unveränderliche `Bytes`; Poll-Kopien müssen deren Inhalt nicht noch einmal kopieren.

Lesbare Darstellung:

```json
{"version":1,"type":"publish","id":"p1","topic":"local/events","payload":{"kind":"bytes","content_type":"text/plain","data":"SGFsbG8="}}
```

`data` ist nur in der JSON-Darstellung kanonisches Standard-Base64. Auf dem Protobuf-Wire stehen die tatsächlichen Bytes. Das Beispiel enthält fünf Nutzbytes (`Hallo`). Leere Bytes sind erlaubt.

Typisierte Ereignisse enthalten numerische Katalog-ID/-version, Kategorie, Aktion, Status und höchstens drei primitive Parameter mit insgesamt höchstens 24 reinen Wertebytes. Katalog-ID 1 bezeichnet `mynofy`, Version 1. Unbekannte Versionen/IDs, fehlende Parameterwerte und nicht endliche Floats werden abgewiesen. Die Grundtypen sind U32/U64/I32/I64/F32/F64/Bool. Keine Strings oder nachgeladenen Kataloge innerhalb eines Ereignisses.

```json
{"version":1,"type":"publish","id":"p2","topic":"workflow/4711/communication","payload":{"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":40,"status":7,"parameters":[{"u64":4711}]}}
```

Das bedeutet `(JOB, EXECUTE, PENDING)` mit einem U64-Parameter. Die konkrete Bedeutung dieses Parameters gehört zum vereinbarten Profil. Der Core prüft bekannte Kennungen und Größen, keine fachlichen Übergänge oder Rollen innerhalb einer Payload. Die [Enums](../crates/mynofy-protocol/src/catalog_values.rs) und numerischen Prüfungen werden aus dem Katalog erzeugt; kein JSON-Katalogscan pro Ereignis.

Eine Payload ist entweder Event oder Bytes. Parameter plus freie Bytes in derselben Payload sind keine dritte implizite Variante. Benötigter Kontext muss im Profil/Topic bekannt sein oder in einer einzelnen freien Payload stehen.

## Kompatibilität und Migration

- Ältere beliebige JSON-Payloads bleiben als Komfortdarstellung lesbar. Sie werden intern als UTF-8-JSON-Bytes gespeichert; ihre vollständige JSON-Serialisierung muss in 4096 Byte passen. Ein String mit 4096 Zeichen benötigt zusätzlich seine JSON-Anführungszeichen und passt so nicht mehr. Für genau 4096 Nutzbytes die explizite Byte-Variante verwenden.
- Die Payload-Objekte mit `kind: "event"` beziehungsweise `kind: "bytes"` sind für die neuen Varianten reserviert. Der interne Inhaltstyp `application/vnd.mynofy.legacy+json` dient dem JSON-Kompatibilitätsadapter; er prüft UTF-8/JSON und darf nicht als undurchsichtiger binärer Typ benutzt werden.
- Vorhandene Service-Konfigurationen mit `max_payload_bytes: 65536` müssen auf höchstens 4096 umgestellt werden. Neue Szenariovorlagen setzen bereits 4096. Bestehende lokale `.local/*`-Dateien werden nicht ungefragt geändert. Den Server nach Aktualisierung neu starten.
- Antwortgrößen und Transportgrößen haben weiterhin eigene Limits; eine Poll-Seite oder Katalogantwort darf größer als eine einzelne Payload sein.
- Protobuf ignoriert unbekannte Felder gemäß dem verwendeten Decoder; oneof-/Scalar-Wiederholungen folgen dessen Zusammenführungsregeln. Die lesbare JSON-Eingabe der CLI prüft doppelte Schlüssel und unbekannte Command-Felder vor der Übersetzung. Der Broker prüft Version, Richtung, Identität, ACL, Grenzen und Zustand zusätzlich zum Protobuf-Codec.
- Auth-Schlüsselfelder/Signaturen und RFC3339-Zeiten behalten vorerst ihre vorhandene Textdarstellung in den typisierten Kontrollnachrichten. Das verändert weder Signaturtranskripte noch Rotationsbindungen. Der seltene Katalogabruf transportiert die eigenständig versionierte JSON-Definition als Bytes in einer Protobuf-Katalogantwort.

## Speicher und Messung

Nachrichten liegen im [kompakten RAM-Format](binary-message-layout.md) mit numerischen Referenzen, Zeitwerten und eingebetteten Eventparametern statt als Wire-Nachrichten oder JSON-Objektbäume vor. Protobuf-`StoredMessage` wird erst für Antworten aufgebaut. Publish und Poll haben einen direkten Protobuf-Pfad. Auth und Rotation nutzen weiterhin JSON-Konvertierung an ihrem internen Adapter; Publish/Poll/Get/List/Permissions/Katalog besitzen native Protobuf-Pfade. Es wäre falsch, daraus einen vollständig JSON-freien Server abzuleiten. Konfiguration unterstützt YAML/JSON; CLI-Darstellung und Katalogdefinition verwenden weiterhin JSON.

Der the storage and transport guides trennt den gleichen aktuellen Core mit zwei Encodings vom zusätzlichen Speicherumbau gegenüber der älteren Implementation. Netzwerk/TLS-Durchsatz wurde damit nicht gemessen.
