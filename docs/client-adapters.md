# Clients als Adapter für vorhandene Software

## Produktmodell

Ein mynofy-Client kann eine native SDK-Anwendung, ein AI-Agent, ein Wrapper um stdin/stdout oder ein regelbasierter Launcher für ein vorhandenes Binary sein. Die angeschlossene Software muss mynofy nicht kennen. Das verbindet Werkzeuge, die über Kommandozeilenargumente, stdin, stdout, stderr und Exitcodes steuerbar sind. Programme, die ein interaktives Terminal benötigen, brauchen gegebenenfalls einen gesonderten PTY-Adapter; ein gewöhnlicher Pipe-Wrapper kann nicht jedes Terminalprogramm unverändert bedienen.

Triggerauswertung, Interpretation und Prozesssteuerung liegen beim Client. Der Server bleibt Transport und begrenzter Nachrichtenspeicher. Es werden keine Skripte oder Programme im Message-Core ausgeführt.

| Clientform | Eingang | Ausführung | Ausgang |
| --- | --- | --- | --- |
| Native Anwendung / Agent | Gepollte Nachrichten | Eigene Anwendungslogik | Publish von Nachrichten/Status |
| Dauerhafter Prozess mit Pipes | Passende Nachrichten werden nach vereinbartem Framing auf stdin geschrieben | Ein laufendes Programm verarbeitet die Eingabe | stdout wird gerahmt gelesen und nach Regeln publiziert; stderr getrennt behandelt |
| Binary pro Trigger | Passende Nachricht liefert validierte Eingabewerte | Client startet ein lokal konfiguriertes Programm mit festgelegten Argumenten und optional stdin | Exitcode und begrenzte Ausgabe werden in das vereinbarte Ergebnisprofil übersetzt |

Framing ist ein Teil des Adapters, etwa JSONL oder längengekennzeichnete Bytes. stdout allein liefert keine verlässlichen Nachrichtengrenzen. Freie Textausgabe kann auch Logs enthalten; deren Interpretation muss konfiguriert sein. Ohne bewusstes Mapping bedeutet Exitcode 0 lediglich erfolgreicher Prozessausgang, nicht automatisch erfolgreiche fachliche Prüfung oder Jobabschluss.

## Regelmäßige Interpretationsbekanntgabe

Zusätzlich zu Kommunikation und Status gibt es ein separates Topic, beispielsweise `workflow/4711/interpretations`. Beim Beginn der Teilnahme, nach Regeländerungen und regelmäßig während seiner Teilnahme veröffentlicht der Client seine Interpretation erneut. Ein technischer Topic-Join ist dafür nicht erforderlich und im Pull-Protokoll nicht vorhanden.

Die Bekanntgabe beschreibt mindestens diese fachlichen Angaben; die konkreten Wire-Feldnamen sind noch Entwurf:

- Bekanntgebender Teilnehmer und aktuelle Prozess-/Teilnahmeinstanz, damit Neustarts unterscheidbar sind.
- Profil/Katalog samt Version und eindeutige Revision der Interpretationsregeln.
- Exakte Eingabetopics und Trigger: Kategorie/Aktion/Status, benötigter Lauf/Versuch/Schritt und weitere Voraussetzungen.
- Wirkung: welche fachliche Aktion der Client ausführt, welche Status-/Ergebnismeldungen folgen und welche Eingaben er voraussetzt.
- Gültigkeit der Bekanntgabe und Erneuerungsrhythmus.

Die Bekanntgabe enthält keine Secrets. Lokale ausführbare Pfade und komplette Prozessumgebungen müssen nicht veröffentlicht werden; für die Teilnehmer ist die angekündigte Wirkung entscheidend. Eine erneuerte Bekanntgabe derselben Revision verändert die Interpretation nicht. Ändern sich Regeln oder Profil, ist eine neue Revision nötig; die Kommunikation klärt die erforderliche Zustimmung und deren Aktivierung.

Bekanntgabe bedeutet „so arbeite ich“, keine automatische Genehmigung oder Registrierung von Ausführungsrechten. Statisch konfigurierte Clients können ein unveränderliches Profil wiederholt bekanntgeben; dynamische Agenten können ein neues Profil im Kommunikationstopic vereinbaren und dann ihre aktivierte Interpretation bekanntgeben. Der Vertrag muss verhindern, dass ein Teilnehmer allein durch eine neue Bekanntgabe fremde Rollen oder Pflichten ändert.

## Ablauf und Speicherbegrenzung

Bekanntgaben sind normale zeitlich befristete Nachrichten. Die Anwendung wählt eine endliche Gültigkeit und einen kürzeren Erneuerungsabstand im Rahmen der Topic-Policy. Ein konkreter Standard für beide Zeiten ist noch festzulegen; es wird keine neue Broker-TTL-Semantik eingeführt.

Der Beobachter hält pro Teilnehmer/Instanz die neueste gültige Bekanntgabe und prüft deren Revision. Alte Bekanntgaben bleiben beim Broker bis zum Ablauf aktiv; sie werden nicht durch ein neues Publish ersetzt. Kurze endliche TTL und angemessener Erneuerungstakt begrenzen den Bestand. Für eine neue Teilnahmeinstanz legt das Profil fest, wann deren Bekanntgabe die alte ersetzt; verschiedene Instanzen dürfen nicht versehentlich dieselbe Exklusivrolle behaupten.

Bei ausbleibender Erneuerung gilt die Interpretation als nicht mehr aktuell verfügbar. Daraus folgt weder, dass das Binary gestoppt ist, noch dass ein laufender Job beendet wurde. Ein Monitor erkennt das selbst; mynofy erzeugt beim Ablauf keine Offline-Meldung. Ausgelaufene Bekanntgaben dürfen keine neuen Ausführungsfreigaben begründen. Der Umgang mit bereits laufender Arbeit muss im Profil stehen.

Ein gemeinsames Topic bietet derzeit keine vom Consumer geprüfte individuelle Producer-Identität pro Message. Die Anwendung muss die tatsächliche Herkunft für verbindliche Rollen-/Regelzuordnungen nachweisen können; ein selbst eingetragenes Teilnehmerfeld genügt dafür nicht. Ebenso bleibt eine Bekanntgabe, die einen externen Koordinator nennt, dessen Aussage und keine atomare Jobvergabe durch mynofy.

## Regeln eines Binary-Adapters

Vor einer Implementierung werden mindestens folgende Punkte festgelegt:

1. Lokale Zuordnung von Trigger zu Programm und Argumenten, Eingabevalidierung und Mapping. Nachrichten wählen nicht beliebige Shellbefehle aus. Eine einfache Umsetzung startet ein festes Programm direkt mit getrennten Argumenten.
2. Framing von stdin/stdout, separates stderr, Obergrenzen für Eingabe und erfasste Ausgabe. Große Ausgaben extern ablegen und nur eine Referenz melden. Die freie Payload bleibt auf 4096 Bytes begrenzt.
3. Parallelitätslimit und begrenzter Rückstand: keine unbegrenzte Zahl von Prozessen oder wartenden Tasks.
4. Laufzeitlimit, Abbruch, Verhalten beim Ende eines dauerhaften Prozesses und Behandlung seiner Kindprozesse.
5. Verhalten bei erneutem Poll, doppeltem Trigger, Clientneustart und verlorenem Ergebnis. Ein Prozessaufruf und das Publish seines Ergebnisses sind nicht atomar; automatisch wiederholte Trigger können doppelte Wirkungen haben.
6. Feste Statuszuordnung: START/SUCCESS erst nach dem im Profil definierten Start, EXECUTE/SUCCESS nach dem definierten Schritt, COMPLETE/SUCCESS nur für den tatsächlich abgeschlossenen Gesamtauftrag. Technisches ACK bleibt separat.
7. Erforderliche Topic-Rechte sowie Ablauf und Erneuerung der eigenen Interpretationsbekanntgabe.

Der Launcher ist selbst der authentifizierte Producer. Das aufgerufene Programm benötigt nicht automatisch einen eigenen Schlüssel. Eine Behauptung über einen Benutzer oder ein Gerät hinter dem Adapter wird dadurch nicht separat authentifiziert.

## Beispiel

Ein Client ist lokal für ein Test-Binary konfiguriert. Er veröffentlicht regelmäßig im Interpretationstopic: „Ich beobachte EXECUTE/PENDING für den Testschritt des vereinbarten Laufprofils. Bei passenden Voraussetzungen rufe ich den fest konfigurierten Testrunner auf und melde den Ausgang im Statustopic.“

Eine AI veröffentlicht im Kommunikationstopic den Testauftrag beziehungsweise stimmt den Schritt dort ab. Der passende Trigger wird auf dem im Profil festgelegten Topic publiziert. Der Client prüft Kontext und Zuständigkeit, startet das Binary, verarbeitet die Ausgabe und meldet das Ergebnis. Ein Mensch kann die Interpretation lesen, im Kommunikationstopic Änderungen vorschlagen und anhand des Statustopics verfolgen, was tatsächlich passiert.

Die Wiederholung der Interpretation ist keine Wiederholung des Testauftrags. Der Client reagiert nicht auf sein eigenes Bekanntgeben mit einem erneuten Binary-Aufruf.

## Umsetzungsstand

Der CLI-Client verwendet Protobuf auf dem Wire und JSONL an stdin/stdout. Der unten beschriebene Adapter implementiert Once-/JSONL-Prozessausführung und regelmäßige Interpretationspublikation. PTY-Unterstützung, dynamische Regelaushandlung und persistente Ausführungscheckpoints bleiben offen.

## Implementierter Adapter und lokaler Test

`mynofy adapter` implementiert einen einzelnen lokal konfigurierten Trigger und genau einen gleichzeitig aktiven Prozessschritt. Keine zusätzliche wartende Clientqueue: während der Verarbeitung wird kein weiterer Trigger gepollt. Die Erneuerung der Interpretation läuft währenddessen weiter. Einträge verbleiben beim Owner nur bis zu ihrer TTL.

```sh
./scripts/setup-adapter.sh
cargo run --locked -- serve --config .local/adapter/config.yaml
```

In einem zweiten Terminal:

```sh
cargo run --locked -- adapter --socket .local/adapter/run/mynofy.sock --key .local/adapter/client.pk8 --owner local --rules .local/adapter/rules.json
```

In einem dritten Terminal den CLI-Client öffnen:

```sh
cargo run --locked -- client --socket .local/adapter/run/mynofy.sock --key .local/adapter/client.pk8 --owner local
```

Darin einen exakten Trigger senden:

```json
{"version":1,"type":"publish","id":"trigger","topic":"workflow/4711/communication","payload":{"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":40,"status":7,"parameters":[{"u64":4711}]}}
{"version":1,"type":"poll","id":"status","topic":"workflow/4711/status"}
{"version":1,"type":"poll","id":"rules","topic":"workflow/4711/interpretations"}
```

Die Statusabfrage gegebenenfalls erneut senden, nachdem das Programm gelaufen ist. Das Setup provisioniert alle drei Topics und verwendet für den lokalen Test denselben Client-Schlüssel für alle Rollen. Für echte getrennte Rollen separate Identitäten/ACLs konfigurieren. Die Beispiele verwenden `/bin/cat` unter Linux/macOS; andere Plattformen brauchen ein passendes Programm und WSS statt Unix-Socket. Das Setup überschreibt kein vorhandenes Demo-Verzeichnis.

Die [Once-Konfiguration](../config/adapters/echo.json) startet `/bin/cat` pro Trigger. Die [JSONL-Konfiguration](../config/adapters/echo-stdio.json) verwendet `mode: "stdio_jsonl"`: ein Prozess bleibt bestehen, erhält pro Trigger genau eine JSON-Zeile des Events und liefert genau eine gültige JSON-Zeile zurück. Zum Testen die passende Regeldatei mit `--rules` angeben; die Topics sind identisch. Das Programm muss dieses Eins-zu-eins-Framing einhalten. PTY/Promptsteuerung und beliebige Logausgaben auf stdout sind damit nicht unterstützt.

Triggervergleich ist exakt einschließlich Parametern und Katalogversion. Die Regeldatei enthält Trigger, Start-/Erfolgs-/Fehlerereignis, Profil/Revision, drei Topics, lokales Programm/Argumente, Pollintervall, maximale Laufzeit, Ausgabelimit sowie Erneuerung und Announcement-TTL. Ein neuer fachlicher Lauf benötigt eine passende lokale Regelkonfiguration. Programm und Argumente werden nicht aus einer Nachricht übernommen; die Eingabe ist die gerahmte JSON-Eventdarstellung auf stdin.

Nach erfolgreichem Prozessstart wird das konfigurierte `started`-Ereignis gemeldet. Für einen Once-Prozess gilt Exitcode 0 nach vollständiger begrenzter Ausgabe als Erfolg; im dauerhaften JSONL-Modus eine gültige vollständige Antwortzeile. Das ist das lokale Ausführungsprofil, keine allgemeine fachliche Erfolgsgarantie für jedes Programm. stdout wird als begrenzte Byte-Payload vor dem separaten Erfolgsereignis publiziert; beides ist nicht atomar. stderr wird verworfen. Spawnfehler, nicht erfolgreicher Exit, ungültige JSONL-Ausgabe, Timeout und Ausgabeüberschreitung führen zum konfigurierten Fehlerereignis. Ein fehlgeschlagener dauerhafter Prozess wird nicht automatisch für denselben Auftrag wiederholt; der nächste explizite Trigger kann einen neuen Prozess starten.

Die Interpretation wird vor dem ersten Poll, danach regelmäßig mit derselben Revision und neuer endlicher TTL publiziert. Sie enthält eine neue zufällige Teilnahmeinstanz pro Adapterstart, den gemeldeten Teilnehmer, Wirkung, Trigger, Statusmapping, Profil und Revision; keine lokalen Programmpfade/Argumente oder Secrets. Standardbeispiel: Erneuerung nach 10 Sekunden, Gültigkeit 30 Sekunden. Es gibt weiterhin keine Ersetzung alter Messages oder Broker-Registry.

ACK und Cursorfortschritt werden nach Lesen und vor Prozessausführung vorgenommen; sie bedeuten keine Fertigstellung. Der Fortschritt ist RAM-Zustand. Neustart kann noch aktive Trigger erneut lesen und damit ein Programm erneut ausführen. Es gibt keine Exactly-once-Wirkung oder automatische Protokollaushandlung. SIGINT/SIGTERM beendet den Adapter. Abbruch/Timeout beendet den direkten Kindprozess; beliebige separat gestartete Nachkommen werden nicht als Prozessgruppe verwaltet. Für Werkzeuge mit eigenen Prozessbäumen bleibt entsprechende Host-/Supervisor-Verwaltung nötig.

On Unix, the adapter starts configured programs in their own process group.
Timeout and adapter shutdown terminate that group while the child is still
tracked. Programs that detach into another group require a separate
supervisor if their descendants must be bounded. Workflow authorization and
deduplication remain deployment decisions.
