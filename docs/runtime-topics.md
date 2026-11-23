# Lokale Topics zur Laufzeit verwalten

Die separate Admin-API unterstützt `topics list|get|create|policy|delete`. Nachrichtenclients erhalten damit keine neuen Verwaltungsrechte; ausschließlich der lokale Admin-Socket erlaubt diese Befehle. Owner bleibt immer die lokale `service.id`. Remote-Routen lassen sich darüber nicht verändern.

```sh
mynofy admin topics list --socket run/admin.sock --limit 20
mynofy admin topics get jobs/status --socket run/admin.sock
mynofy admin topics create jobs/status --default-ttl 60 --max-ttl 3600 --socket run/admin.sock
mynofy admin acl set CLIENT jobs/status --allow publish,subscribe,inspect_own --socket run/admin.sock
mynofy admin topics policy jobs/status --default-ttl 120 --max-ttl 7200 --socket run/admin.sock
mynofy admin topics delete jobs/status --socket run/admin.sock
```

`create` verlangt eine ausdrückliche Policy, erzeugt ein RAM-Topic und vergibt keine ACL-Freigaben. Ein Publish erzeugt niemals implizit ein Topic. Namen müssen wie in der Startkonfiguration eindeutig, nicht leer und höchstens 256 UTF-8-Bytes lang sein; höchstens 10.000 konfigurierte Topics einschließlich Remote-Routen. TTL: `1 <= default <= max <= 31.536.000` Sekunden. Ein vorhandenes lokales oder konfiguriertes Remote-Topic führt zu `conflict`.

`list` liefert lokale Topics in lexikalischer Reihenfolge. `limit` ist 1–100, Default 20; `--after NAME` setzt exklusiv nach diesem Namen fort. `next_after: null` kennzeichnet das Ende. Zwischen Seiten darf sich die Liste ändern; es ist kein eingefrorener Snapshot. Ein inzwischen entfernter Anker ist zulässig. `get` und Listeneinträge zeigen Owner, Policy, Storage-Strategie sowie aktive und noch physisch residente Nachrichten. Diagnosezähler scannen den Bestand; diese Admin-Operationen sind keine konstantzeitigen Datenpfad-Requests.

## Policy und vorhandene Inhalte

`policy` ändert die TTL-Vorgaben für künftige Publishes. Bereits akzeptierte Nachrichten behalten ihre Ablaufzeit, auch wenn das neue Maximum kürzer ist. Bei persistenten Topics wird die Änderung als dirty erfasst und erst durch einen erfolgreichen Save dieses Topics dauerhaft. Ungültige Änderungen werden vor der Mutation abgewiesen. Es gibt kein beliebiges Update von Listenern, Ressourcenlimits oder Routing.

## Entfernen und Neuanlage

`delete` akzeptiert ausschließlich vollständig geleerte RAM-Topics. Aktive oder noch nicht physisch bereinigte abgelaufene Nachrichten führen zu `topic_not_empty`. Es gibt keinen Force-Schalter und keine vorzeitige Löschung von Nachricht/Client-Bestätigungen. Die normale budgetierte Bereinigung läuft weiter; danach kann der Admin den Befehl erneut ausführen.

Bei `manual` oder `interval` lautet der Fehler `storage_transition_forbidden`, auch wenn der Bestand leer ist. Zuerst den Dienst stoppen, CFG-RAM-Baseline setzen und den bestehenden Offline-Befehl `storage topic-ram` verwenden; dessen Regeln stehen unter [Neustart und Speicher](restart-and-storage.md). Nach dem nächsten Start lässt sich das RAM-Topic entfernen. Ein Online-Delete umgeht damit nicht die beschlossene Offline-Grenze.

Delete entfernt alle ACL-Freigaben des Topics und invalidiert Revisionen, Sessions und ausstehende Wechsel der betroffenen Clients. Andere Topics und deren Nachricht/Client-Referenzen bleiben gültig. Numerische Slots werden kompakt gehalten: Bei Bedarf verschiebt sich der letzte Topic-Slot samt Record-Verweisen, ACLs und Content-Zeitplan atomar unter dem Core-Lock. Diese Admin-Arbeit kann proportional zum Bestand sein; es gibt dafür keinen zusätzlichen Scan im Publish-/Poll-Hotpath.

Ein später neu angelegtes Topic gleichen Namens hat eine leere ACL. Der initiale Kopfstand liegt beim globalen Annahmestand des laufenden Owners, damit alte Cursor im selben Boot neue Nachrichten nicht durch wiederverwendete kleine Positionen überspringen. Neustart-/Umlaufgrenzen der kompakten Cursor bleiben wie bisher.

## Speicherung und Neustart

Topic-Definitionen und Policies gehören zum RAM-/Content-Zustand, ACLs zum Security-Zustand:

- Neue RAM-Topics verschwinden beim Neustart, sofern sie nicht ausdrücklich exportiert und als neue Startkonfiguration verwendet werden.
- Für ein neues Topic kann der bestehende Befehl `topics storage` RAM→manual/interval aktivieren, sofern Backend und gegebenenfalls Secret beim Start provisioniert wurden. Ein anschließender erfolgreicher Content-Save speichert das Topic einschließlich Policy und gegebenenfalls Nachrichten. Start-Restore stellt es auch ohne Eintrag in der ursprünglichen CFG wieder her.
- Create-/Policy-Antworten melden `durable: false`: Die RAM-Wirkung bestätigt keinen Save. Status und Content-Save zeigen den dauerhaften Stand.
- Die Entfernung eines ursprünglichen CFG-RAM-Topics verändert die CFG nicht. Bei Neustart mit derselben Datei erscheint dessen Definition wieder, nach synchronisiertem Security-Reset aber ohne die entfernten Freigaben. Für die dauerhafte Änderung exportieren und die exportierte Konfiguration ausdrücklich beim nächsten Start verwenden.
- `security_storage.sync: true` confirms the Create/Delete security reset only after journal synchronization; it does not persist the topic definition or message content.
- Create/Delete erzeugen automatisch einen begrenzten asynchronen Security-Record `topic_reset`. Er setzt ACLs zurück und verwaltet einen Marker für dynamische Topics; er speichert keine Policy und macht ein RAM-Topic nicht dauerhaft. Beim Recovery werden ACLs markierter Topics verworfen, wenn weder CFG noch Content sie wiederherstellt. Fehlende nicht markierte lokale ACL-Ziele bleiben Recovery-Fehler.

Wie alle Security-Mutationen warten diese Antworten nicht auf den Datenträger. Eine volle Security-Queue weist Create/Delete vor der Wirkung ab. Nach einem Absturz vor dem Security-Sync kann noch der vorherige Sicherheitsstand gelten; `security_storage` zeigt eingereihte und dauerhafte Sequenz. Alle Topologie-/Policy-Mutationen werden während eines laufenden oder unbestätigten Content-Saves abgelehnt (`content_save_busy` beziehungsweise `content_storage_requires_restart`). Es gibt kein Laufzeit-Nachladen.

## Admin-JSON und Tests

Beispiel auf dem separaten JSONL-Admin-Socket:

```json
{"version":1,"command":{"op":"topic_create","topic":"jobs/status","policy":{"default_ttl_seconds":60,"max_ttl_seconds":3600}}}
```

[Schema](../protocol/admin/v1.schema.json) und [Beispiele](../protocol/admin/examples.json) enthalten alle neuen Operationen. Das normale Protobuf-Nachrichtenprotokoll bleibt unverändert.

Core-Tests prüfen Default-Deny, Paging, atomare Validierung, unveränderte alte expires-Werte, Queue-/Busy-Ablehnung, Record-/ACL-/Schedule-Slotverschiebung, Bestätigungsreferenzen, Cursor nach Neuanlage, Export und Neustart mit verlorenen RAM- beziehungsweise gesicherten neuen Topics. Ein echter CLI-/Unix-Socket-Test prüft Anlegen, Anzeigen, Paging, Policy, ACLs, Export und Entfernen. Die bestehende Abbruchmatrix läuft unverändert mit der gesamten Workspace-Suite.
