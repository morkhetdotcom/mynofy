# Sprach-SDKs

Client-Unterstützung bedeutet einbindbare SDKs, nicht eine Agenten-Runtime.
Sprachen teilen das vorhandene Protobuf-/Auth-Protokoll und gemeinsame
[Konformitätsvektoren](../protocol/conformance/sdk-v1.json); Workflow, Mandat,
Tickets und lokale Fachzustände bleiben Anwendungssache.

| Sprache | Stand |
| --- | --- |
| Rust | Vorhandene [Rust client crate](../crates/mynofy-client), Referenz für direkten und getunnelten Zugang |
| Python | [Eigenständiges asyncio-SDK](../sdks/python/README.md), direkter WSS-/Unix-Zugang |
| Go | [Natives SDK](../sdks/go/README.md), direkter WSS-/Unix-Zugang |
| Java | [SDK ab Java 17](../sdks/java/README.md), direkter WSS-/Unix-Zugang |
| Bash | [Shell-Bibliothek](../sdks/bash/README.md) über Rust-CLI, direkter und getunnelter Zugang |
| C | [Native C-ABI](../sdks/c/README.md) über Rust-Client, direkte und getunnelte Verbindungen |

## Gemeinsame Grundoperationen

Verbindung mit Owner-/Client-Identitätsprüfung, Publish, Poll mit explizitem
Cursor, ACK/NACK, eigene Nachrichten, Berechtigungen und Katalog. Clientseitiger
Schlüsselwechsel erhält die stabile Client-ID. SDKs dürfen keine fremden Rechte
aus Rollenbehauptungen ableiten oder unklare Publishes automatisch wiederholen.

Die [Wire-Vektoren](../protocol/conformance/sdk-v1.json) prüfen Bytes, typisierte
Events, große `u64`/`i64`, Request-/Response-Felder und Auth-/Rotations-Transkripte.
Rust erzeugt und prüft sie, Python, Go und Java lesen dieselben Daten. Spätere SDKs verwenden
sie ebenfalls; die Vektoren sind keine zusätzliche Wire-Spezifikation.

## Verifizierter Python-Umfang

Python verwendet drei direkte Laufzeitabhängigkeiten: offizielle Protobuf-Runtime,
`websockets` und `pyca/cryptography`. Keine CLI-Unterprozesse im SDK und keine
neuen Rust-Registry-Abhängigkeiten. Generierte Bindings liegen im Paket; der
Compiler `grpcio-tools` ist nur Entwicklungswerkzeug.

Tests verwenden selbst gestartete Rust-Services: Unix und TLS 1.3/WSS, Owner/
Clientauth, ACL-Fehler, Katalog, Bytes und Events einschließlich Integergrenzen,
Paging/Cursor, ACK/NACK und Rotation mit anschließender Rust-CLI-Anmeldung.
Deterministische Fehlerpeers prüfen verlorene Mutationsergebnisse, Timeout,
Antwortbindung, Cancellation und Schließen während eines Requests. Der Python-
Testlauf ist separat von `cargo test --workspace` auszuführen.

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -e './sdks/python[dev]'
./scripts/test-python-sdk.sh
```

Für reproduzierbare Entwicklungsabhängigkeiten steht
[requirements-test.lock](../sdks/python/requirements-test.lock) bereit; es enthält
auch Build-/Generatorwerkzeuge. Das installierte Laufzeitpaket benötigt sie nicht.

Echte native macOS-Prüfung folgt mit der geplanten CI. Peer-Tunnel, automatische
Polling-/Zustandshelfer und Admin-Zugang gehören nicht zur ersten Python-Version.
Der vorhandene Rust-Client ist für Owner-Tunnel weiter der vollständige Zugang.

## Verifizierter Go-Umfang

Das [Go-SDK](../sdks/go/README.md) unterstützt dieselben direkten Grundoperationen
und den clientinitiierten Schlüsselwechsel. Drei direkte Abhängigkeiten:
`coder/websocket`, offizielle Protobuf-Runtime und `golang.org/x/sys` für Peer-UIDs.
Kryptografie und TLS verwenden die Go-Standardbibliothek. Keine CLI-Unterprozesse
und keine Compiler-/Generatorabhängigkeiten zur Laufzeit; Go ab 1.26.

`scripts/test-go-sdk.sh` führt native Rust-Service-Tests und deterministische
Fehlerpeers mit dem Go-Race-Detector aus. Rust-generierte Schlüssel sind in Go
lesbar; Go-generierte Schlüssel nach Rotation in Rust und optional Python.
Darwin/ARM64 wird crosskompiliert; das ersetzt keinen nativen macOS-Test.
Peer-Tunnel, Admin-Zugang und Polling-/Zustandshilfen sind wie bei Python ausgenommen.
Erste Versionen aller sechs Sprachzugänge sind implementiert. Die CI im privaten Entwicklungsrepository
konfiguriert ihre gemeinsamen Linux-/macOS-Prüfungen; der erste Runnerlauf steht aus.

## Verifizierter Java-Umfang

Das [Java-SDK](../sdks/java/README.md) ab Java 17 deckt die direkten Grundoperationen
und den clientinitiierten Schlüsselwechsel ab. Zwei direkte Laufzeitabhängigkeiten:
Java-WebSocket und offizielle Protobuf-Runtime, dazu die transitive SLF4J-API.
Kryptografie/TLS aus dem JDK; keine zusätzlichen nativen oder Krypto-Pakete.
JSON-Hilfen, Protobuf-JSON und JUnit nur im Test. Katalogdefinition bleibt JSON-Bytes.

`scripts/test-java-sdk.sh` prüft echte Rust-Services, gemeinsame Wire-Vektoren und
Fehlerpeers einschließlich Thread-Interrupt, Antwortverlust und Größenlimit.
Rust-v2-Schlüssel direkt importieren; Java-v2-Export in Rust und optional Python
prüfen. PKCS#8 v1 benötigt zusätzlich den Public Key, weil das JDK keine direkte
Seed→Public-Key-API anbietet. Linux-Unix-Transport mit Peer-Prüfung getestet;
macOS bleibt für CI offen. Owner-Tunnel und Admin wie bei Python/Go ausgenommen.

## Verifizierter Bash-Umfang

Die [Bash-Bibliothek](../sdks/bash/README.md) kapselt die vorhandene CLI mit
Komfortfunktionen für Publish/Poll, ACK/NACK, eigene Nachrichten, Rechte, Katalog,
Keygen und Rotation. jq ausschließlich für Strings/Antwortprüfung; große Zahlen
als unveränderte Dezimaltexte erhalten. Keine neuen Rust-/Kryptoabhängigkeiten.

Komfortaufrufe öffnen jeweils eine Verbindung und prüfen Request-ID/Antworttyp.
Broker-Fehler, CLI-Ausfall, ungültige Antworten und Eingabefehler haben getrennte
Exitcodes. Private temporäre Antwortdateien und Operationsprozesse werden beim
regulären Ende/abfangbaren Operationssignal bereinigt. Ein JSONL-Passthrough hält
bei Bedarf eine CLI-Verbindung offen; dessen rohe Exit-/Antwortsemantik bleibt.
Die CLI liefert dort keine separaten Welcome-/Neustart-Metadaten.

`scripts/test-bash-sdk.sh` prüft gemeinsame Request-/Rust-Wire-Vektoren, echte
Unix-/WSS-Dienste, Auth/ACL, Bytes/Events, Integergrenzen, Paging, ACK/NACK, Rotation,
Batch und TLS-Fehler. Doubles ergänzen Quoting, Antwortbindung, Exitcodes und Abbruch.
Owner-Tunnel-Flags geprüft; reale Mehrhop-Prüfung weiterhin in der Rust-/CLI-Suite.

Die Bash-Suite besteht unter Bash 3.2 und 5.2 auf Linux; das ersetzt keinen
nativen macOS-Test.

## Verifizierter C-Umfang

Das [C-SDK](../sdks/c/README.md) ist eine separate Workspace-Adapter-Crate über dem
Rust-Client. Shared-/Static-Library mit C11-Header, ohne Server-/CLI-Abhängigkeit
oder zusätzliche Registry-Pakete. Typed Publish/Poll ohne JSON-Zwischenschritt,
unveränderliche Response-Views statt zusätzlicher Payload-Kopien, native U64/I64.
Keys und Responses haben explizite Freigaben; der Client hält eigene Credentials.

Ein gemeinsamer I/O-Pool mit zwei Threads für alle aktiven C-Clients, Freigabe nach
dem letzten Client. Eine Operation pro Handle, Busy ohne Queue und paralleles
Close; keine Freigabe eines Handles während anderer Aufrufe. C-Pointerkonvertierung
ist die eng begrenzte Ausnahme zum sonstigen Verzicht auf eigenen unsafe-Code.

`scripts/test-c-sdk.sh`: Rust-FFI-Unit-Tests plus separat kompiliertes C-Programm an
echten Unix-/WSS-Services, Auth/ACL, Integergrenzen, Bytes/Events, Cursor, ACK/NACK,
Response-Lebensdauer, Threadabbau und Key-Rotation/Rust-CLI-Interop. ASan/UBSan
prüfen zusätzlich den C-Consumer; Rust bleibt dabei uninstrumentiert. Owner-Tunnel
nutzen den bestehenden Rust-Backendpfad und dessen Mehrhop-Integrationstests.
Native macOS-ABI-Prüfung ist in der CI im privaten Entwicklungsrepository konfiguriert; erster nativer Lauf noch offen.
