# Offline-Werkzeuge und gezielte Speicherabbrüche

Die Offline-CLI bietet:

```sh
mynofy storage help
mynofy storage check --config config.yaml
mynofy storage inspect --config config.yaml
mynofy storage cleanup --config config.yaml
mynofy storage backup --config config.yaml --out backups/content-001
mynofy storage restore --config config.yaml --from backups/content-001
mynofy storage rekey --config config.yaml
mynofy storage topic-ram --config config.yaml --topic jobs/status
```

Die [vollständigen Betriebsregeln](restart-and-storage.md) gelten weiterhin: Dienst stoppen; Content und Security bleiben exklusiv gesperrt; Secrets nur über stdin; Restore verändert keine aktuelle Security und setzt den Referenz-Hochwasserstand nicht zurück; der explizite Rückwechsel zu RAM entfernt gespeicherte Topic-Dateien.

`inspect` liefert pro Topic Strategie, gespeicherte Revision, Dateigröße, aktive Nachrichtenzahl und Zahl der Bestätigungspaare. Nachrichteninhalte und Secrets werden nicht ausgegeben. Größen sind gespeicherte Topic-Dateibytes einschließlich Framing beziehungsweise Verschlüsselungsumschlag, keine RAM-/RSS-Messung.

`cleanup` validiert zuerst den vollständigen gespeicherten Stand und entfernt danach nur unreferenzierte interne Hash-/Slot-Dateien und die festen Staging-Namen `topic.tmp`, `state.tmp`, `references.tmp`. Aktive Dateien, Security, unbekannte Dateien und externe Backup-Verzeichnisse bleiben erhalten. Symlinks im unreferenzierten internen Namensraum werden selbst entfernt, nicht ihr Ziel. Eine erneute Bereinigung ist zulässig. Auch `topic-ram` kann nach einem bereits erfolgten Manifest-Commit wiederholt werden, um eine unterbrochene Dateibereinigung abzuschließen.

Unvollständige Kommandoargumente werden vor CFG-/Storage-Zugriff und Secret-Abfrage abgewiesen. `storage help` benötigt keine Konfiguration. Die bestehenden globalen Regeln zum frühen Rechteabbau gelten unverändert.

## Testablauf

```sh
./scripts/test-storage-crashes.sh
```

Alternativ eine einzelne Gruppe:

```sh
cargo test --locked -p mynofy-server storage_fault_empty_restore -- --nocapture
```

Die Tests sind Bestandteil von `cargo test --locked --workspace`. Sechs Matrix-Tests starten zusammen **176 echte Kindprozesse** mit privaten Testverzeichnissen und bekannten Test-Credentials. Ein eigener Worker-Test ist im normalen Lauf als `ignored` markiert und wird ausschließlich durch die Matrix mit `--ignored --exact` gestartet. Die tatsächlichen Fälle werden dadurch trotzdem ausgeführt.

Der Worker verwendet dieselben `Engine`-/`OfflineStorage`-Schreibpfade wie Dienst und CLI. An einem ausgewählten Schritt meldet er dem Parent über stderr, dass der Punkt erreicht ist. Bei Abbruchfällen wartet er dort ohne Busy-Loop; erst dann beendet ihn der Parent mit **SIGKILL** und wartet das Prozessende ab. Es gibt keine geschätzten Sleeps, um einen Schreibzeitpunkt zu treffen. Begrenzte Deadlines erkennen ausgebliebene Haltepunkte beziehungsweise hängende Worker.

Die Haltepunkte und Teil-Schreibvorgänge sind ausschließlich mit `cfg(test)` eingebaut. Im normalen Library-/CLI-Build werden keine Fault-Schalter, Umgebungsprüfungen, Wartepunkte oder zusätzlichen Teil-Writes erzeugt. Keine zusätzliche Registry-Abhängigkeit und kein eigenes unsafe.

## Matrix und Erwartungen

| Bereich | Geprüfte Grenzen |
| --- | --- |
| Topic-Dateien | Nach Truncate, echtem halbem Staging-Write, vollständigem Write, File-Sync, Rename und Verzeichnis-Sync; jeweils erste und zweite Topic-Datei. |
| Manifest | Halber/vollständiger Write, File-Sync, vor/nach Rename, vor/nach Verzeichnis-Sync. |
| Cleanup / Storage→RAM | Vor und nach Entfernen alter Slots sowie vor abschließendem Verzeichnis-Sync; Wiederaufnahme der Bereinigung. |
| Neuer Restore-Zielspeicher | Halbe Referenzdatei, Referenz-Rename und teilweise/vollständig geschriebener Initialisierungsmarker vor dem ersten Manifest. |
| I/O-Fehler | Simuliertes ENOSPC und EIO an Öffnen, Schreiben, File-Sync, Rename und Manifest-Verzeichnis-Sync für Save, Backup, Restore und Rekey. |

Von den 176 Fällen sind **109 SIGKILL-Abbrüche**, **34 ENOSPC-** und **33 EIO-Injektionen**. Die Hauptmatrix nutzt ein gemeinsames Raw-Key-Secret mit einem Klartext-Topic und einem verschlüsselten Topic. Zusätzliche Abbruchfälle verwenden vollständig unverschlüsselten Content sowie den Passwortmodus. Nachrichten nutzen echte 4096-Byte-Payloads; gespeicherte ACK-/NACK-Endzustände gehören zum Prüfbestand.

Nach jedem Fall wird der Speicher neu geöffnet und geprüft:

- vor dem Manifest-Commit bleibt die vorherige vollständige Generation lesbar;
- nach dem bereits sichtbaren Manifest-Rename ist bei diesem Prozessabbruch die neue vollständige Generation lesbar;
- niemals ein Topic aus der alten und das andere aus der neuen Generation;
- Inhalt, Payloadlänge, stabile Bestätigungsreferenzen sowie ACK-/NACK-Zustände bleiben korrekt;
- beim Rekey akzeptiert der gültige aktuelle Stand genau das zugehörige alte oder neue Secret;
- ein unterbrochenes Backup ohne Manifest wird nicht als vollständiges Backup angenommen; die Quelle bleibt bytegleich;
- Offline-Operationen lassen Security-Snapshot und Journal bytegleich und senken keinen vorhandenen Referenz-Hochwasserstand;
- beim Rückwechsel zu RAM bleibt das andere Topic erhalten; ein erneutes Öffnen/Bereinigen entfernt nur nicht mehr referenzierte Dateien.

## Geschlossene Wiederanlauflücken

Ein Abbruch zwischen Referenz-Rename und fertigem Initialisierungsmarker blockierte zuvor einen erneuten Restore in einen leeren Zielspeicher. Die Wiederaufnahme ist jetzt eng begrenzt: **kein publiziertes Manifest**, gültige Owner-/Checksum-Bindung der bereits gespeicherten Referenzdatei und ein leerer beziehungsweise gültiger Präfix des erwarteten Markers. Der Marker wird vollständig geschrieben und synchronisiert; der Hochwasserstand wird nicht neu berechnet oder abgesenkt. Ein beschädigter Marker neben einem vorhandenen Manifest bleibt ein Fehler.

Nach einem fehlgeschlagenen Manifest-Verzeichnis-Sync ist der sichtbare Commit unbestätigt. Der laufende Dienst meldet `content_storage.requires_restart: true`, lässt ungesicherte RAM-Änderungen dirty und weist weitere Saves bereits vor einer neuen Capture-Kopie mit `content_storage_requires_restart` ab. Normale Vorab-I/O-Fehler bleiben erneut versuchbar. Im Offline-Objekt verhindern dieselben Regeln insbesondere ein Backup oder Cleanup anhand der noch alten In-Memory-Dateiverweise. `check`/`inspect` kennzeichnen den Zustand mit `durability_uncertain`; vor weiterer Wartung muss neu geöffnet werden. Beim Öffnen wird der sichtbare Verzeichnisstand synchron bestätigt, bevor alte Slots wiederverwendet oder entfernt werden dürfen. Weitere ENOSPC-/EIO-Fälle prüfen, dass ein fehlgeschlagener Bestätigungs-Sync das Öffnen ohne Datenänderung abbricht.

Beim Schließen wird der eigene flock ausdrücklich freigegeben. Dadurch hält eine während eines parallelen Prozessstarts kurzzeitig geerbte Descriptor-Kopie das bereits beendete Owner-Objekt nicht weiter gesperrt. Ein eigener Test hält eine Descriptor-Kopie offen und prüft das erneute Öffnen.

Außerdem prüft Restore den vollständigen Initialisierungs-/Vergabestand des Backups, bevor Zieldaten oder Ziel-Hochwasserstand geändert werden. Eine beschädigte Backup-Metadatei wird nicht als fehlender neuer Stand interpretiert.

## Einordnung

Dies prüft reale Prozessabbrüche und simulierte I/O-Fehler auf dem Linux-Dateisystem der Testumgebung. Stromverlust, Controller-/Datenträgerfehler, Netzwerkdateisysteme und native macOS-Laufzeit sind damit nicht validiert. Auch ein erfolgreicher Rename ohne bestätigtes Verzeichnis-Sync ist keine Haltbarkeitszusage bei Stromverlust. Es wurden keine Performancewerte abgeleitet.
