# mynofy Go-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Nativer Client ab Go 1.26 für direkte WSS- und Unix-Verbindungen. Keine CLI-
Unterprozesse im SDK. Die API und das gemeinsame Protobuf-v1-Protokoll sind noch
veränderliche Prototypen.

## Einbinden

Das Modul ist noch nicht veröffentlicht. Im eigenen Go-Projekt einen lokalen
Repository-Pfad verwenden:

```sh
go mod edit -require=mynofy.local/sdk/go@v0.0.0
go mod edit -replace=mynofy.local/sdk/go=/absoluter/pfad/mynofy/sdks/go
go mod tidy
```

Imports zuerst im eigenen Programm ergänzen. Die drei direkten Abhängigkeiten sind
[`coder/websocket`](https://github.com/coder/websocket),
[`golang.org/x/sys`](https://pkg.go.dev/golang.org/x/sys) und die offizielle
[Protobuf-Runtime](https://pkg.go.dev/google.golang.org/protobuf).
Ed25519, TLS und AES-GCM kommen aus der Go-Standardbibliothek. Generierte
Protobuf-Typen werden mitgeliefert; Python, protoc und gRPC sind keine Laufzeitabhängigkeiten.

## Lokal senden und lesen

Der Public Key muss beim Owner registriert sein und die nötigen Topic-Rechte
besitzen. `mynofy keygen` erzeugt kompatible Schlüssel. Client und Unix-Service
laufen als derselbe normale OS-Benutzer.

```go
package main

import (
    "context"
    "fmt"
    "os"

    mynofy "mynofy.local/sdk/go"
)

func run() error {
    der, err := os.ReadFile(".local/unix/client.pk8")
    if err != nil { return err }
    key, err := mynofy.ParseKey(der)
    if err != nil { return err }
    ctx := context.Background()
    client, err := mynofy.Dial(ctx, mynofy.Config{
        Socket: ".local/unix/run/mynofy.sock", Owner: "local", Key: key,
    })
    if err != nil { return err }
    defer client.Close()
    fmt.Println(client.Info()) // Owner, stabile Client-ID, Neustartzähler
    sent, err := client.Publish(ctx, "local/events",
        mynofy.Bytes([]byte("Hallo"), "text/plain"), nil)
    if err != nil { return err }
    page, err := client.Poll(ctx, "local/events", "", 20)
    if err != nil { return err }
    for _, item := range page.Items {
        fmt.Println(item.Message.GetId(), item.Message.Payload)
        // Nach Verstehen/Zuordnen; kein fachlicher Abschluss:
    }
    fmt.Println(sent.GetMessageId(), page.GetNextCursor(), page.GetHasMore())
    return nil
}
func main() {
    if err := run(); err != nil { fmt.Fprintln(os.Stderr, err); os.Exit(1) }
}
```

`Poll` startet keine Hintergrundschleife. Bei `has_more` weitere Seiten sofort
abholen. Den Cursor erst nach eigener Verarbeitung oder dauerhafter Übernahme
speichern. Nach bekanntem Service-Neustart anhand von `Info().RestartCounter`
neu ohne Cursor lesen und mit lokalem Zustand abgleichen. Keine automatische fachliche Bestätigung,
keine Zustandsdatei und keine automatische Wiederholung fachlicher Aktionen.
Poll reads without recording reader state; the client keeps its progress locally.

## WSS und weitere Operationen

Für WSS `URL: "wss://localhost:8080"` statt `Socket` setzen, optional
`CAFile: ".local/demo/ca.crt"`. Ohne CA-Datei gelten die System-Trustroots.
TLS mindestens 1.3, Hostname-Prüfung, keine unsichere Option, Redirects oder
Proxy-Umgebungsvariablen. Unix prüft Socketmodus 0600, Verzeichnisschutz und
Kernel-Peer-UID unter Linux/macOS; fehlende Peer-Prüfung führt zum Abbruch.
Native macOS-Tests stehen noch aus; der Darwin-Code wird crosskompiliert.

Alle Operationen nehmen einen `context.Context`. `Config.Timeout` begrenzt
Verbindungsaufbau einschließlich Authentifizierung und jeweils einen Austausch;
Standard zehn Sekunden. Ein früheres Context-Limit hat Vorrang.

- `Publish(ctx, topic, payload, expires)`: maximal 4096 rohe Bytes oder ein Event;
  `expires` ist optional `*time.Time`, andernfalls entscheidet die Topic-Policy.
- `Event(*protocol.Event)`: Katalog 1/v1, gültige Enum-IDs, bis zu drei typisierte
  Parameter. Protobuf erhält `uint64` und `int64` ohne Fließkommakonvertierung.
- `ListMessages(ctx, ListOptions{Topic: ..., Limit: 20})`: eigene aktive Nachrichten;
  beim Paging denselben Filter **und dieselbe Seitengröße** beibehalten.
- `GetMessage(ctx, id)`, `GetPermissions(ctx, topic)` und
  `GetCatalog(ctx, "mynofy", 1)`: letzteres liefert `json.RawMessage` mit geprüfter
  Katalogidentität. Alle übrigen Antworten sind generierte Protobuf-Typen aus
  `mynofy.local/sdk/go/protocol`; optionale Felder sind Zeiger.

Payloads und ihre Byte-Slices während eines Aufrufs nicht parallel verändern.
`Bytes` übernimmt den Slice ohne vorsorgliche Kopie.

## Fehler und Schlüsselwechsel

`errors.As` unterscheidet `*BrokerError` (Code, Detail, optional RetryAfterMS) und
`*TransportError`. Bei I/O-/Protokollfehler wird die Verbindung verworfen.
`OutcomeUnknown` bedeutet, dass eine Mutation bereits angenommen worden sein
kann. Nicht blind erneut veröffentlichen. Context-Abbruch und Timeout sind mit
`errors.Is` prüfbar. `ErrBusy` verweigert gleichzeitige Operationen ohne Queue;
`Close` bricht auch einen laufenden Austausch ab. Keine automatischen Retries,
Neu-Verbindungen oder Backoff-Strategien.

`GenerateKey`, `PublicKey`, `TokenID`, `ClientID` liefern Registrierungsdaten;
`PrivateBytes` exportiert Rust/Python-kompatibles PKCS#8 v2. `ParseKey` akzeptiert
kanonisches PKCS#8 v1 und die exakt geprüfte Rust-v2-Struktur einschließlich
Abgleich des enthaltenen Public Keys. Private Schlüsseldateien mit 0600 speichern.
Go garantiert keine vollständige Löschung aller Schlüsselkopien aus dem Speicher.

`Rotate(ctx, newKey)` setzt voraus, dass der neue Schlüssel bereits sicher gespeichert
ist. Erfolg schließt die alte Sitzung; mit neuem Schlüssel und ursprünglicher
`Config.ClientID` neu verbinden. Bei unklarem Commit beide Schlüssel behalten
und den neuen Zugang prüfen. Das SDK schreibt selbst keine Dateien.

## Tests und Generierung

Als normaler Benutzer mit Rust/Cargo und Go:

```sh
./scripts/test-go-sdk.sh
# Optional zusätzlicher Test mit installiertem Python-SDK:
MYNOFY_PYTHON=/pfad/zum/venv/bin/python ./scripts/test-go-sdk.sh
```

Die Tests starten temporäre Rust-Services und prüfen Unix/WSS, TLS/Auth/ACL,
Bytes/Events, Integergrenzen, Cursor, Rotation und Rust-Schlüsselinterop.
Gemeinsame Wire-/Auth-/Rotationsvektoren gelten für Rust, Python und Go.
Fehlerpeers prüfen Antwortverlust, falsche Frames, Timeout, Cancellation und Busy;
`-race` prüft nebenläufige Zugriffe. Vorhandene Demo-Daten werden nicht verändert.

Bindings nur aus der kanonischen `.proto`-Quelle und dem Katalog regenerieren:

```sh
go install google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.12
# Python-Entwicklungsumgebung wie beim Python-SDK einrichten:
PROTOC_GEN_GO="$(go env GOPATH)/bin/protoc-gen-go" python sdks/go/generate.py
```

Kein Peer-/Owner-Tunnel, Admin-Zugang oder Workflow-/Polling-Helfer im ersten Go-SDK.
Für Owner-Tunnel bleibt der Rust-Client verfügbar. Weitere Sprachen: [SDK-Übersicht](../../docs/sdks.md).

## Owner-authenticated producer identity

Read `item.Message.GetProducerId()` for the stable client identity stamped by the owner.
It is immutable, survives authorized key rotation and storage restore, and is
separate from any sender or actor claim inside the payload. Match workflow roles
to this value. This is an assertion over the protected owner connection, not a
portable cryptographic message signature.
