# Security-Journal

Message content and security state have separate lifecycles. Security changes
apply immediately in RAM and enter an ordered append-only journal. By default,
successful service security-mutation responses wait for disk synchronization.
Explicit `sync: false` allows asynchronous confirmations and can lose unsynced
changes on a crash.

## Durable security confirmations

Durable confirmation is the default, including when the security storage section
is omitted. This explicit configuration selects the same default (YAML or JSON):

```yaml
security_storage:
  directory: ./security
  sync: true
```

Only set `sync: false` to opt out of durable confirmation.

`sync` defaults to `true`. With the default, successful security mutation responses on
the Admin socket and successful client key-rotation commits wait for the journal
sequence to be synchronized to disk. This includes client/key/ACL changes, peer
policy changes and topic security resets. It does not make message publications
or topic content durable; those retain their separate storage policy.

RAM changes and session invalidations still happen immediately. The service waits
outside the Engine mutex, on a blocking worker, with a five-second fence deadline;
other clients and Admin status reads can continue. The Admin connection itself
also has a bounded deadline. If synchronization fails, Admin returns
`security_storage_unsynced` when its deadline permits a response. The change is
already applied in RAM and may become durable later. Inspect status instead of
blindly retrying. A failed rotation confirmation closes the client connection,
preserving an unknown commit outcome; retain both keys until reconciled.

Admin status exposes `security_storage.sync`, `submitted_sequence`,
`durable_sequence` and `last_error`. The asynchronous journal and retry mechanism
are the same in both modes; `sync` changes the service's success-confirmation
policy. Embedded synchronous Engine APIs still modify RAM and enqueue records;
only the service transports perform the asynchronous confirmation wait.

## Einrichtung

Für den CLI-Dienst ist `security_storage.directory` in YAML oder JSON ausdrücklich zu konfigurieren:

```json
"security_storage": {
  "directory": "/var/lib/mynofy/security",
  "queue_capacity": 128,
  "compact_after_records": 4096
}
```

Relative Pfade beziehen sich auf das Konfigurationsverzeichnis. Ein neuer
Store wird einmal als endgültiger Dienstbenutzer mit
`mynofy security-init --config FILE` angelegt. `serve` verweigert fehlende,
leere oder einem anderen Owner zugeordnete Stores. Die Initialisierung baut
den Store im selben Elternverzeichnis auf und veröffentlicht ihn erst nach
Synchronisierung aller Bestandteile. Bei einer Wiederholung werden eindeutige
Staging-Reste verworfen und der Store aus der aktuellen Konfiguration neu aufgebaut.
Unerwartete Dateien und bereits veröffentlichte Stores führen zum Abbruch.
Während der Einrichtung braucht der Dienstbenutzer Schreibrechte im
Elternverzeichnis. Provisioniere den Store in einem privaten, schreibbaren
Parent und setze danach die vorgesehenen Betriebsrechte.

Bei einem Upgrade auf den bisherigen Store verweisen. Für Altinstallationen
mit implizitem Pfad war das üblicherweise `<config-Dateiname>.security`.
Bei Verlust den aktuellsten konsistenten Security-Store desselben Owners
bei gestopptem Dienst wiederherstellen. Bei älteren Sicherungen Widerrufe
und ACL-Änderungen seit der Sicherung vor Wiederinbetriebnahme nachziehen.
Der Elternpfad muss existieren, das Verzeichnis muss privat und der
Dienst-UID zugeordnet sein. Dateien sind private reguläre Dateien mit
Modus 0600 und ohne zusätzliche Hardlinks. Ein exklusives Dateilock
verhindert zwei Instanzen im selben Store.

Der Dateispeicher verwendet sichere Unix-APIs aus dem bereits vorhandenen offiziellen `rustix`-Crate. Linux ist getestet; native macOS-Validierung bleibt für GitHub CI vorgemerkt. Andere Plattformen unterstützen dieses Backend derzeit nicht.

Die einbindbare Core-API bleibt ohne Dateisystemzugriffe nutzbar: Ein direkt konstruiertes `Config` mit `security_storage: None` erzeugt keinen Writer und keinen Speicher. `Config::load` setzt dagegen stets einen Speicherpfad, auch bei fehlendem oder `null`-Feld. Eingebettete Netzwerk-Listener verlangen ebenfalls einen initialisierten Owner-Store.
Ein bewusst flüchtiger Listener kann über `transport::prepare_volatile` vorbereitet
werden, wenn Content und Security rein im RAM bleiben. `check-config` löst den Pfad auf, öffnet aber weder Journal noch Lock.

## Sicherheitszustand und Baseline

Gespeichert werden:

- Client-Registrierungen, stabile Identitäten, Anzeigenamen und Sperren;
- registrierte Public Keys und absolute Gültigkeitsgrenzen einschließlich Widerrufen;
- ACL-Ersetzungen für exakte lokale Topics;
- abgeschlossene Schlüsselrotationen als ein atomarer Client-Eintrag mit dem vollständigen resultierenden Keybestand.

Private Schlüssel, Shared Secrets, Challenges, Sessions, laufende Wechsel, Rate-Limiter, Nachrichten und Zustellreferenzen werden nicht gespeichert. Die Dateien sind kein Audit-Archiv: Ein Client-Eintrag beschreibt seinen resultierenden Sicherheitszustand, und Kompaktierung entfernt ältere Änderungen.

Beim ersten Start bilden Clients und lokale Topic-ACLs der Konfigurationsdatei die Security-Baseline. Sie wird vor Annahme von Clients gespeichert. Später ist der gültige Snapshot samt Journal für diese Felder maßgeblich; alte CFG-Schlüssel oder ACLs werden nicht zusätzlich eingemischt. Betriebskonfiguration, Topic-Definitionen/-Policies, TLS-Pfade und statische Peer-Einstellungen kommen weiterhin aus der CFG. Laufzeit-Peer-Verwaltung ist nicht implementiert.

Ein gespeicherter nicht leerer ACL-Eintrag für ein inzwischen fehlendes lokales Topic führt zum Startabbruch. Die Inhalte erzeugen niemals automatisch Clients oder Rechte. Namens-/Owner-Konflikte und ungültige Referenzen werden nicht durch permissive Defaults aufgelöst.

Der vorhandene Admin-Export ist weiterhin ein manuell angeforderter Konfigurationssnapshot. Er enthält jetzt auch den aufgelösten Security-Pfad und ersetzt das Journal nicht. Zum Start einer unabhängigen Kopie einen eigenen Security-Pfad wählen. Ein Reset des bestehenden Security-Speichers ist eine ausdrückliche Betreiberaktion bei gestopptem Dienst; er kann alte CFG-Schlüssel/Rechte wieder als Baseline aktivieren.

## Laufzeit, Fehler und Status

Eine eigene Writer-Thread verarbeitet die Änderungen in Core-Reihenfolge. Pro Änderung wird vor der RAM-Mutation ein Platz reserviert. Die Grenze umfasst auch gerade geschriebene oder zur Wiederholung zurückgehaltene Records. Default: 128, erlaubt: 1–4096. Bei voller Queue liefert die Operation `security_storage_busy`, ohne Sicherheitszustand zu ändern; bei ausgefallenem Writer `security_storage_unavailable`. Ein fehlgeschlagener Rotations-Commit kann seinen bereits geprüften, einmaligen Wechselvorgang konsumieren; der neue Versuch beginnt dann mit einer neuen Wechsel-ID. Datenoperationen erhalten keine Journalarbeit im Hotpath.

Der Writer fasst bis zu 32 bereits wartende Änderungen zu einem Schreib-/Synchronisierungsbatch zusammen, ohne künstliche Sammelwartezeit. Gespeicherte Änderungen werden erst nach erfolgreichem `sync_all` als dauerhaft gezählt. Bei Fehlern hält er den Batch zurück, meldet den Fehler auf stderr und im Admin-Status und versucht es nach 100 ms erneut. Ein teilweise geschriebener Batch wird vor Wiederholung auf seine ursprüngliche Dateiposition zurückgesetzt. Bei Kompaktierungsfehlern werden weitere Appends zurückgehalten; dadurch wächst das Journal nicht unbegrenzt.

`mynofy admin status --socket PATH` liefert zusätzlich `security_storage`:

| Feld | Bedeutung |
| --- | --- |
| `enabled` | Ein Security-Writer ist konfiguriert. |
| `submitted_sequence` | Letzte im RAM angewandte und eingereihte Sicherheitsänderung. |
| `durable_sequence` | Letzter erfolgreich synchronisierter Journaleintrag. |
| `snapshot_sequence` | Stand des zuletzt vollständig abgeschlossenen Checkpoints. |
| `pending_records` | Reservierte/eingereihte/gerade geschriebene Records. |
| `last_error` | Aktueller Schreib-/Kompaktierungsfehler oder `null`. |

Bei regulärem Freigeben des Engine-Zustands wird die Queue abgearbeitet und der Writer beendet. Bei fortbestehendem Speicherfehler wird ungespeicherter Zustand gemeldet, statt endlos neue Wiederholungen während des Beendens zu starten. Betriebssystem-I/O kann den Writer beziehungsweise das Beenden verzögern; daraus folgt keine Echtzeitgarantie. Ein SIGKILL kann noch nicht synchronisierte Änderungen verlieren. Die Haltbarkeit von `sync_all` setzt korrekt arbeitendes Dateisystem und Speichergerät voraus.

## Dateiformat und Wiederherstellung

Das veränderliche lokale Format v1 ist unabhängig vom Nachrichten-Wire-Protokoll und vom RAM-Recordlayout. Es gibt drei dauerhafte Dateien: `lock` (Initialisierungsmarker und Prozesslock), `snapshot` und `journal`. `snapshot.tmp` ist ausschließlich die austauschbare Arbeitsdatei der Kompaktierung.

Jeder Frame hat einen 80-Byte-Header: `MNS1` (4 Byte), Payloadlänge (u32 LE), Sequenz (u64 LE), SHA-256 über diese ersten 16 Bytes und SHA-256 über den Payload. Es folgt ein versionierter JSON-Payload. Die Header-Prüfsumme schützt insbesondere die Länge, bevor Speicher reserviert oder eine unvollständige Payload angenommen wird. Prüfsummen erkennen Beschädigung; sie sind keine Signatur gegen einen Angreifer mit Schreibrechten auf die Dateien.

Journal-Payloads sind auf 16 KiB begrenzt, Snapshot-Payloads auf 32 MiB. Die Serialisierung bricht bereits beim Erreichen der Grenze ab. Ein Snapshot enthält Owner-ID, Sequenz und ausschließlich Clients/Keys/Sperren/ACLs. Journal-Payloads enthalten `version: 1`, die geprüfte Owner-ID und entweder `client` mit vollständigem Clientzustand oder `acl` mit einem betroffenen Client/Topic-Paar.

Beim Start wird der Snapshot geprüft und anschließend das Journal sequenziell angewandt. Neue Records müssen die Sequenz lückenlos fortsetzen. Ein unvollständiger letzter Frame wird auf die letzte gültige Grenze gekürzt. Ein vollständiger Frame mit falscher Prüfsumme, unbekannter Version oder falscher Reihenfolge führt zum Startabbruch, ebenso ein fehlender initialisierter Speicherbestand, ein beschädigter Snapshot oder eine andere Owner-ID. Kein stiller Rückfall auf die möglicherweise veraltete CFG. Alte Records, die der Snapshot bereits enthält, werden geprüft und nicht erneut angewandt.

Nach erfolgreicher Wiederherstellung wird vor der Verbindungsannahme ein Checkpoint erstellt. Im Betrieb geschieht das ab 4096 Änderungen seit dem letzten Checkpoint (konfigurierbar 1–1.000.000) oder 8 MiB Journal. Höchstens ein Batch von 32 begrenzten Records kann diese Bytegrenze überschreiten; die Recovery lehnt Dateien über 16 MiB ab.

Der neue Snapshot wird vollständig geschrieben und synchronisiert, dann atomar umbenannt und sein Verzeichnis synchronisiert. Erst danach wird das alte Journal gekürzt und synchronisiert. Ein Absturz dazwischen lässt entweder den alten Snapshot mit Journal oder den neuen Snapshot mit noch vorhandenen, bereits abgedeckten Records zurück. Wiederholungen dürfen keine neueren Sicherheitsänderungen überschreiben.

## Verifikation

Unit-Tests prüfen Journalsequenzen, abgebrochene letzte Frames, beschädigte Header/Payloads, Versionsfehler, Snapshot-Prüfsummen, fehlende Baseline, Dateilocks, private Pfade, Symlinks, Größenlimits und Wiederholung nach Schreibfehler. Core-Tests prüfen sämtliche Mutationsarten samt Rotation, nicht persistierte Sessions/Wechsel/Inhalte, sofortige RAM-Wirkung bei blockierter Kompaktierung und Ablehnung vor Mutation bei voller Queue. Ein CLI-Prozesstest beendet den Dienst nach bestätigter Synchronisierung per SIGKILL und prüft Wiederanlauf und fortbestehende Rechte bei leerem Nachrichtenbestand. Das simuliert keine physische Stromunterbrechung; entsprechende Absturzgrenzen werden separat mit konstruierten Dateiständen geprüft.

Das separate [Content-Save-Backend](content-storage.md) ist implementiert, einschließlich optionaler Dateiverschlüsselung und vollständigem Start-Restore. Es ersetzt oder überschreibt den Security-Stand nicht.

## Synchroner Neustartzähler

Der Security-Snapshot enthält zusätzlich `restart_counter: u32` (Default 0 für ältere Abbilder). Beim Dienststart wird er nach Recovery mit Umlauf erhöht und synchron im Start-Checkpoint gespeichert, bevor Clients angenommen werden. Dieser Boot-Schritt ist die beschlossene Ausnahme zur asynchronen Laufzeitmutation. Er wird im Auth-Welcome und Admin-Status ausgegeben. Offline-Content-Werkzeuge halten das Security-Lock und lesen die aktuellen Rechte, erhöhen aber den Zähler nicht und ersetzen keine Security-Konfiguration.


## ACL-Lebenszyklus dynamischer Topics

Die [Laufzeitverwaltung](runtime-topics.md) ergänzt `topic_reset`: Create löscht bisherige ACLs des Namens und markiert ihn in `transient_topics`, Delete entfernt ACLs und Marker. So bleiben Sicherheitsänderungen automatisch asynchron gespeichert, während die Topic-Definition weiterhin RAM-/Content-Zustand ist. Beim Recovery werden ACLs zu markierten Topics verworfen, die weder CFG noch Content-Checkpoint enthält. Nicht markierte fehlende lokale Topic-Ziele bleiben Fehler. Bei Neuanlage gelten immer leere ACLs. Ein gespeichertes dynamisches Topic wird vor Security-Recovery aus dem Content vorbereitet und erhält seine aktuellen gespeicherten Rechte.

Ältere Snapshots ohne `transient_topics` werden weiterhin gelesen. Nach neuen `topic_reset`-Records beziehungsweise Snapshots mit Markern ist ein Downgrade auf ein Binary ohne diese Unterstützung nicht vorgesehen.

## Oversized-record admission

The concrete owner/change envelope is serialized through the same bounded
16-KiB record encoder before it can enter the write queue. A runtime peer update
submits this validated change before replacing its RAM policy. An admin request
that fits the 16-KiB input limit can therefore still return
`security_record_too_large`; input and journal envelopes have different overhead.
That rejection consumes no sequence or queue entry and does not block subsequent
valid security changes. Filesystem failures retain asynchronous retry behavior.

## Total-state admission

`security_snapshot_too_large` rejects a mutation before RAM changes when the
resulting security state would exceed its recoverable snapshot budget. Accounting
includes queued changes and reserves space for the owner/envelope and counter
growth; deletions release capacity. Individual records retain their separate
16-KiB check. Normal disk I/O remains asynchronous. See the
[storage and restart guide](restart-and-storage.md).


### Recovery floor and planned shutdown

A deployment may set `security_storage.minimum_sequence` from a trusted
record of the last acceptable durable security sequence. Startup and offline
content maintenance reject a replay below that value. Keep the reference
outside the restored security volume. After restoring an older generation,
review ACLs and revocations before admitting clients.

The Admin listener reserves time to send a structured failure after a
durability wait. A supervised shutdown reports a drain that lasts over ten
seconds. The process continues to hold its store locks while I/O finishes.


`check-config` validates configuration statically. A positive
`minimum_sequence` requires a qualifying recovered store, so
`security-init` rejects that setting before writing a new store.
Normal SIGTERM stops acceptance, closes sessions and drains the writers.
If the accepted security sequence exceeds the durable sequence after drain,
`serve` exits with an error and logs `security_drain_failed` with both
sequence numbers. Embedded owners can pass a shutdown future to
`run_prepared_until` or `run_unix_prepared_until` to receive the same result.


Der Replay-Präfix wird gegen `minimum_sequence` geprüft. Beim Dienststart
wird der akzeptierte Präfix erst nach erfolgreicher gemeinsamer Prüfung von
Content und Security kompaktiert. Eine Ablehnung bis zu diesem Punkt erhält
die Journal-Bytes. Explizite Offline-Wartung kann ein unvollständiges Ende
eines akzeptierten Präfixes reparieren.
