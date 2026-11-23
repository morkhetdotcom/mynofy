# Topic-Speicherung, Start-Restore und gemeinsames Secret

RAM bleibt der Standard. Jeder lokale Topic-Owner legt die Speicherstrategie seines Topics in der CFG fest. Sicherheitsänderungen werden unabhängig davon weiterhin automatisch im [Security-Journal](security-journal.md) gespeichert. Publish bestätigt RAM-Annahme; Poll liest den RAM-Bestand; ein erfolgreicher Save erhält den erfassten Stand für einen Neustart.

## Konfiguration

Globale Einstellungen zur bestehenden Service-CFG hinzufügen:

```yaml
content_storage:
  directory: content
  encryption: password_stdin
  max_snapshot_bytes: 134217728
```

Beim jeweiligen Eintrag in `topics` ergänzen:

```yaml
# Flüchtige Kommunikation; auch ohne storage-Angabe der Default.
- name: agents/communication
  # owner_id, policy und acl wie bisher konfigurieren.
  storage:
    mode: ram

# Bewusst gesicherter Zustand, mit Dateiverschlüsselung.
- name: jobs/status
  storage:
    mode: manual
    encrypted: true

# Periodische Sicherung bei Änderungen.
- name: machines/status
  storage:
    mode: interval
    seconds: 60
    encrypted: false
```

Die Topic-Beispiele zeigen nur die neuen Felder; `owner_id`, TTL-`policy` und `acl` bleiben erforderlich. Speicherregeln sind Owner-Konfiguration und werden nicht durch Producer oder Topic-ACL geändert. Remote-Topics dürfen keine eigene Speicherstrategie besitzen.

| Topic-Strategie | Verhalten |
| --- | --- |
| `ram` | Keine Nachrichten oder Topic-Kopfstände speichern. |
| `manual` | Nur durch expliziten Admin-Save sichern. |
| `interval` | Bei geändertem Topic-Zustand regelmäßig sichern; manueller Save ebenfalls möglich. |

`seconds` erlaubt 1–2.592.000 Sekunden. `encrypted` ist bei gespeicherten Topics standardmäßig false; ein verschlüsseltes Topic erfordert global `password_stdin` oder `key_stdin`. Ohne `content_storage` sind ausschließlich RAM-Topics zulässig. Ein Verzeichnis allein macht kein Topic persistent. RAM→Storage kann über die Admin-API aktiviert werden; dauerhaft wird die Änderung erst durch einen erfolgreichen Save. Storage→RAM erfolgt ausschließlich über das Offline-Werkzeug. [Bedienung](restart-and-storage.md).

`directory` ist absolut oder relativ zur CFG. Das Elternverzeichnis muss existieren und dem endgültigen Dienstbenutzer zugänglich sein. Der Dienst erstellt sein Verzeichnis mit 0700 und Dateien mit 0600; sie müssen der Dienst-UID gehören. Symlinks, zusätzliche Hardlinks und fremde Dateieigentümer werden abgewiesen. Ein exklusives Lock verhindert zwei Instanzen im selben Verzeichnis. Content und Security benötigen getrennte Verzeichnisse.

## Ein gemeinsames Start-Secret

| Globales `encryption` | Eingabe beim Start |
| --- | --- |
| `none` (Default) | Keine Eingabe; ausschließlich unverschlüsselte Topic-Abbilder. |
| `password_stdin` | Eine Passwortzeile aus stdin. |
| `key_stdin` | Kanonisches Standard-Base64 eines symmetrischen 32-Byte-Schlüssels aus stdin. |

```sh
mynofy serve --config config.yaml
```

Das Secret wird genau einmal gelesen und für alle verschlüsselten Topics verwendet. Die Owner-Metadaten im Manifest werden bei aktiviertem globalen Verschlüsselungsmodus ebenfalls verschlüsselt, auch wenn einzelne Topic-Abbilder unverschlüsselt bleiben.

Am Terminal ist die Eingabe verdeckt; normale Rückkehr und Eingabefehler stellen das Echo wieder her. Bei umgeleitetem stdin wird eine begrenzte Zeile ohne Prompt gelesen. Entfernt wird ausschließlich LF beziehungsweise CRLF; Passwort-Leerzeichen bleiben erhalten. EOF ohne Zeilenumbruch ist zulässig. Leere Eingaben und Eingaben über 4096 Byte werden abgewiesen.

Secrets stehen weder in CFG/argv/env noch im Admin-Export oder Status. `check-config`, Konvertierung und Admin-Kommandos fragen sie nicht ab. Das Secret bleibt für Saves im laufenden Prozess in einem `Zeroizing`-Puffer. Inhalte liegen während des Betriebs entschlüsselt im RAM; diese Option schützt gespeicherte Dateien. Das Security-Journal bleibt separat.

Ein Topic-Abbild trägt seinen tatsächlich gespeicherten Verschlüsselungsmodus im Manifest. Eine Änderung von `encrypted` wirkt beim nächsten Save dieses Topics; das bisherige Abbild wird beim Start weiterhin mit seinem gespeicherten Modus gelesen. Ein Wechsel des Secrets erfolgt ausdrücklich offline mit `storage rekey`; der globale Secret-Modus bleibt dabei gleich. Kein impliziter Rekey oder Klartext-Fallback.

## Einzelne Topics oder Gruppen speichern

```sh
# Alle Topics mit manual oder interval gemeinsam erfassen.
mynofy admin content save --socket run/mynofy-admin.sock

# Nur diese Topics gemeinsam erfassen.
mynofy admin content save --topics jobs/status,machines/status --socket run/mynofy-admin.sock

mynofy admin status --socket run/mynofy-admin.sock
```

Ein Save erfasst alle ausgewählten Topics unter derselben Owner-Synchronisierung. Zugehörige Nachrichten und Payloads werden kopiert. RAM-Topics und nicht ausgewählte Topic-Inhalte werden nicht kopiert. Der Erfassungsvorgang benötigt CPU und zusätzlichen Speicher proportional zum gewählten Bestand; er ist keine konstante Echtzeitoperation. Register und Owner-Vergabestände werden gemeinsam erfasst.

Die Antwort `{"queued":true,"revision":N}` bestätigt nur die eingereihte Kopie. Anschließend laufen Rahmenbildung, Hashing, Verschlüsselung und Datei-I/O auf einem separaten Writer-Thread ohne Engine-Lock. Höchstens ein Save ist ausstehend; weitere Requests ergeben `content_save_busy`. Ohne Content-Speicher ergibt sich `content_storage_disabled`; unbekannte, doppelte, leere oder RAM-Topic-Auswahlen ergeben `invalid_content_topics`.

Intervall-Saves wählen ausschließlich fällige, veränderte Intervall-Topics aus. Sie speichern keine aktuellen Nachrichten eines manuellen Topics mit. Nicht ausgewählte gespeicherte Topic-Abbilder bleiben unverändert. Termine verwenden monotone Laufzeit; Maintenance kann sie verzögern. Nach einem gewöhnlichen Schreibfehler bleibt der Zustand dirty und kann beim nächsten Intervall beziehungsweise manuell erneut gesichert werden. Es gibt keine unbeschränkte Save-/Retry-Queue.

Unabhängig gespeicherte Topics können unterschiedliche Zeitstände haben. Für einen zusammenhängenden Workflow die betroffenen Topics ausdrücklich gemeinsam sichern. Das erzeugt einen gemeinsamen technischen Erfassungszeitpunkt; eine atomare fachliche Transaktion zwischen Publish und Verarbeitung entsteht dadurch nicht.

## Status und Abschluss

`status.content_storage` enthält:

| Feld | Bedeutung |
| --- | --- |
| `enabled` / `encrypted` | Speicher eingerichtet / gemeinsamer Secret-Modus aktiv. |
| `loaded` | Beim Start wurde ein Manifest beziehungsweise älterer Checkpoint geladen. |
| `revision` | Aktueller Inhaltsänderungsstand persistenter Topics. |
| `saved_revision` | Revision des zuletzt erfolgreich gespeicherten Save-Auftrags. |
| `dirty` | Mindestens ein persistentes Topic hat noch nicht gespeicherte Änderungen. |
| `save_in_progress` | Save eingereiht oder in Bearbeitung. |
| `requires_restart` | Unbestätigter Manifest-Commit; weitere Saves bis zum Neustart abgewiesen. |
| `last_save_time_ms` / `last_error` | Letzter Abschlusszeitpunkt / aktueller Fehler oder null. |
| `topics` | Pro persistentem Topic Strategie, `revision`, `saved_revision`, `dirty`, `loaded`. |

Für einen Save der Revision N prüfen: `save_in_progress` false, `last_error` null und `saved_revision >= N`. Unter `topics` bleibt erkennbar, ob andere, nicht ausgewählte Topics weiterhin dirty sind. Ein einzelner Topic-Save bestätigt nicht den vollständigen aktuellen Owner-Inhalt. Neue Änderungen während eines Saves bleiben dirty. RAM-Änderungen lösen keine Save-Arbeit aus.

Reguläres Beenden wartet auf bereits angenommene Saves, startet aber keinen impliziten zusätzlichen Save. SIGKILL kann laufende und nach dem letzten Save erfolgte Änderungen verlieren. Betriebssystem-I/O kann das Beenden verzögern.

## Atomare Dateistruktur und Grenzen

`state.bin` ist das versionierte Manifest `MYNTOP01`: gemeinsame Owner-Metadaten plus eine begrenzte Topic-Tabelle mit Topic-Namen, Speicherstrategie, Revision, Dateigröße, Slot und SHA-256 des gespeicherten Abbilds. Die kleinen Einträge sind JSON-kodiert; Nachrichten bleiben binär. Die Owner-Metadaten verwenden den bisherigen binären Checkpoint-Rahmen ohne Nachrichten/Topics.

Jedes Topic erhält ein eigenes binäres `MYNCP001`-Abbild mit Nachrichten, Policies, Kopfstand. Es bettet das [gemeinsame Nachrichtenlayout](binary-message-layout.md) ein. Nur tatsächlich benötigte Producerbindungen stehen zusätzlich im Topic-Abbild; die gemeinsame Identitätstabelle liegt im Manifest. Relative Speicheradressen und Slots werden beim Zusammenstellen beziehungsweise Zusammenführen angepasst, ohne JSON-/Protobuf-Konvertierung der Nachrichten oder Eventwerte. Die RAM-Arenen bleiben gemeinsam, um kleine Topics nicht jeweils mit eigenen reservierten Seiten zu belasten.

Dateinamen werden aus SHA-256 des Topic-Namens und einem von zwei Slots erzeugt; Topic-Namen werden nicht als Pfade verwendet. Der Writer schreibt und synchronisiert zuerst die unreferenzierten Topic-Slots. Erst wenn alle ausgewählten Abbilder fertig sind, ersetzt er `state.bin` atomar und synchronisiert das Verzeichnis. Ein abgebrochener Auftrag vor diesem Commit lässt sämtliche vorherigen Manifestreferenzen gültig. Nach erfolgreichem Commit werden ersetzte Slots entfernt. Fehler vor dem Manifest-Commit bereinigen die erzeugten uncommitteten Slots. Nach vollständiger Startvalidierung werden verwaiste interne Topic-Slots entfernt.

Scheitert das Verzeichnis-Sync **nach** dem Manifest-Rename, kann das neue gültige Manifest bereits sichtbar sein, seine Haltbarkeit ist aber unbestätigt. Weitere Saves werden bis zum Neustart verweigert, damit kein möglicherweise referenzierter Slot überschrieben wird. Das wird als Speicherfehler gemeldet. Eine physische Stromausfallmessung wird damit nicht behauptet.

`max_snapshot_bytes` begrenzt die Summe aus aktuellem Manifest und allen referenzierten Topic-Dateien einschließlich Verschlüsselungsumschlägen; erlaubt sind 64 KiB bis 512 MiB, Default 128 MiB. Während eines Saves existieren alte und vorbereitete Dateien nebeneinander. RAM-Kopie, ein Topic-Abbild und Serialisierungs-/Verschlüsselungspuffer benötigen zusätzlichen Arbeitsspeicher. Capture und Datei-Parser sind begrenzt; neue Registry-Abhängigkeiten wurden nicht eingeführt.

Verschlüsselte Dateien verwenden AES-256-GCM mit authentifiziertem 64-Byte-Header und 16-Byte-Tag, frischer 96-Bit-Nonce pro Datei/Save. Der Passwortmodus verwendet einen zufälligen 128-Bit-Salt je Store-Schlüssel; neue Dateien und das Manifest teilen diesen Salt. Der Raw-Key hat 256 Bit; Passwortmodus verwendet PBKDF2-HMAC-SHA256 mit 600.000 Iterationen. Die teure Passwortableitung erfolgt einmal beim Öffnen des Stores vor dem Serving; der vorbereitete Schlüssel wird für Saves wiederverwendet. Alte Dateien mit unabhängigen Salts bleiben lesbar und benötigen beim Laden jeweils ihre ursprüngliche Ableitung, bis sie neu geschrieben werden. Format und 600.000 Runden bleiben unverändert; Offline-Rekey erzeugt einen neuen Salt. Es werden keine Klartext-Temporärdateien für verschlüsselte Abbilder geschrieben. Serialisierte Klartextpuffer, Secrets und abgeleitete Schlüssel werden bei normaler Freigabe genullt. Prüfsummen unverschlüsselter Dateien erkennen Beschädigung, authentifizieren aber keinen Angreifer mit Schreibrechten.

Die Umsetzung verwendet den bestehenden [AWS-LC-AEAD-Provider](https://docs.rs/aws-lc-rs/latest/aws_lc_rs/aead/) und orientiert den PBKDF2-Arbeitsfaktor an [OWASP](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html). Keine FIPS-Zertifizierungszusage.

## Vollständiger Start-Restore

Der gesamte Load/Decrypt/Validate/Indexaufbau läuft vor Listener-Bindung: Rechte abbauen → CFG und einmaliges Secret lesen → Manifest und **alle referenzierten Topic-Dateien** vollständig prüfen/entschlüsseln → aktuelle Security zuordnen → aktiven Bestand und Indizes aufbauen → Listener öffnen. Falsches Secret, fehlende/defekte referenzierte Datei, falscher Owner, unbekannte Identität oder aktive Quotenüberschreitung brechen den Start ab. Kein teilweiser Betriebsstart und kein automatischer Leerstand als Fehlerfallback.

Gespeicherte Storage-Strategien haben Vorrang vor der ursprünglichen RAM-Baseline in der CFG, damit ein erfolgreich gespeicherter Online-Wechsel einen Neustart übersteht. Ein CFG-Edit allein ist kein Storage→RAM-Wechsel. Der ausdrückliche Offline-Befehl entfernt den gespeicherten Topic-Inhalt und seine Dateien; die CFG-Baseline muss dafür RAM vorsehen. Eine fehlende Topic-Definition kann aus dem Abbild wiederhergestellt werden, ohne Rechte zu erfinden. Aktuelle Security bleibt für Clients, Sperren, Schlüssel und ACLs maßgeblich. [Neustart und Offline-Wartung](restart-and-storage.md).

Gesichert werden Nachrichten/Payloads/IDs/absolute Zeiten, Topic-Policies/-Kopfstände, Producer-Positionen, gemeinsame Nachrichten-Vergabestände. Sessions, Challenges, unvollständige Rotationen und Rate-Limiter werden nicht wiederhergestellt. Abgelaufene Nachrichten werden vor Client-Annahme entfernt; die Lebensdauer wird nicht verlängert. Der gespeicherte Owner-Zeit-Höchststand verhindert Wiederbelebung bei zurückspringender Uhr.

Topic-Cursor behalten gespeicherte Kopfstände. Ein als ungültig erkannter Poll-Cursor startet wieder am aktiven Anfang mit denselben Seiten-/Byte-Limits. Alte erneut passende achtstellige Werte bleiben mehrdeutig. Producer-Listen-Cursor bekommen eine neue Prozessgeneration.

**Kein Load/Restore/Reload oder Nachladen während des Betriebs.** Spätere Dateiveränderungen werden nicht eingelesen. Der Writer verwendet nur die beim Start geladene Manifesttabelle und die eingereihten RAM-Kopien; er liest keine Topic-Dateien nach.

## Vergabestand und Backups

The retired `references.bin` delivery allocator is no longer created, advanced or required. Existing MYNCP001 confirmation sections are validated and discarded during startup; new snapshots contain none. See [upgrade notes](message-client-confirmation.md).

Bei Backups das Manifest und seine referenzierten Topic-Dateien zusammen sichern, während kein Save läuft. Message-ID-RID-Pools setzen am gespeicherten Reservierungsstand an und prüfen aktive Kollisionen; sie sind keine über beliebige Backups global eindeutigen Ereigniskennungen.

## Wechsel vom bisherigen Owner-Checkpoint

Das bisherige globale `content_storage.save_interval_seconds` entfällt. Dieses Feld aus der CFG entfernen und gewünschte Strategien explizit an den Topics setzen; ohne Angabe bleibt das jeweilige Topic RAM. Ein alter einzelner `MYNCP001`-Checkpoint wird beim Start nach vollständiger Prüfung in Topic-Dateien und Manifest überführt, mit demselben globalen Secret-Modus. Alle gespeicherten Topics brauchen dafür eine ausdrückliche CFG-Definition und mindestens eines eine persistente Strategie. Andernfalls bricht der Start ab, ohne den alten Checkpoint durch leeren Inhalt zu ersetzen. Als RAM konfigurierte Topics werden nicht übernommen. Der neue Manifest-Commit erhält den alten gespeicherten Zustand bis zum erfolgreichen Wechsel.

## Verifikation

Unit- und Prozesstests prüfen alle drei Secret-Modi, verdecktes Terminal-Echo, SIGKILL-Restore, zustandsloses Polling, aktuelle ACLs, expiry/Inhaltslimits, Schreibfehler, unabhängige manuelle/Intervall-Saves, Gruppen-Rollback, RAM-Default/-Wechsel, gemischte Dateiverschlüsselung, beschädigte/fehlende Topic-Dateien, strikte Konfiguration und die Übernahme älterer Checkpoints. Native macOS-Laufzeitprüfung bleibt für GitHub CI vorgemerkt. Neue Performancewerte oder physische Stromausfallmessungen werden damit nicht behauptet.

Gezielte Prozessabbrüche, ENOSPC/EIO und die Offline-Befehle `inspect`/`cleanup`: [Speicher-Abbruchtests](storage-interruption-tests.md).

## Content working-memory admission

`limits.max_content_working_bytes` defaults to 268435456 bytes (256 MiB), accepts
nonnegative byte counts representable on the host, and controls conservative runtime save admission separately
from `max_active_message_bytes` and the disk limit `max_snapshot_bytes`. Zero
rejects saves. The estimate accounts for six times allocated record/blob/type
storage, pair/index and registry allowances, plus 1 MiB of fixed headroom. It
currently considers the whole engine even for a selected-topic save. A rejection
returns `content_memory_budget_exceeded` before capture and releases the save slot.
`content_storage.working_memory_reserved_bytes` in admin status reports the
admitted estimate while capture/writing is in progress, then returns to zero.

Only one content capture/write runs per engine. Broad saves pin initialized
record and payload blocks under the owner lock, together with producer counters,
topic heads/policies, expiry cutoff and ID reservations. The worker selects and
compacts this immutable epoch before writing. A later live mutation copies only
its affected shared block; an ordinary unique write performs no atomic reference-
count operation. When the last snapshot is gone, a subsequent write recovers the
original unique buffer without copying it.

For smaller selections, the existing topic indexes capture only selected records
under the lock, without pinning unrelated RAM payload pages. Saves with no selected records do
not pin empty retained arenas. Unselected persistent-topic metadata remains bound
to the same epoch; RAM-only content is never written. No wire or disk format changes.

A queued save is not a completed save: compaction/size validation, encryption and
I/O may fail on the worker. Such failures preserve the previous manifest, keep
changes dirty and release the save slot/working-memory reservation. Check status
for completion. A compatible single-topic image is borrowed by the writer rather
than copied again. Capture still has bounded-by-data metadata and copy-on-write
costs; it is not lock-free. This setting is an admission estimate, not a
hard allocator or process RSS cap. Startup/offline loading, transport buffers,
normal live-message growth and allocator retention require separate headroom.
Per-response transport limits remain independent. See the
[storage and restart guide](restart-and-storage.md).

## Security dependency of a content save

Each save captures the submitted security-journal sequence with its content image.
The background content writer waits outside the core lock until that sequence is
durable before writing the checkpoint. A security error, stopped writer or
five-second dependency timeout fails the save, preserves the previous committed
content and leaves the captured changes dirty. Inspect content status and retry
once security storage recovers. RAM security changes remain asynchronous and
immediately effective. See [security journal guide](security-journal.md).

Reducing a topic's TTL policy affects future publishes. Previously accepted
messages retain their original expiry across save and restart; checkpoint
validation applies the protocol-wide lifetime bound to those records.

SIGINT or SIGTERM during interactive secret entry cancels the prompt with a
controlled error and restores the original terminal settings before exit. The
same applies to offline storage prompts. Partial terminal input is discarded on
aborted entry; successful multi-line piped restore/rekey input is preserved. A
catchable prompt interruption does not start listeners. SIGKILL cannot run this
cleanup. See [restart and storage guide](restart-and-storage.md).

## Detecting a missing manifest

The content lock file records whether the store is fresh or has committed a
manifest. A missing manifest in an initialized store aborts startup and preserves
remaining topic files. A genuinely fresh store can still recover from partial
files left by an interrupted first save. Legacy stores with a valid manifest are
upgraded when opened. Offline restore retains the maximum saved owner-time cutoff
of the destination and backup, together with the current time, so a clock rollback
cannot revive a record already expired in the destination's saved history.
See [restart and storage guide](restart-and-storage.md).

## Aggregate size of the next generation

`max_snapshot_bytes` limits the encoded files of the next committed generation,
including its manifest. A multi-topic save counts unchanged files plus the new
versions of all selected topics; replaced old versions are excluded together,
so growing one selected topic and shrinking another is independent of selection
order. Staging still needs physical disk space for old and new slots until commit.

Live RAM-topic names and policies do not consume the encoded snapshot budget.
Projection limits bound the image itself; the bounded encoder and aggregate
writer count actual serialized controls and files. The separate working-memory
admission may conservatively include the entire live owner. Check save completion
and errors through status; an accepted queue request is not a commit receipt.
