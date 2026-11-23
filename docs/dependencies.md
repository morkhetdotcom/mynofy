# Abhängigkeiten des ersten Rust-Prototyps

Stand: 12. November 2026. Der Workspace deklariert fünfzehn unterschiedliche direkte externe Abhängigkeiten, zusätzlich interne Pfadabhängigkeiten zwischen seinen vier Crates. Es gibt keine zusätzlichen Dev-Abhängigkeiten. Es gibt keine Git-Abhängigkeiten, `[patch]`-Overrides oder projektspezifischen Forks. `Cargo.lock` hält die aufgelösten Versionen und Registry-Prüfsummen fest.

| Crate | Verwendung | Upstream |
| --- | --- | --- |
| `tokio` | Async-Runtime, TCP, Timer, begrenzte Parallelität. | [tokio-rs/tokio](https://github.com/tokio-rs/tokio) |
| `tokio-rustls` | TLS 1.3 mit Rustls; PEM-Verarbeitung über dessen vorhandene PKI-Typen. | [rustls/tokio-rustls](https://github.com/rustls/tokio-rustls) |
| `tokio-tungstenite` | WebSocket-Handshake und Frames, ohne weiteres Web-Framework. | [snapview/tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) |
| `futures-util` | Benötigte Stream-/Sink-Erweiterungen; Features auf `std` und `sink` begrenzt. | [rust-lang/futures-rs](https://github.com/rust-lang/futures-rs) |
| `serde` | Typisierte Konfiguration und Protokollfelder. | [serde-rs/serde](https://github.com/serde-rs/serde) |
| `serde_json` | JSON-Frames und CFG; doppelte Schlüssel werden zusätzlich explizit abgewiesen. | [serde-rs/json](https://github.com/serde-rs/json) |
| `saphyr-parser` | YAML-Konfiguration: reiner Event-Parser ohne Default-Features; keine Alias-/Tag-Verarbeitung durch die Anwendung. | [saphyr-rs/saphyr](https://github.com/saphyr-rs/saphyr) |
| `aws-lc-rs` | Derselbe offizielle Provider für Rustls und Ed25519, SHA-256, HMAC, CSPRNG, PBKDF2 und AES-GCM. | [aws/aws-lc-rs](https://github.com/aws/aws-lc-rs) |
| `base64` | Kanonisches Token-/Schlüssel-/Cursor-Encoding. | [marshallpierce/rust-base64](https://github.com/marshallpierce/rust-base64) |
| `time` | RFC-3339-Validierung und Formatierung. | [time-rs/time](https://github.com/time-rs/time) |
| `prost` | Protobuf-Nachrichten und Derives; kein Runtime-Descriptor-Framework. | [tokio-rs/prost](https://github.com/tokio-rs/prost) |
| `bytes` | Unveränderliche geteilte Payloads, vorher bereits transitiv vorhanden. | [tokio-rs/bytes](https://github.com/tokio-rs/bytes) |
| `zeroize` | Verwerfen sensibler Schlüssel-/Secret-Puffer. | [RustCrypto/utils](https://github.com/RustCrypto/utils) |
| `rustix` | Linux: sichere UID/GID-, Gruppen-, Capability- und `no_new_privs`-Aufrufe im CLI-Bootstrap; Features je Verwendung begrenzt: `std`/`process` sowie `thread` (CLI) oder `fs` (Server). | [bytecodealliance/rustix](https://github.com/bytecodealliance/rustix) |
| `nix` | Andere Unix-Plattformen: Identitätsprüfung (macOS verweigert Root) und verfügbare POSIX-Aufrufe; nur `user`. | [nix-rust/nix](https://github.com/nix-rust/nix) |

Tokio, Rustls, Tungstenite und der Kryptografie-Provider bringen notwendige transitive Abhängigkeiten mit. Die Anzahl direkter Abhängigkeiten bezeichnet nicht die gesamte Zahl transitiver Crates. Versionen und gemeinsame Features stehen zentral unter `[workspace.dependencies]` im Root-Manifest. Der Provider benötigt native Build-Werkzeuge; OpenSSL ist keine TLS-Laufzeitabhängigkeit des Services. Es wird kein zweiter Signatur-/AEAD-Stack eingebaut.

Der archivierte Wrapper `rustls-pemfile` wurde vor Abschluss entfernt; sein Upstream [verweist auf die PKI-PEM-APIs](https://github.com/rustls/pemfile/releases). Die Auswahl wurde gegen Upstream-Repositories und aktuelle Releases geprüft. Eine laufende Wartungszusage kann diese Prüfung nicht ersetzen; vor Updates sind Änderungen, Sicherheitsmeldungen und der aktive Abhängigkeitsbaum erneut zu prüfen.

```sh
cargo tree --locked --workspace --depth 1
cargo tree --locked --workspace --duplicates
cargo metadata --locked --format-version 1
```

Der aktive Baum enthält derzeit zwei Versionen des kleinen transitiven `untrusted`-Hilfscrates aus Provider und WebPKI. Das stammt aus den offiziellen Upstreams; es sind keine zusätzlichen Projekt-Forks. Benötigte Versionskombinationen werden nicht durch lokale Patches erzwungen.

Die [Rust client crate](../crates/mynofy-client) verhindern eine Server-Abhängigkeit des Client-SDKs. Das Protokoll-Crate ist ohne optionales `crypto`-Feature unabhängig von Netzwerk-Runtime und Kryptografie-Provider baubar. Die Workspace-Aufteilung führt keine neuen Registry-Abhängigkeiten ein.

Protobuf ergänzt prost 0.14.5 aus dem offiziellen Upstream. `prost-derive` bringt notwendige Proc-Makro-Abhängigkeiten (unter anderem anyhow/itertools/syn) mit. Kein prost-build, vendored protoc oder Build-Generator als Cargo-Abhängigkeit. Python/Rustfmt regenerieren nur bei Entwicklung die eingecheckten Typen; Details im [Protobuf-Leitfaden](protobuf.md).

Für den frühen [Rechteabbau](privileges.md) wurden die offiziellen Upstreams von rustix 1.1.6 (Bytecode Alliance) und nix 0.31.3 geprüft. Die CLI wählt ihre Bindings plattformspezifisch. Der Server verwendet rustix unter Unix zusätzlich für sichere no-follow-Dateizugriffe, atomare Umbenennung und Dateilocks des Security-Journals; es kommen keine weiteren Registry-Crates hinzu. Client und Protokoll bleiben frei von diesen OS-Bindings. Rustix liefert unter Linux sichere Capability-/prctl- und explizite Thread-ID-APIs; nix liefert die Identitätsprüfung auf macOS; da seine Gruppen-APIs dort ausgeschlossen sind, verweigert mynofy dort privilegierte Starts. Dadurch bleibt `forbid(unsafe_code)` in den vier Kerncrates erhalten. Keine Forks oder eigenen OS-FFI-Bindings. Der getrennte C-SDK-Adapter ist die ausdrücklich benötigte C-ABI-Ausnahme.

YAML verwendet saphyr-parser 0.3.4 aus dem offiziellen Saphyr-Projekt (Paket-MSRV 1.85, kompatibel mit Projekt-MSRV 1.88). Upstream, Release und Manifest wurden vor Auswahl geprüft. Die Produktionsabhängigkeiten sind arraydeque und thiserror; thiserror war bereits transitiv vorhanden. Keine C-YAML-Bibliothek, kein zusätzlicher Serde-YAML-Wrapper und keine Git-Patches. Der Server allein erhält die Abhängigkeit; Client und Protokoll bleiben YAML-frei. Das archivierte serde_yaml wird nicht eingeführt. Die bewusst eingeschränkte Event-Anbindung und Exportregeln stehen im [YAML-Leitfaden](yaml-config.md).

Content-Verschlüsselung verwendet den vorhandenen offiziellen AWS-LC-Provider auch direkt im Server, ohne zusätzliche Registry-Abhängigkeit oder zweiten Krypto-Stack. Die CLI nutzt zusätzlich rustix/termios für verdeckte stdin-Eingabe unter Unix; native macOS-Validierung steht aus. Parameter/Referenzen im [Content-Leitfaden](content-storage.md).

## C-SDK

Die zusätzliche Workspace-Crate `mynofy-client-c` verwendet nur bereits vorhandene
Workspace-Abhängigkeiten: Client, Tokio/Rustls/Tungstenite und Zeroize. Keine neue
Registry-Version und kein zweiter Krypto-/Protobuf-Anbieter. Tokio erhält hier das
bereits verfügbare Feature `rt-multi-thread`; zwei gemeinsame Worker für aktive
C-Clients statt einer Runtime pro Verbindung. Erstes/letztes Handle steuern den
Runtime-Lebenszyklus. `prost`/`serde_json`/`futures-util` sind direkte Testhilfen,
keine neue C-Runtime-API. Pointer- und Union-Konvertierung bleibt im C-Adapter;
Core/Transport und die bisherigen Sicherheits-/OS-Bindings bleiben unverändert.
