# Funktionstests und reproduzierbare Mutationstests

Der lokale Owner-Core wird mit einem unabhängigen Referenzmodell, Vergleichen zwischen Protokollpfaden und begrenzten Mutationstests geprüft. Die Tests sind Teil von `cargo test --locked --workspace` und benötigen keine zusätzlichen Registry-Abhängigkeiten oder externen Fuzzing-Werkzeuge. Testgeneratoren und feste Testidentitäten sind ausschließlich unter `cfg(test)` eingebunden.

## Geprüfte Abläufe

Der Zustandsmodelltest mischt Publish, Poll, ACK/NACK, Get/List, Ablauf, kleine Cleanup-Budgets, Uhr-Rücksprünge, Rechteentzug und Reconnects für zwei Clients und zwei Topics. Zeit wird kontrolliert, ohne Sleeps oder Laufzeitgrenzen. Verschiedene Seeds verwenden unterschiedliche Nachrichten-/Paarquoten.

Ein einfaches Modell aus Nachrichten, Berechtigungen und Bestätigungspaaren liefert unabhängig vom Core die erwarteten Inhalte und Endzustände. Nach jedem Schritt prüft der Test unter anderem:

- aktive Nachrichten bleiben bis zur absoluten Ablaufzeit erhalten; eigene Inhalte bleiben nach Topic-Rechteverlust einsehbar;
- Poll-Auswahl und producerseitiges Paging liefern die erwartete Reihenfolge und respektieren aktuelle Rechte;
- wiederholte Polls behalten die Paarreferenz; fremde/abgelaufene Referenzen werden abgewiesen; Endbestätigungen sind idempotent oder ergeben einen Konflikt;
- Statuszähler und Sichtbarkeit von `inspect_own` entsprechen dem Modell;
- Rechteänderungen invalidieren bestehende Sitzungen; deren Commands erzeugen keine Nachrichten;
- Nachrichten-/Paarquoten, aktive Bytebelegung, Topic-/Producer-/ID-Indizes und Paarzuordnungen stimmen mit dem residenten beziehungsweise aktiven Bestand überein;
- begrenzte physische Bereinigung darf abgelaufene Records vorübergehend behalten, aber keine aktiven Daten verlieren oder abgelaufene Daten wieder sichtbar machen.

Ein zweiter Test führt dieselben Requests über JSON, typisiertes Protobuf und die direkte Protobuf-Ausgabe aus. Er vergleicht JSON-Ergebnisse und die bytegenaue direkte Ausgabe mit dem generierten Encoder. Bytes bis einschließlich 4096 und ungültige 4097 Byte, numerische Events, Parametergrenzen, Unicode-Request-IDs, Polling, ACK/NACK und Ablauf sind enthalten. Zufällige interne Owner-Generationen und Vergabestartwerte werden für diesen Vergleich ausdrücklich angeglichen.

## Datenstrukturen und Parser

Die zusätzlichen Referenz-/Mutationstests prüfen:

| Bereich | Prüfung |
| --- | --- |
| Geordnete Indizes | Vergleich mit `BTreeMap` bei Insert/Replace/Remove/Get/Mutation, offenen/geschlossenen Bereichsgrenzen, U64-Rändern, Reclaim und Wechsel zwischen kleinen Vektoren und Bitmap-Blöcken. |
| Ablaufindex | Vergleich mit einfacher Referenztabelle bei gemischter Einfügefolge, gleichen/exakten Fristen, Deadline 0/U64::MAX, Pop und Reclaim. |
| JSON-/Protobuf-Frames | Mutierte Bytes auf nicht authentifizierten Sessions dürfen keine Inhalte oder Bestätigungspaare erzeugen; ausgegebene binäre Fehler bleiben dekodierbar. |
| YAML/JSON-Konfiguration | Mutierte gültige/ungültige Vorlagen werden abgewiesen oder als akzeptierte Werte/Configs erneut ausgegeben, eingelesen und verglichen. |
| Binäre Nachrichtenabbilder | Mutation, Abschneiden, Einfügen, Duplizieren und zufällige Bytes; akzeptierte Abbilder werden vollständig materialisiert und bytegenau erneut geschrieben. Zusätzlich Short-Read-Reader mit 1–31 Byte pro Read. |
| Vollständige Checkpoints | Mutation der Kontrolltabellen und eingebetteten Abbilder; akzeptierte Checkpoints müssen erneut bytegleich geschrieben werden können. |
| Topic-Manifest | Größen, Topic-/Clientbindungen, Einträge und Referenzen prüfen; Parseraufrufe verändern weder Manifest noch referenzierte Topic-Dateien. |
| Verschlüsselte Umschläge | Roundtrip unterschiedlicher Klartextlängen, falscher Schlüssel, Größenlimit, Modusverwechslung und mutierte Header/Ciphertexte/Tags; eine akzeptierte Entschlüsselung darf keine veränderten authentifizierten Bytes enthalten. |

Bei einem Teil der Nachrichtenabbild-/Checkpoint-/Manifest-Mutationen werden die jeweiligen Framing-Prüfsummen neu berechnet. Dadurch werden Strukturprüfungen hinter dem Prüfsummenvergleich erreicht. Gezielt veränderte gültige Byte-Payloads stellen sicher, dass der Image-Parser auch tatsächlich neue gültige Inhalte akzeptiert. Die kryptografischen Tags werden nicht nachträglich repariert.

Die Zufallsentscheidungen und Operationsfolgen verwenden einen festen, einstellbaren Seed. Kryptografische Nonces/Challenges kommen weiterhin aus dem vorhandenen Provider. Das sind begrenzte, seedgesteuerte Modell- und Mutationstests mit expliziten Orakeln; sie liefern weder vollständige Pfadabdeckung noch einen Beweis für beliebige Eingaben. Automatisches Shrinking von Fehlerfällen ist nicht implementiert.

## Ausführen und Fehler wiederholen

Schneller Standardlauf mit 16 Modell-Seeds × 600 Schritten und überwiegend 2048 Fällen pro zusätzlichem Test:

```sh
cargo test --locked -p mynofy-server functional -- --nocapture
```

Erweiterter Lauf mit 64 Modell-Seeds × 1000 Schritten und 16.384 Fällen pro zusätzlicher Kampagne:

```sh
./scripts/test-core-functional.sh
```

Der JSON/Protobuf-Vergleich ist auf 1500 Requests pro Lauf begrenzt. Der Short-Read-Test verwendet fünf feste Readergrößen. Die übrigen acht Kampagnen verwenden jeweils `MYNOFY_FUZZ_CASES`.

Anderer Start-Seed, einschließlich U64-Seed-Umlauf:

```sh
MYNOFY_TEST_SEED=0xffffffffffffffe0 ./scripts/test-core-functional.sh
```

Eigene Laufgröße:

```sh
MYNOFY_MODEL_SEEDS=32 MYNOFY_MODEL_STEPS=1000 MYNOFY_FUZZ_CASES=8192 \
  ./scripts/test-core-functional.sh
```

Der Modelltest meldet bei Fehler den konkreten Seed und die letzten 24 Operationen. Einzelnen Fall wiederholen:

```sh
MYNOFY_TEST_SEED=0x6d796e6f66790001 MYNOFY_MODEL_SEEDS=1 MYNOFY_MODEL_STEPS=1000 \
  cargo test --locked -p mynofy-server seeded_reference_model -- --nocapture
```

Für Parserfehler zusätzlich die ursprüngliche `MYNOFY_FUZZ_CASES`-Einstellung übernehmen und den betreffenden Testnamen filtern. Seed, Case-Index und derselbe Quellstand gehören zur Fehlerbeschreibung.

## Abgrenzung und nächste Schritte

Diese Erweiterung testet bestehendes Verhalten. Neue Neustarterkennung, Offline-Rekey/Backup-Werkzeuge, Speicherumbauten und Performancearbeiten sind nicht Teil dieses Schritts. Bestehende Prozess-/Transporttests bleiben in der vollständigen Suite erhalten; neue Transport-Dauertests wurden nicht vorgezogen. Native macOS-Prüfung bleibt für GitHub CI vorgesehen.

Die Nutzerreihenfolge lautet: zuerst Funktionstests einschließlich Modell-/Mutationstests abschließen, danach Neustart und Speicher behandeln, anschließend Performance messen und gezielt optimieren.

## Validierter Stand


Der verlinkte Prüfbericht hält den Stand vor der anschließenden [Neustart- und Speicherwartung](restart-and-storage.md) fest; sein Quell-Fingerprint ist historisch und wird nicht als Fingerprint späterer Änderungen ausgegeben.
