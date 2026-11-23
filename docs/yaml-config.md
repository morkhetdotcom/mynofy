# YAML-Konfiguration

YAML ist das bevorzugte Format der vom Betreiber bearbeiteten Service-Konfiguration. `.yaml` und `.yml` werden unabhängig von Groß-/Kleinschreibung als YAML gelesen. `.json` und andere beziehungsweise fehlende Endungen behalten das bisherige strikte JSON-Verhalten. Es gibt keinen zweiten Parser-Versuch nach einem Formatfehler. YAML und JSON verwenden dieselben `Config`-Typen, Defaults und fachlichen Prüfungen.

```yaml
# Auszug; eine vollständige Baseline steht in config/scenarios/unix.yaml.
service:
  id: local
  unix_socket: run/mynofy.sock
  admin_socket: run/mynofy-admin.sock
security_storage:
  directory: security
  queue_capacity: 128
  compact_after_records: 4096
```

Relative Datei-, Socket- und Security-Pfade beziehen sich auf den Ordner der Konfigurationsdatei. `Config::parse` prüft Daten ohne Pfadauflösung und ohne Speicherzugriffe; `Config::load` liest zusätzlich die Datei und löst Pfade auf. Die neuen Setup-Skripte erzeugen `config.yaml` und setzen einen eigenen `security`-Ordner im jeweiligen Demo-Verzeichnis. Vorhandene `.local`-Dateien werden nicht umgeschrieben.

## Unterstütztes Profil

Der reine Rust-Parser `saphyr-parser` 0.3.4 aus dem [offiziellen Saphyr-Projekt](https://github.com/saphyr-rs/saphyr) übernimmt die YAML-Grammatik. Die kleine Anbindung bildet die erlaubten Events auf das bestehende JSON-Datenmodell ab. Es wird kein zweiter YAML-Parser und kein YAML-Objekt-/Tag-Framework eingebaut. Rust 1.88 bleibt die Projekt-Mindestversion; der ausgewählte Parser benötigt laut Paketmanifest Rust 1.85.

Unterstützt sind Kommentare, Block-/Flow-Mappings und -Listen, einfache/doppelte Anführungszeichen sowie literale/gefaltete mehrzeilige Strings. Mappings verwenden skalare Schlüsselnamen; doppelte Schlüssel werden auf jeder Ebene nach dem Dekodieren abgewiesen. Unbekannte Konfigurationsfelder und falsche Feldtypen werden wie bei JSON abgewiesen.

Anker, Aliase, Tags (auch `!!str`), Merge-Keys `<<`, komplexe Mapping-Schlüssel und mehrere Dokumente werden ausdrücklich abgewiesen. Es gibt keine Includes oder Umgebungsvariablen-Auswertung. Die Konfiguration hat maximal 32 MiB UTF-8-Eingabe, YAML zusätzlich höchstens 64 Verschachtelungsebenen und eine Million Parser-Events. Die YAML-Ausgabe ist ebenfalls auf 32 MiB begrenzt.

Ungequotete Werte verwenden ein bewusst kleines Typmodell:

- `true`/`false` einschließlich `True`/`False` und Großschreibung sind Boolesche Werte.
- `null`, `Null`, `NULL`, `~` und leere Werte sind `null`.
- Zahlen verwenden JSON-Zahlensyntax: beispielsweise `12`, `-3`, `1.5`. Die jeweiligen Config-Feldtypen entscheiden, was erlaubt ist; Limits verlangen weiterhin ganze nichtnegative Zahlen.
- Andere Skalare sind Strings. Datums-/Zeitangaben werden als Strings durch die bisherige RFC-3339-Prüfung validiert. `yes`/`no` werden nicht in Boolesche Werte umgewandelt. Hex-/Oktalschreibweisen und führende Nullen sind keine alternative Eingabe für numerische Limits.

Strings wie `"true"`, `"null"` oder `"0012"` bei Bedarf ausdrücklich quotieren. Der Export quotiert alle Stringwerte automatisch und verwendet zwei Leerzeichen Einrückung. Kommentare und ursprüngliche Formatierung werden beim Export/Konvertieren nicht übernommen.

## Start und bestehende Konfigurationen

```sh
cargo run --locked -- check-config --config config/scenarios/unix.yaml
cargo run --locked -- serve --config .local/unix/config.yaml
```

Die gewöhnlichen Unix-Rechteregeln gelten auch für diese Kommandos; privilegierte Linux-Aufrufe benötigen explizite Ziel-UID/GID. Alte JSON-Konfigurationen funktionieren weiter. Die JSON-Versionen der vier Szenariovorlagen bleiben als Kompatibilitätsvorlagen und Test-Fixtures erhalten; Tests prüfen ihre inhaltliche Gleichheit mit YAML.

Zum Wechsel einer vorhandenen Installation:

```sh
mynofy config-convert --config config.json --out config.yaml
```

Der Befehl prüft die Quelle, schreibt eine neue Datei mit privaten Rechten und überschreibt keine vorhandene Datei. Die Zielendung bestimmt YAML oder JSON. Pfade werden absolut ausgegeben, einschließlich des bisher effektiven Security-Verzeichnisses. Das vorhandene Journal wird weder geöffnet noch verändert.

Das ist insbesondere für die Security wichtig: Ohne expliziten Pfad leitet der Dienst das Verzeichnis aus dem vollständigen Dateinamen ab. Ein bloßes Umbenennen von `config.json` nach `config.yaml` würde deshalb von `config.json.security/` zu `config.yaml.security/` wechseln. `config-convert` erhält die bisherige Zuordnung durch einen expliziten Pfad. Bei manueller Umstellung `security_storage.directory` entsprechend setzen; vor dem Neustart das Ergebnis prüfen.

## Admin-Export und interne Formate

```sh
mynofy admin export --socket run/mynofy-admin.sock --out snapshot.yaml
```

Bei `.yaml`/`.yml` erzeugt der Admin-Export YAML, sonst JSON. Ohne `--out` bleibt die Ausgabe JSON für bestehende Werkzeuge. Auch hier wird nur eine neue Datei angelegt. Der bestehende Security-Pfad bleibt Teil des Exports.

Das Admin-JSONL-Protokoll, Client-JSONL, Adapterregeln, Kataloge und das interne Security-Journal behalten ihre bestehenden Formate. YAML wird ausschließlich an der Service-Konfigurationsgrenze gelesen/geschrieben; der Nachrichten-Hotpath erhält keine YAML-Arbeit.
