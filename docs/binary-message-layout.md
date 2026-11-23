# Gemeinsames binäres Nachrichtenlayout

Der Nachrichtenbestand verwendet ein initialisiertes, zeigerfreies Byte-Layout sowohl im RAM als auch in einem Nachrichtenabbild. Speichern und Einlesen wandeln Records, Payload-Blöcke und Inhaltstyp-Tabelle nicht in JSON, Protobuf oder native Rust-Objekte um. Dateirahmen und Prüfsummen werden zusätzlich erzeugt beziehungsweise geprüft. Alle eigenen Zugriffe bleiben ohne `unsafe`.

Der Umbau betrifft den laufenden Core, nicht nur einen Export-Encoder. Der direkte Protobuf-Poll-Encoder liest dieselben Byte-Records und Payload-Blöcke. JSON und die typisierte Protobuf-API bilden daraus bei Bedarf Antworten. Die bereits vorhandenen Such-/Sortier-/Ablaufindizes bleiben separate Laufzeitstrukturen.

## Record v1

Ein Record belegt exakt **80 Byte**, einschließlich einer 32-Byte-Payload-Union. Ein belegter Arena-Slot hat dieselben 80 Byte, ohne `Option<Record>` oder native Enum-/Paddingrepräsentation. Mehrbytezahlen sind Little Endian; Message-IDs enthalten acht ASCII-Bytes aus dem bestehenden Alphabet.

| Offset | Breite | Feld |
| --- | --- | --- |
| 0 | 8 | Message-ID |
| 8 | 8 | Absoluter Ablaufzeitpunkt, Unix-ms |
| 16 | 8 | Producer-Annahmeposition |
| 24 | 8 | Topic-Annahmeposition |
| 32 | 8 | Vollständiger Erstellungszeitpunkt, Unix-ms |
| 40 | 2 | Producer-Registry-Slot |
| 42 | 2 | Topic-Registry-Slot |
| 44 | 4 | Inhaltstyp-ID, 0 = keiner |
| 48 | 32 | Payload-Union |

Der vollständige Erstellungszeitpunkt ersetzt den alten U32-Zeitanteil samt Sondertabelle für lange TTLs. Deren Diagnosefeld `extended_time_entries` bleibt vorerst mit 0 erhalten. Es gibt keine zeitabhängige Uminterpretation gespeicherter Records.

Die Union enthält ihr Typkennzeichen an relativem Offset 6, reserviertes Byte 7 ist 0:

- **Bytes, Kennzeichen 1:** Länge u32 an 0; reservierte Bytes 4–5 sind 0; relative Blockadresse u64 an 8; Bytes 16–31 sind 0. Länge maximal 4096. Leere Payloads haben Adresse 0 und brauchen keinen Block.
- **Event, Kennzeichen 2:** Kategorie U16/Aktion U8/Status U8 als gemeinsamer u32-Schlüssel an 0; drei 3-Bit-Parametertypen als u16 an 4; 24 Wertebytes ab 8. Nicht genutzte Bits/Bytes sind 0. Der eingebaute Katalog 1/v1 bleibt implizit. Typfolge, Bool-Werte und endliche Floats werden beim Einlesen geprüft; Floatbits einschließlich negativem Nullwert bleiben erhalten.

Records liegen in Byte-Chunks zu 256 Slots: live-Zähler u32 an 0, vier reservierte Nullbytes, 32 Byte Belegungsbitmap und anschließend 256 × 80 Recordbytes. Die Bitmap ist das Belegungsmerkmal; freie Records sind vollständig 0. Die gesamte persistierte Chunkgröße ist 20.520 Byte. Freilisten und Chunk-Suchpositionen gehören nicht zu diesem Block.

## Variable Daten und Inhaltstypen

Byte-Payloads und internierte Inhaltstypnamen liegen in separaten Seiten mit je **16 KiB Datenbereich**. Größenklassen von 16 bis 4096 Byte erlauben Wiederverwendung freier Plätze ohne Verschieben lebender Payloads. Eine Adresse enthält `(Seiten-ID + 1)` in den oberen 32 Bits und den Offset innerhalb des Datenbereichs in den unteren 32 Bits. Sie enthält keinen Prozesszeiger.

Eine Seite beginnt mit Klassen-Log2 (1 Byte), drei reservierten Nullbytes, live-Zähler u32 und einer zur Klasse passenden Bitmap aus u64-Worten. Der Rest ist der Datenbereich. Die Seitengröße umfasst daher 16 KiB plus 16–136 Headerbytes. Nur die benötigte Bitmap wird gespeichert. Unbenutzte Slots und der Rest eines belegten Klassenplatzes nach der tatsächlichen Datenlänge sind 0.

Die Inhaltstyp-Tabelle besteht aus 16-Byte-Einträgen: Blockadresse u64, UTF-8-Länge u32 und Referenzzahl u32. Freie Einträge sind 0. Ihre Namenssuche ist ein flüchtiger Hash-Index; beim Lesen wird er aus den geprüften Bytes rekonstruiert. Die unveränderte Inhaltstyp-ID verbindet Records mit dieser Tabelle.

Beim Entfernen einer Nachricht werden Record und freigegebene Blockplätze vollständig genullt. Alte Inhalte in bereits freigegebenen Plätzen werden dadurch nicht versehentlich mitgesichert. Leere Seiten und Chunks werden budgetiert während der vorhandenen Maintenance freigegeben; freie Plätze bleiben vorher wiederverwendbar. Neue Diagnosefelder `payload_allocated_bytes`, `payload_pages` und `content_type_table_bytes` machen Seitenkapazität und Tabellenkosten sichtbar. Die logische Annahmequote ist weiterhin keine exakte RSS-Grenze.

Die bisherige individuelle `Bytes`-Allokation ist kein Bestandteil mehr des dauerhaften Records. Der direkte Wire-Encoder schreibt aus geliehenen Block-Slices. Die typisierte API erzeugt dagegen unabhängige `Bytes`-Antworten durch Kopieren; dadurch können Antworten den Core-Lock sicher verlassen, während die Seiten weiter verändert werden. Diese Kosten werden im Vergleich getrennt ausgewiesen.

## Abbild-API und Dateirahmen

```rust,no_run
use mynofy_server::message_image::{ImageLimits, MessageImage};
# fn example(engine: &mynofy_server::engine::Engine) -> Result<(), mynofy_server::BoxError> {
// Unter der Synchronisierung des einbettenden Owners eine konsistente Kopie erfassen:
let image = engine.capture_message_image(ImageLimits::default())?;
// Danach außerhalb des Core-Locks schreiben; der Aufrufer bestimmt Dateirechte/Atomarität.
let mut options = std::fs::OpenOptions::new();
options.write(true).create_new(true);
#[cfg(unix)]
{
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}
let mut file = options.open("messages.bin")?;
image.write_to(&mut file)?;
file.sync_all()?;
let restored = MessageImage::read_from(std::fs::File::open("messages.bin")?, ImageLimits::default())?;
assert_eq!(image.message_count(), restored.message_count());
# Ok(()) }
```

Für Produktbetrieb neue private Dateien verwenden und bei Ersetzung einen synchronisierten temporären Stand atomar umbenennen; der generische Writer erledigt keine Dateiverwaltung. Die API ist eine Bibliotheksgrundlage, kein neuer Admin-Command. `capture_message_image` kopiert unter der Owner-Synchronisierung Byte-Blöcke und Registry-Bindungen. Es ist keine konstante Arbeit und benötigt zusätzlichen RAM. Das zurückgegebene Abbild bleibt von nachfolgenden Publishes/Abläufen unabhängig; Datenträger-I/O erfolgt mit dem eigenen Abbild außerhalb des Locks.

Der 96-Byte-Dateiheader enthält `MYNIMG01`, Formatversion 1, Recordbreite, Slot-/Tabellengrößen, Registry-Anzahlen und Recordanzahl. Die ersten 64 Byte enthalten Felder und reservierte Nullen, die letzten 32 ihren SHA-256. Danach folgen Owner-ID, Client- und Topic-Bindungen, Inhaltstyp-Tabelle, Record-Chunks und Blockseiten in festgelegter Reihenfolge. Jeder Block hat einen 48-Byte-Rahmen mit Ordnungsnummer u64, Länge u32, vier reservierten Nullbytes und SHA-256 über die Nutzbytes.

Die Registry-Bindungen enthalten stabile Client-IDs und Topic-Namen. Sie identifizieren die numerischen Referenzen des Records und enthalten keine ACLs oder Schlüssel. Stringbytes werden direkt als UTF-8-Blöcke geschrieben; Record-/Union-/Tabellen-/Seitenbytes entsprechen exakt dem RAM-Bestand. Eine spätere Formatversion darf das native Rust-Layout nicht stillschweigend zum Dateiformat machen.

`read_from` liest die Blöcke direkt in dieselben RAM-Container, prüft vor Freigabe und rekonstruiert deren Allokatorlisten und Namensindex. Das ist kein ungeprüftes Speicher-Mapping. Defaultgrenzen: 64 MiB gesamte Datei, 100.000 residente Records, 4096 Chunk-Slots und 65.536 Seiten-Slots; der Aufrufer kann engere oder andere passende Grenzen wählen. Header/Blocklängen werden vor Allokationen begrenzt.

Validiert werden Version/Breiten, Prüfsummen und Blockreihenfolge, vollständiges EOF ohne Nachlauf, Bitmap/Zähler/Reserviertbytes, IDs und Zeitwerte, Registry-Grenzen, doppelte IDs/Positionen, Union-Typen, Parameter, sämtliche Adressen und Inhaltstyp-Referenzzahlen. Gemeinsam verwendete Inhaltstypen bleiben referenzgezählt; unerlaubt doppelt verwendete Payload-Allokationen und nicht referenzierte belegte Blöcke werden abgewiesen. Es gibt keinen halb übernommenen Datenbestand bei einem Lesefehler.

## Umfang und Einbindung

`MessageImage` ist ein **Nachrichten-Speicherabbild**, noch kein vollständiger Owner-Checkpoint. Es enthält die residenten Nachrichten samt Ablaufzeiten, Payloads und Registry-Bindungen. Residente, logisch bereits abgelaufene Records können enthalten sein; ein künftiger Owner-Restore muss sie vor Ausgabe weiterhin nach ihrer unveränderten absoluten Zeit verwerfen. `message()` dient der expliziten Offline-Inspektion, führt keine Client-Authentifizierung oder Topic-Autorisierung aus und ist kein Server-Lesepfad.

Der vollständige [Content-Checkpoint](content-storage.md) bettet dieses Nachrichtenabbild inzwischen zusammen mit ACK/NACK, Policies, Kopfständen und Vergabemetadaten ein. `MessageImage` bleibt die schmalere Bibliotheks-Schnittstelle; der Service lädt ausschließlich den vollständigen Checkpoint und nur während des Starts. Sicherheitszustand und Content bleiben getrennt. Ein als ungültig erkannter Topic-Cursor beginnt inzwischen automatisch am aktiven Anfang; die Mehrdeutigkeit eines erneut passenden achtstelligen Werts bleibt bestehen.
