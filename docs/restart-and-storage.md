# Neustarterkennung und Offline-Speicherwartung

## Service-ID und Neustartzähler

`service.id` bleibt die dauerhafte Identität; es gibt keine zusätzliche Instanz-ID. Der Dienst lädt den Security-Speicher, erhöht dessen `restart_counter: u32` mit Umlauf und synchronisiert den neuen Stand, bevor ein Listener Clients annehmen kann. Fehlgeschlagene Starts dürfen Werte verbrauchen. Ein Fehler bei diesem Speicherschritt bricht den Start ab. Normale Security-Änderungen bleiben wie bisher asynchron; der Startzähler ist die ausdrücklich beschlossene synchrone Ausnahme.

Der Zähler liegt im vorhandenen Security-Snapshot. Ältere Snapshots ohne Feld beginnen beim ersten aktualisierten Start mit 1. Beim Überlauf folgt auf 4294967295 die 0. Clients prüfen **Ungleichheit**, nicht Größenordnung. Nach exakt 2^32 unbeobachteten Starts bleibt der Wert mehrdeutig. Ein direkter eingebetteter `Engine` ohne Security-Backend bleibt dateifrei und verwendet 0; für dauerhafte Neustarterkennung das Security-Backend konfigurieren. `Config::load` aktiviert es für den CLI-Dienst ohnehin.

Auth-Welcome enthält jetzt:

```json
{"version":1,"type":"auth_welcome","request_id":"auth-proof","client_id":"client:ed25519:…","service_id":"local","restart_counter":12}
```

Der Admin-Status liefert denselben Zähler. Er erscheint nicht in jedem Nachrichtenrecord oder Cursor. Das achtstellige Cursorformat bleibt unverändert. Im Protobuf ist das Feld optional repräsentiert, um fehlende Werte von gültiger 0 unterscheiden zu können; der Client verlangt beim Authentifizieren ausdrücklich seine Anwesenheit. Server und Clients gemeinsam aktualisieren.

## Client-Fortschritt und Replay

Der verwaltete SDK-Client `mynofy_client::progress::RestartClient` erhält eine authentifizierte Verbindung, deren Welcome und einen `ClientProgress`. `connect_client_info` beziehungsweise `connect_unix_client_info` liefern Socket und Welcome zusammen. Fortschritt ist an Service-ID und stabile Client-ID gebunden; die Anwendung bewahrt ihn für den nächsten Verbindungsaufbau auf.

Bei geändertem Counter werden gespeicherte Topic-Cursor verworfen. Der erste erfolgreiche Poll je Topic startet ohne bisherigen Cursor, auch wenn der Aufrufer einen alten, noch syntaktisch gültigen Cursor übergibt. Ein abgewiesener Poll verbraucht diesen Replay nicht. `poll(topic, limit)` verwendet danach den aktualisierten eigenen Fortschritt. Legacy confirmation records are discarded when loading content. Nachrichten dürfen erneut erscheinen; automatische Neuveröffentlichung, erneute Ausführung oder genau einmalige Verarbeitung werden daraus nicht abgeleitet.

Die bisherigen Funktionen `connect_client`, `connect_unix_client`, `exchange` und `exchange_frame` bleiben direkte Socket-APIs ohne verwalteten Fortschritt. Ein Wrapper ist nötig, damit kein globaler unbeschränkter Zustand pro Owner/Client entsteht. Er begrenzt den eigenen Fortschritt auf 10.000 Topics. Es gibt keinen automatischen Verbindungs-/Command-Retry.

Die CLI kann ihren Fortschritt zwischen Aufrufen speichern:

```sh
mynofy client --socket run/mynofy.sock --key client.pk8 --owner local \
  --progress .local/client-progress.json
```

Alternativ dieselbe Option beim WSS-Client verwenden. JSONL-Requests und Antworten bleiben gleich. Die Fortschrittsdatei enthält Owner, Client-ID, Counter und Cursor, keine Credentials. Privates reguläres File, No-Follow, Verzeichnisprüfung, exklusives Lock, begrenztes Einlesen und atomare synchronisierte Ersetzung schützen den Zustand. Parallelverwendung derselben Fortschrittsdatei wird abgewiesen. Auch ein noch ausstehender Cursor-Reset bleibt über einen Client-Abbruch hinweg gespeichert. Nach einer Poll-Antwort schreibt die CLI den Fortschritt erst nach erfolgreichem stdout-Flush; eine atomare Verbindung zur Verarbeitung durch den Empfänger entsteht dadurch nicht. Ohne `--progress` bleibt der bisherige direkte CLI-Modus erhalten. Der Adapter beginnt ohnehin ohne Cursor und bleibt ohne dauerhaften Workflow-Checkpoint.

## RAM → Storage im laufenden Dienst

Das globale `content_storage` mit Verzeichnis und gegebenenfalls gemeinsamem Start-Secret muss bereits beim Start konfiguriert sein. Auch dann bleiben Topics standardmäßig RAM. Das Backend wird nicht dynamisch installiert oder nachgeladen.

Über den getrennten lokalen Admin-Socket kann ein RAM-Topic aktiviert werden:

```sh
mynofy admin topics storage jobs/status --mode manual --encrypted true \
  --socket run/mynofy-admin.sock

mynofy admin topics storage machines/status --mode interval --seconds 60 \
  --encrypted false --socket run/mynofy-admin.sock
```

Der Wechsel wirkt sofort im RAM. Er startet keinen impliziten Save; bei `interval` greift der konfigurierte nächste Termin. Der erste erfolgreiche Save sichert sämtliche dann noch aktiven Nachrichten des Topics, einschließlich der vor dem Wechsel entstandenen. Ohne erfolgreichen Save fällt die Laufzeitaktivierung beim Neustart auf die bisherige CFG-/gespeicherte Baseline zurück. Mit erfolgreichem Save stellt der Start die im Manifest gespeicherte Strategie wieder her, auch wenn die ursprüngliche CFG noch RAM enthält.

```sh
mynofy admin content save --topics jobs/status --socket run/mynofy-admin.sock
mynofy admin status --socket run/mynofy-admin.sock
```

Nur RAM → manual/interval ist online erlaubt. Weitere Storage-/Parameterwechsel und Storage → RAM werden online abgewiesen. Während eines laufenden Saves wird die Aktivierung ebenfalls abgewiesen, um keine widersprüchlichen erfassten Strategien zu erhalten. Der normale Save-Status unterscheidet weiterhin Queue-Annahme und dauerhaften Abschluss. Es gibt keine automatische CFG-Rückschreibung.

## Offline-Werkzeuge

Alle folgenden Kommandos öffnen keine Listener. Sie halten das Content-Lock und das bestehende Security-Lock; bei laufendem Dienst scheitern sie. Der Security-Zähler wird dabei nicht erhöht. Sicherheitszustand, Schlüsselwiderrufe und ACLs werden nicht aus einem Inhaltsbackup ersetzt. Vorhandene Security-Journale werden wie beim Start gelesen; ein abgerissener letzter Record darf nach der bestehenden Recovery-Regel gekürzt werden.

### Prüfen

```sh
mynofy storage check --config config.yaml
mynofy storage inspect --config config.yaml
mynofy storage cleanup --config config.yaml
```

`inspect` ergänzt Topic-Metadaten ohne Payloads; `cleanup` entfernt nach erfolgreicher Prüfung nur verwaiste interne Slots und Staging-Dateien. Einzelheiten und [gezielte Abbruchtests](storage-interruption-tests.md).

Liest, entschlüsselt und prüft Manifest, alle referenzierten Topic-Abbilder, Owner-/Clientbindungen, Vergabemetadaten und aktive konfigurierte Quoten. Liefert eine kleine JSON-Zusammenfassung. Fehlende, defekte oder inkompatible Daten werden nicht als gültiger Leerstand behandelt. Alte einzelne Owner-Checkpoints zunächst durch einen regulären Start in das Topic-Format migrieren.

### Backup

```sh
mynofy storage backup --config config.yaml --out backups/content-001
```

Das Ziel muss ein **neues** Verzeichnis sein; bestehende Ziele werden nicht überschrieben. Gesichert wird der letzte gespeicherte Content-Stand: Manifest, referenzierte Topic-Dateien und ein mindestens gleich hoher technischer Referenzstand. Unverschlüsselte Topics bleiben unverschlüsselt; verschlüsselte Dateien behalten ihren bestehenden Umschlag. Das Backup enthält keine Security-Konfiguration, TLS-Dateien oder Client-Privatschlüssel. Ein fehlerhaft abgebrochenes Ziel kann unvollständig existieren; ohne gültiges Manifest ist es kein erfolgreiches Backup. Die Quelle bleibt unverändert.

### Restore

```sh
mynofy storage restore --config config.yaml --from backups/content-001
```

Stellt Inhalte desselben `service.id` wieder her. Die aktuelle Security bleibt maßgeblich; fehlende erforderliche Identitäten werden nicht aus dem Backup angelegt. There is no delivery-ID allocator or retained confirmation state. Ein neuer leerer Content-Zielordner ist zulässig; ein vorhandener beschädigter Stand wird nicht stillschweigend als leer behandelt.

Der Restore schreibt zunächst in unreferenzierte Topic-Slots und ersetzt erst dann das Manifest. Fehler vor diesem Commit lassen den bisherigen Inhaltsstand nutzbar. Ein bereits erhöhter technischer Vergabestand darf bei einem fehlgeschlagenen Restore als sichere Lücke zurückbleiben. Bei einem Sync-Fehler nach Manifest-Rename bleibt die bestehende Regel: Haltbarkeit unbestätigt, keine weiteren Schreiboperationen auf dem geöffneten Store bis zum Neustart.

Verwendet das Backup ein älteres Secret desselben Passwort-/Key-Modus:

```sh
mynofy storage restore --config config.yaml --from backups/content-001 \
  --backup-secret stdin
```

Zuerst wird das aktuelle Ziel-Secret, danach das Backup-Secret über stdin gelesen. Das importierte Backup wird beim Commit mit dem aktuellen Ziel-Secret gespeichert. Unterschiedliche globale Verschlüsselungsmodi sind in diesem Schritt kein impliziter Migrationspfad.

### Gemeinsames Secret wechseln

```sh
mynofy storage rekey --config config.yaml
```

Liest das aktuelle Secret, das neue Secret und dessen Bestätigung ausschließlich über stdin; Terminaleingaben sind verdeckt. Eine ungleiche Bestätigung bricht vor dem Schreiben ab. Bei umgeleitetem stdin entsprechend drei Zeilen liefern. Der Modus `password_stdin` beziehungsweise `key_stdin` bleibt erhalten. Alle verschlüsselten Topics und das Manifest wechseln gemeinsam; unverschlüsselte Topics behalten ihre Strategie. Die CFG wird nicht verändert. Alte, separat erzeugte Backup-Verzeichnisse behalten ihr bisheriges Secret.

Ein fehlgeschlagener Commit vor Manifest-Rename lässt das alte Secret nutzbar. Erst nach dem gemeinsamen Commit werden die ersetzten Slots entfernt. Bei unbestätigtem Post-Rename-Sync muss der tatsächlich sichtbare Stand beim erneuten Öffnen geprüft werden; kein automatischer Rückfall auf Klartext oder ein zweites Secret.

### Storage → RAM

Zuerst den Dienst stoppen und für das Topic in der CFG `storage: {mode: ram}` setzen. Danach:

```sh
mynofy storage topic-ram --config config.yaml --topic jobs/status
```

Das Kommando entfernt das Topic aus dem gespeicherten Content und löscht seine internen Topic-Dateien; es bleibt keine automatische Inhaltskopie dieses Topics im Store erhalten. Andere Topics und Security bleiben erhalten. Ein späterer Dienststart verwendet für dieses Topic die RAM-Baseline. Ein CFG-Edit allein setzt keine bereits gespeicherte Storage-Strategie zurück: Er wäre nicht von der ursprünglichen RAM-Baseline vor einer erfolgreichen Online-Aktivierung unterscheidbar. Eigenständig angelegte externe Backup-Verzeichnisse werden nicht automatisch verändert.

## Tests und verbleibende Grenzen

Unit-Tests prüfen den synchronen Counter, Fehler beim Schreiben, U32-Umlauf, Aktivierung mit/ohne Save, vorbestehende Inhalte, Online-Ablehnung des Rückwechsels, Offline-Locks, atomare Fehlerfälle von Restore/Rekey, Restore in einen leeren Store, unveränderte aktuelle Security und niemals zurückgesetzte Vergabestände. SDK-Tests prüfen Counter-Ungleichheit einschließlich Umlauf, mehrere Topics, erfolglose Polls, Bindung an den Owner und Ablehnung eines Welcome ohne Counter. Ein echter CLI-Prozesstest deckt Aktivierung, Save, hartes Beenden, fehlgeschlagenen Boot-Counter-Write vor Bindung, Cursor-Replay nach Neustart, Backup, Rekey, Restore mit altem Backup-Secret und Offline-RAM-Löschung ab.

Linux ist hier geprüft; native macOS-CI bleibt offen. Physische Stromausfälle werden damit nicht behauptet. Performance-Messungen und weitere Speicheroptimierungen folgen als eigener nächster Schritt.

Abschließend bestanden auf Linux 176 Workspace-Tests, die erweiterte Funktionstestkampagne (64.000 Modellschritte und acht Kampagnen mit je 16.384 Fällen), Formatprüfung, Clippy mit Warnungen als Fehler sowie die isolierten Client- und Protokollprüfungen.

Die anschließende [Offline-Härtung und Abbruchmatrix](storage-interruption-tests.md) ergänzt Inspect/Cleanup, wiederaufnehmbare Erstinitialisierung und vorgezogene Save-Ablehnung bei unbestätigtem Commit. Das vorstehende Testergebnis beschreibt den Stand vor dieser Erweiterung; der aktuelle Prüfbericht ist dort verlinkt.

Ein dateifreier eingebetteter Owner meldet 0 ohne dauerhafte Neustartgarantie. `RestartClient` verwirft bei jedem Welcome mit 0 gespeicherte Cursor; auch ein persistenter Umlauf auf 0 führt sicherheitshalber zum Replay. Direkte Socket-Nutzer müssen dieselbe Regel selbst anwenden.

Progress saves create a private, randomly named staging file exclusively in the
checked parent directory. An existing `<progress-path>.tmp` neighbor is preserved.
Ordinary failures clean up only the current save's own staging file; an abrupt
kill can leave an orphaned file, which later saves do not adopt or truncate.

Both CLI JSONL client modes limit each input line to 64 KiB including its optional
newline and terminate locally on overflow, even if stdin remains open. Explicit
`expires: null` is invalid command input; omit `expires` to use owner policy.
Backup and Admin file output report parent-directory synchronization failures
rather than claiming a durable newly created name.
