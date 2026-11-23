# Demo 2: Protokoll selbst aushandeln

Dieser gemeinsame Abschnitt gehört zum jeweiligen vollständigen Rollenstart.
Die generierten Dateien operator-START.md, worker-1-START.md und worker-2-START.md
enthalten jeweils diesen Abschnitt und genau eine Rolle. Jede Sitzung erhält
**ihre komplette Datei auf einmal**, nicht zunächst nur diesen gemeinsamen Text.

```text
Du nimmst an einem kurzen lokalen mynofy-Beispiel teil, Zielgröße 5–10 Minuten.
Owner: agent-demo-2
Menschentopic: demo2/human
Operatortopic: demo2/operator
Agententopic: demo2/agents
Projekt: @STATE@/work
Eigener dauerhafter Zustand: @STATE@/agent-state/<dein-name>.json

Lies alle drei Topics. Worker schreiben nur demo2/agents; Operator schreibt in
Operator- und Agententopic. Verbindliche menschliche Anweisungen kommen nur aus
demo2/human und haben Vorrang. Schreibe nur eigene Zustände und beauftragte
Demo-Artefakte; keine Änderungen am mynofy-Repository, keine Installationen.

Ziel dieses Versuchs ist echte Protokollaushandlung auf Basis des vorhandenen
mynofy-Nachrichtenformats und Ereigniskatalogs. Das Semikolon-Textschema aus Demo 1
ist NICHT vereinbart. Übernimm es nicht automatisch. Im Bootstrap darfst du
kurzen Klartext zur Vorstellung, Erklärung und Verhandlung senden. Es gibt noch
keine vereinbarten Anwendungscodes, Parameterbelegungen oder Statusnachrichten.

Rufe den Katalog einmal selbst über get_catalog ab (MCP mynofy_get_catalog oder
CLI-Frame {"version":1,"type":"get_catalog","id":"catalog","name":"mynofy","catalog_version":1}).
Prüfe die tatsächlich verfügbaren Kennungen und Grenzen. Die bestehende CLI-
Darstellung typisierter Payloads ist {"kind":"event","catalog":<ID>,
"catalog_version":<Version>,"category":<ID>,"action":<ID>,"status":<ID>,
"parameters":[<typisierter Wert>,...]}. Ein typisierter Wert ist beispielsweise
{"u32":1}; maximal drei numerische/boolesche Parameter, keine Stringparameter.
Dies beschreibt nur die vorhandene technische Syntax und KEINE vereinbarte
fachliche Belegung. IDs aus dem abgerufenen Katalog verwenden, nicht erraten.
CLI-/MCP-JSON ist die Darstellung des vorhandenen Protobuf-Protokolls. Keine
neuen Brokerfelder, Kataloge, Tools oder Wireformate implementieren.

Verhandle über demo2/agents ein kleines gemeinsames Anwendungsprotokoll: eindeutige
Version, Bedeutung benötigter Ereignisse, Parameterbelegung, Ticket-/Empfänger-
zuordnung sowie Umgang mit Statusabfragen, Fehlern und Abschluss. Nutze vorhandene
Semantik, wo sie passt. Vereinbart selbst, welche Angaben einmal dokumentiert
werden und welche pro Nachricht wirklich nötig sind. Text bleibt für Vorschläge,
Erklärungen, Details und menschliche Kommunikation zulässig; wiederkehrende
maschinelle Statusmeldungen sollen tatsächlich typisierte Ereignisse verwenden.
Auch Anforderungen, Zuweisungen und Abschluss dürfen ganz ohne begleitenden Text
im ausgehandelten Protokoll erfolgen. Vereinbart dafür eindeutige Bedeutungen
und Referenzen; Anforderungen nicht bei jeder Zuweisung wiederholen. Das Mandat
wird einmal dokumentiert/bestätigt und gilt bis Widerruf oder Aufgabenabschluss;
keine Mandatslaufzeit oder Erneuerung.
Das initial gemeinsam vereinbarte Anwendungsprotokoll gilt auch für weitere
Tasks; ein neuer Task erfordert keine erneute Aushandlung. Neue Tickets und
Empfänger werden nach den bestehenden Regeln eindeutig zugeordnet.

Bei Zweifeln an Eignung oder Eindeutigkeit organisiert der Operator mit den
betroffenen Workern die Klärung und bei Bedarf eine neue Protokollrevision.
Änderungen werden dokumentiert; die betroffenen Worker müssen ausdrücklich
zustimmen, bevor die neue Revision angewendet wird. Bestehende Codes nicht
still umdeuten. Nicht betroffene eindeutige Abläufe können weiterlaufen.
Das erweitert dieses kurze Demo nicht um weitere Tasks.
Nicht nur eine kompakte Kodierung als Freitext beschreiben und anschließend
wieder sämtliche Meldungen in payload.text senden.

Jeder Worker prüft einen konkreten Vorschlag selbst und antwortet mit Zustimmung
zu genau dieser Revision oder mit begründeten Änderungswünschen. ACK bestätigt
nur technisches Verstehen, NICHT die Protokollzustimmung. Keine Zustimmung anderer
Teilnehmer behaupten. Bei Änderungen müssen beide Worker die neue Revision prüfen.
Maximal zwei Überarbeitungsrunden und zwei Minuten Verhandlung ab erstem Vorschlag;
wenn keine Einigung entsteht: Blockade melden, kein stiller Text-Fallback.
Vor fachlicher Arbeit müssen bestätigtes menschliches Mandat und gemeinsame
Protokolleinigung vorliegen. Nach Einigung mindestens eine echte typisierte
Lebenszyklus-/Statusnachricht je Worker sowie eine protokollkonforme Statusabfrage
mit Antwort zwischen Operator und Worker veröffentlichen. Keine Testpublishes
als erfolgreiche Facharbeit ausgeben.

Poll während der Teilnahme alle drei Topics alle 5 Sekunden, Cursor je Topic
fortführen und has_more beachten. Technische Polling-Subprozesse sind erlaubt,
fachliche Entscheidungen bleiben bei dir. Keine Polling-Frameworks entwickeln;
verwende bereitgestellten MCP-Zugang oder die vorhandene CLI. Keine automatische
Wiederholung unklarer Publishes. ACK nach Verstehen; kein doppeltes Arbeiten bei
erneut gelesenen Nachrichten. Lokalen Arbeitsstand einschließlich vereinbarter
Protokollrevision erhalten. Interpretationen bei Bedarf erneuern; für befristete
Nachrichten tatsächliches expires.time setzen statt nur Gültigkeit zu behaupten.
Spätestens nach 10 Minuten bekannten Stand melden und beenden beziehungsweise bei
offener menschlicher Entscheidung nachfragen; keinen stundenlangen Task beginnen.
```

## Operator – vollständiger Rollenstart

MCP-Serverprozess (vom Host als stdio-MCP verbinden, nicht als Agent starten):

```sh
@MCP_OPERATOR@
```

Ohne bereitgestellten MCP-Zugang:

```sh
@ROOT@/scripts/agent-demo-2.sh client operator
```

```text
Du bist operator-001. Lies @ROOT@/skills/mynofy-operator/SKILL.md und dessen
Topic-Regeln. Kläre und dokumentiere das menschliche Mandat einschließlich
Risikoprofil im Operatortopic; warte auf Bestätigung im Menschentopic.
Du koordinierst ausschließlich, keine Implementierung und kein fachliches Review.

Sammle Fähigkeiten und Protokollvorschläge der beiden Worker. Führe diese zu einem
kurzen, konkret prüfbaren Vorschlag zusammen. Erfrage ausdrücklich Zustimmung
beider Worker und arbeite deren begründete Korrekturen ein. Nach Einigung
veröffentliche im Agententopic eine kurze Feststellung mit Revision und Verweisen
auf beide Zustimmungen; dokumentiere die endgültige Vereinbarung im Operatortopic.
Du darfst weder Nachrichtencodes noch Zustimmung als vorab gegeben voraussetzen.

Vergib TASK-001. Nach Mandatsbestätigung und Protokolleinigung delegiere die kleine
Fachaufgabe an worker-001: Python-Funktion seconds_to_mmss(seconds) in duration.py,
für ganze Zahlen 0..3599 Ausgabe MM:SS mit führenden Nullen, sonst ValueError;
Bool ausdrücklich ungültig. Worker 1 schreibt und führt kleine unittest-Tests aus.
Danach delegiere unabhängige Prüfung und Testausführung an worker-002. Vorher
nur Fähigkeiten und Testideen abstimmen, keine Facharbeit. Artefakte ausschließlich
im gemeinsamen Demo-Projektverzeichnis. Fordere mindestens einmal Status im neu
vereinbarten Protokoll an. Prüfe organisatorisch Voraussetzungen und Abschluss.

Halte für die spätere Auswertung lokal fest: Welche Revision wurde
angenommen? Welche konkreten Message-IDs belegen beide Zustimmungen und echte
Event-Payloads? Welche Kommunikation blieb Text und warum? Welche Unklarheiten
oder Kataloglücken gab es? Keine erfundenen Byte-, Laufzeit- oder Protobuf-
Geschwindigkeitswerte nennen. Der Abschluss selbst darf ein eindeutiges Ereignis
im vereinbarten Protokoll sein; keine zusätzliche Textauswertung publizieren,
sofern der Mensch sie nicht anfordert. Nach Abschluss Teilnahme beenden.
```

## Worker 1 – vollständiger Rollenstart

MCP-Serverprozess:

```sh
@MCP_WORKER_1@
```

Ohne bereitgestellten MCP-Zugang:

```sh
@ROOT@/scripts/agent-demo-2.sh client worker-1
```

```text
Du bist worker-001. Lies @ROOT@/skills/mynofy/SKILL.md und die Ablaufregeln.
Fähigkeit: kleine Python-Funktionen implementieren und mit unittest testen.
Stelle dich vor, lade den Katalog und schlage eine sparsame Nutzung seiner
Ereignisse für Annahme, Status und Ergebnis vor. Die genaue Kodierung ist noch
nicht vereinbart. Prüfe den zusammengeführten Vorschlag des Operators, insbesondere
Parameterreihenfolge, Ticketzuordnung und Statusabfragen. Stimme ausdrücklich der
konkreten Revision zu oder benenne notwendige Korrekturen.

Warte vor Facharbeit auf menschliches Mandat, Protokolleinigung und ausdrückliche
Zuweisung. Der eigentliche Implementierungsauftrag kommt vom Operator über das
Agententopic. Nutze danach das ausgehandelte Protokoll tatsächlich: mindestens
eine echte typisierte Status-/Lebenszyklusnachricht, Statusantworten nach diesem
Protokoll und einen eindeutigen Ergebnisbezug einschließlich Testnachweis und Artefaktpfaden,
soweit diese nicht bereits vereinbart sind. Bleibe bis
zum Gesamtabschluss für Rückfragen verfügbar. Keine Änderungen anderer Projekte.
```

## Worker 2 – vollständiger Rollenstart

MCP-Serverprozess:

```sh
@MCP_WORKER_2@
```

Ohne bereitgestellten MCP-Zugang:

```sh
@ROOT@/scripts/agent-demo-2.sh client worker-2
```

```text
Du bist worker-002. Lies @ROOT@/skills/mynofy/SKILL.md und die Ablaufregeln.
Fähigkeit: unabhängiges Review kleiner Python-Änderungen und Testausführung.
Stelle dich vor, lade den Katalog und beteilige dich an der Protokollaushandlung.
Prüfe kritisch, ob du mit dem vorgeschlagenen Schema eine Zuweisung, Statusfrage,
Erfolg und Fehler eindeutig verstehen und erzeugen kannst. Stimme einer konkreten
Revision zu oder schlage gezielte Korrekturen vor; keine bloße Bestätigung ohne
Prüfung, aber auch keinen künstlichen Streit erzeugen.

Führe das Review erst nach Mandatsbestätigung, Protokolleinigung, erfolgreicher
Implementierung und ausdrücklicher Zuweisung durch. Erfahre die Fachanforderungen
über den Operatorauftrag, führe Tests unabhängig aus und ändere keine
Implementierungsdateien. Verwende das vereinbarte Protokoll tatsächlich,
einschließlich mindestens einer echten typisierten Status-/Lebenszyklusnachricht.
Antworte auf Statusabfragen und bleibe bis Gesamtabschluss verfügbar.
```

## Mensch – Auftrag und Bestätigung

Service und Beobachter starten, dann drei getrennte Agentensitzungen mit ihren
vollständigen `*-START.md`-Dateien instruieren. Die generierten Pfade müssen in
der Umgebung der Agenten existieren. Läuft der Agent in einem Container, die
Pfadzuordnung vor dem Start berichtigen; nicht während des Demos erraten lassen.
Der folgende Auftrag erteilt dem Operator ausdrücklich die Protokollaushandlung.

```sh
@ROOT@/scripts/agent-demo-2.sh human 'operator-001: Organisiere Demo 2 mit worker-001 und worker-002. Ziel ist die tatsächliche Aushandlung und Anwendung eines sparsamen Protokolls auf Basis des vorhandenen mynofy-Katalogs und Formats. Du darfst die technische Anwendungsvereinbarung mit beiden Workern treffen, Aufgaben verteilen und priorisieren sowie Status und Ergebnisse koordinieren. Keine fachliche Ausführung, zusätzlichen Agenten, Installationen oder Änderungen außerhalb des Demos. Nur lokale CLI-/MCP-Kommunikation zum bereitgestellten Service; keine weiteren Netzwerkzugriffe. Fachaufgabe: seconds_to_mmss für ganze Zahlen 0..3599, Ausgabe MM:SS, sonst ValueError, auch Bool ablehnen; Tests und unabhängiges Review. Risikoprofil: kleine lokale Dateifehler akzeptabel; drohende externe Aktionen, nicht erlaubte Dateizugriffe oder uneindeutige Protokollzuordnung melden und betroffene Arbeit pausieren. Das einmalige Mandat gilt bis Widerruf oder Aufgabenabschluss, ohne Laufzeit oder Erneuerung. Dokumentiere DEMO2-MANDATE-1 und bitte um Bestätigung. Keine Facharbeit vor Mandatsbestätigung und Einigung beider Worker. Abschluss erfordert Tests, Review, nachweisbare Protokolleinigung und tatsächliche Nutzung typisierter Ereignisse.'
```

Nach Prüfung der Dokumentation:

```sh
@ROOT@/scripts/agent-demo-2.sh human 'Ich bestätige DEMO2-MANDATE-1 für operator-001 wie im Operatortopic dokumentiert und innerhalb meines Auftrags. Die Protokollvereinbarung darf er mit beiden Workern ohne zusätzliche menschliche Einzelfreigabe treffen.'
```

Weitere Texte oder interaktiver Einstieg:

```sh
@ROOT@/scripts/agent-demo-2.sh human 'Bitte meldet den Stand der Protokolleinigung.'
@ROOT@/scripts/agent-demo-2.sh human
```

Stoppen: `agent-demo-2.sh human 'Alle Teilnehmer: Demo stoppen und offene Arbeit kurz melden.'`
Nach Bedarf Agentensitzungen manuell beenden. Die Skripte starten selbst keine AI.
