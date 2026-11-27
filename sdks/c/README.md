# mynofy C-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Kleine native C-ABI über die vorhandene Rust-Clientbibliothek: langlebige
Verbindungen, direkte WSS-/Unix- und Owner-Tunnel, typisierte Bytes/Events,
Publish/Poll, eigene Nachrichten, Rechte, Katalog und Schlüsselwechsel.
Keine CLI-Unterprozesse, kein zusätzlicher Protobuf-/TLS-/Kryptoanbieter und keine
neue Registry-Abhängigkeit. Header: [mynofy.h](include/mynofy.h).

Die ABI ist wie das Protokoll ein veränderlicher Prototyp. Header und Bibliothek
müssen aus derselben Version stammen. Config-Größe und ABI-Version werden geprüft.

## Bauen und einbinden

```sh
cargo build --locked --release -p mynofy-client-c
```

Erzeugt `target/release/libmynofy_c.so` (Linux), `.dylib` (macOS) und jeweils eine
statische `.a`. Die konsumierende C-Anwendung benötigt zur Laufzeit nur Bibliothek
und Header beim Kompilieren; kein Rust-Toolchain, Python, CLI oder Protobuf-Compiler.
Die Bibliothek enthält den vorhandenen Rust-/AWS-LC-/Rustls-/Protobuf-Clientstack.

Unter Linux ein vollständiges [C11-Beispiel](examples/local.c) bauen:

```sh
cc -std=c11 -Wall -Wextra -Werror -pedantic -I sdks/c/include \
  sdks/c/examples/local.c -L target/release -lmynofy_c \
  -Wl,-rpath,"$PWD/target/release" -o /tmp/mynofy-c-example
/tmp/mynofy-c-example SOCKET CLIENT.pk8 local local/events
```

CMake mit vorgebauter Bibliothek:

```sh
cmake -S sdks/c -B target/c-sdk-example \
  -DMYNOFY_LIBRARY="$PWD/target/release/libmynofy_c.so"
cmake --build target/c-sdk-example
```

Auf macOS den `.dylib`-Pfad einsetzen; die Bibliothek verwendet den portablen
Install-Namen `@rpath/libmynofy_c.dylib`. Der CMake-Import `mynofy::client` enthält
Header und System-Linkabhängigkeiten und akzeptiert auch die `.a`. Beim manuellen
statischen Linken unter Linux die vom Compiler gemeldeten Systembibliotheken
verwenden: `-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc`.

## Verbindung und Publish

Public Key und Topic-Rechte müssen beim Owner registriert sein. Private Schlüssel
von `mynofy keygen` sind direkt importierbar; `mynofy_key_parse` nimmt die rohen
PKCS#8-Bytes entgegen. Die API erzeugt oder liest selbst keine Schlüsseldateien.

```c
#include "mynofy.h"

/* der/der_len wurden von der Anwendung sicher geladen. */
mynofy_key *key = NULL;
mynofy_client *client = NULL;
mynofy_response *response = NULL;
mynofy_error error;
int32_t status = mynofy_key_parse(der, der_len, &key, &error);
/* Nach jedem Aufruf status prüfen; bei Fehler Ressourcen freigeben. */

mynofy_config config = MYNOFY_CONFIG_INIT;
config.socket_path = "/privater/pfad/mynofy.sock";
config.owner = "local";
config.key = key;
status = mynofy_connect(&config, &client, &error);

const uint8_t bytes[] = {0, 1, 255};
status = mynofy_publish_bytes(client, "local/events", bytes, sizeof bytes,
                            "application/octet-stream", NULL, &response, &error);
/* Erfolgreiches Result: Message-ID und vom Owner bestimmtes expires. */
mynofy_str id = mynofy_response_message_id(response);
/* id.data ist geliehen und nicht NUL-terminiert; vor response_free kopieren. */
mynofy_response_free(response);
mynofy_client_free(client);
mynofy_key_free(key);
```

Das vollständige Beispiel prüft Fehler und räumt sämtliche Handles auf. Für WSS
statt `socket_path` die Felder `url="wss://HOST:PORT"` und `ca_file` setzen.
TLS 1.3, CA-/Hostname-Prüfung und Keypair-Auth übernimmt der Rust-Client. Unix
übernimmt dessen geschützten Pfad, Socket- und Kernel-Peer-Prüfung. Keine Option zum
Abschalten dieser Prüfungen, kein Klartext-/Proxy-Umgebungsfallback.

Owner-Tunnel: `via_topic`, `owner_ca_file`, optional `owner_name`; `owner` ist die
Identität des Ziel-Owners. `ca_file` prüft bei äußerem WSS den Proxy, die Owner-CA
den inneren TLS-Endpunkt. Client-Auth und ACL enden beim Owner.

`mynofy_client_info` liefert Owner, stabile Client-ID und U32-Neustartzähler.
Nach bekanntem Neustart bewusst ohne gespeicherten Poll-Cursor lesen und mit dem
eigenen Zustand abgleichen. Das SDK verwaltet keinen Fortschritt automatisch.

## Events und Antworten

```c
mynofy_event event = {0};
event.catalog = 1; event.catalog_version = 1;
event.category = 12; event.action = 42; event.status = 2;
event.parameter_count = 2;
event.parameters[0].type = MYNOFY_U64;
event.parameters[0].value.u64_value = UINT64_MAX;
event.parameters[1].type = MYNOFY_I64;
event.parameters[1].value.i64_value = INT64_MIN;
```

IDs und Parameterbedeutung aus Katalog/Anwendungsvereinbarung entnehmen. Ganze
Struct mit `{0}` initialisieren, danach IDs, Anzahl und aktive Union-Mitglieder
setzen. Höchstens drei Parameter: U32/U64/I32/I64/F32/F64/BOOL; BOOL exakt 0/1.
Nicht endliche Floats und unbekannte Katalog-IDs werden lokal abgewiesen.
`mynofy_publish_event` benötigt keinen JSON-Zwischenschritt. Rohe Bytes maximal
4096, einschließlich NUL. `expires` ist optionaler RFC3339-Zeittext, sonst gilt
Topic-Policy. Alle Ganzzahlen bleiben native `uint64_t`/`int64_t` ohne Rundung.

`mynofy_poll(client, topic, after, limit, &response, &error)` gibt sofort eine
begrenzte Seite zurück; `after=NULL` liest den aktiven Bestand. Limit 1..100.
Mit `mynofy_response_message_count` und `mynofy_response_message_at` auf Einträge
zugreifen; diese enthalten Payload und Ablauf.
Bei `has_more` die nächste Seite sofort holen. Cursor erst nach Verarbeitung oder
dauerhafter Übernahme fortschreiben. Poll does not record reader state. Report application results as separate messages.

Producer-Listen enthalten Zusammenfassungen ohne Payload; dafür
`mynofy_get_message` verwenden. Bei `mynofy_list_messages` Filter und Limit beim
Paging beibehalten. Message views have no delivery-ID or confirmation-counter fields.

`mynofy_get_permissions` liefert Actions, Owner und ACL-Revision über die
zugehörigen Accessors. `mynofy_get_catalog` liefert die Katalogdefinition als
uninterpretierte JSON-Bytes; ein JSON-Parser bleibt Anwendungssache.

## Besitz und Parallelität

- Keys, Clients und Responses sind opaque Handles. Jedes erfolgreiche Ergebnis
  genau einmal mit der zugehörigen `*_free`-Funktion freigeben, nie mit C-`free`.
- Inputs sind NUL-terminierte UTF-8-Strings beziehungsweise lesbare Datenpuffer
  mit Länge. Optionale Strings sind `NULL`; leere Pflichtnamen sind ungültig.
  `NULL` für Bytes nur bei Länge 0. Config/Event vollständig initialisieren.
- Outputs müssen gültig und beschreibbar sein. Handle-Outputs beginnen als `NULL`;
  vorhandene Responses vor Wiederverwendung freigeben. Error-Ausgabe ist optional.
- String-/Byte-Views sind unveränderlich und **nicht NUL-terminiert**. Sie gelten
  bis zum Freigeben ihres besitzenden Handles. Message-Views/Parameterwerte sind
  Stack-Kopien; ihre Datenpointer leihen den Response-Speicher. Ein Response bleibt
  auch nach dem Freigeben des Clients lesbar. Keine weitere Payload-Kopie beim Lesen.
- Der Client kopiert seine Credentials beim Connect; der ursprüngliche Key darf
  danach freigegeben werden. Private SDK-Keypuffer werden beim Drop zeroisiert;
  eigene exportierte Kopien/Dateien bleiben Verantwortung der Anwendung.
- Eine Operation pro Client. Gleichzeitige Versuche liefern `MYNOFY_BUSY`, ohne
  Befehlsqueue. Unterschiedliche Clients können von mehreren C-Threads arbeiten.
- `mynofy_client_close` darf parallel laufen: dauerhafter Abbruch und Schließen
  des Transports. `client_free` erst aufrufen, wenn alle Aufrufe zurückgekehrt sind.
  Keys/Responses ebenfalls erst nach Ende aller lesenden Zugriffe freigeben.
- Synchron/blockierend. Alle aktiven C-Clients teilen eine I/O-Runtime mit zwei
  Workerthreads. Nach dem Freigeben des letzten Clients endet sie; Responses und
  Keys benötigen sie nicht. Keine pro Client gestartete Runtime/Workergruppe.

Der notwendige `unsafe`-Code liegt ausschließlich in dieser C-Adapter-Crate.
Null-/Größen-/UTF-8-/Enum-Prüfungen ersetzen nicht die Verantwortung für gültige
Pointer, Lebensdauern und genau eine Freigabe. Rust-Panics werden an der ABI-Grenze
abgefangen; keine Rust-Exceptions werden durch einen C-Stack abgewickelt.

## Fehler, Abbruch und Rotation

Status ist `MYNOFY_OK` oder ein Code aus `enum mynofy_status`. `mynofy_error`
enthält Code, begrenzte NUL-terminierte Diagnose, Broker-Code und optional
`retry_after_ms`. Nach erfolgreichem Aufruf ist die Fehlerausgabe zurückgesetzt.

Broker-Abweisung hält die Verbindung nutzbar. Transport-/Protokollfehler, Timeout
oder Close verwerfen sie. `outcome_unknown` zeigt an, dass Publish/Rotation
bereits wirksam sein könnten. Nie automatisch erneut senden. Bekannte eigene
Nachrichten und lokalen Zustand abgleichen, explizit neu verbinden. Es gibt keine
automatischen ACKs, Reconnects, Pollschleifen, Zustandsdateien oder Admin-Funktionen.

`config.timeout_ms` begrenzt Verbindungsaufbau einschließlich Authentifizierung
und jeweils eine Operation, Standard 10000 ms. Der komplette Schlüsselwechsel hat
ebenfalls dieses Budget; zusätzlich gelten die Fristen des Owners und die
festen Phasen-/Austauschlimits des Rust-Transports. Ein größeres C-Budget verlängert
diese inneren Limits nicht.

Vor `mynofy_rotate` den neuen Key mit `mynofy_key_generate` erzeugen und über
`mynofy_key_pkcs8` selbst sicher speichern, z. B. exklusive Datei mit Modus 0600.
Erfolg schließt die alte Sitzung. Mit neuem Key und ursprünglicher
`config.client_id` neu verbinden. Bei unklarem Commit beide Schlüssel behalten.
Kein Dateischreiben und keine automatische Credentials-Ersetzung durch die API.

## Tests

Als normaler Benutzer mit Rust/Cargo, C11-Compiler, Python-Standardbibliothek
und OpenSSL:

```sh
./scripts/test-c-sdk.sh
cargo clippy --locked -p mynofy-client-c --all-targets -- -D warnings
```

Rust-Unit-Tests prüfen FFI-Eingaben, Verlust/Timeout/Busy/Close, Antwortbindung,
strukturierte Rate-Limits, Panic-Grenze, gemeinsame Event-Wirevektoren.
Ein getrennt kompiliertes C-Programm prüft echte Unix-/WSS-Dienste, ACL, Bytes und
Events, Paging, Views nach Clientfreigabe und Rotation mit Rust-CLI-
Anmeldung. Linux prüft auch, dass nach Clientfreigabe keine SDK-I/O-Threads bleiben.
Das eigenständige C-Beispiel wird kompiliert und ausgeführt.

Die stdlib-Testfixture wird mit der Bash-Suite geteilt; weder Bash-SDK noch CLI
sind Laufzeitabhängigkeiten der C-Anwendung. TLS-/Mehrhop-Prüfungen des gemeinsam
genutzten Rust-Transports bleiben zusätzlich in dessen bestehender Integrationssuite.
Native macOS-ABI-/Laufzeitprüfung folgt in der CI. ASan/UBSan auf dem C-Testclient
instrumentieren diesen Consumer, nicht die vorgebaute Rust-Bibliothek.

## Owner-authenticated producer identity

`mynofy_response_message_producer_id(response, index)` returns the immutable stable
client identity stamped by the owner for poll and get-message results. The UTF-8
view is borrowed from the response, is not NUL-terminated, and remains valid until
response-free. Invalid indices, null responses, and producer-list summaries yield
an empty view. This accessor does not change the existing message struct layout.
Match application actor IDs to this value rather than trusting payload claims.

Rotation rejections preserve the owner's code, detail and backoff in
`mynofy_error` and return `MYNOFY_BROKER` with `outcome_unknown = 0`, including
rejections during confirmation and commit. The connection remains available for
an explicit next operation. A lost or invalid commit response remains an unknown
outcome; keep both keys and verify credentials through an explicit reconnect.
No automatic retry occurs. See [key rotation guide](../../docs/key-rotation.md).

For cursor continuity, compare the value from `mynofy_client_boot_id` between authenticated sessions. This borrowed string is empty for legacy owners and is valid while the handle lives.
