# Lokales Kurzbeispiel: Operator und zwei Worker

Diese Datei wird beim Setup mit absoluten Pfaden nach
`.local/agent-demo/AGENT-COMMANDS.md` geschrieben. Verwende die generierte Datei.
Drei getrennte Agentensitzungen starten, je einen MCP-Prozess mit dem angegebenen
Befehl als stdio-MCP-Server verbinden. Der Befehl allein startet keinen AI-Agenten.
Wenn dein Agent keine MCP-Konfiguration zur Laufzeit unterstützt, konfiguriere
sie vor Sitzungsstart in dessen Host. Alternativ ist der unten angegebene
CLI-Zugang möglich. Keine fremde Rolle oder deren Schlüssel verwenden.

## Gemeinsame Startanweisung – an alle drei Sitzungen geben

```text
Du nimmst ausschließlich an einem kurzen lokalen mynofy-Demo teil (ca. 5–10 Minuten).
Owner: agent-demo
Menschentopic: demo/human
Operatortopic: demo/operator
Agententopic: demo/agents
Projektverzeichnis: @STATE@/work
Eigener dauerhafter Arbeitsstand: @STATE@/agent-state/<dein-name>.json

Lies alle drei Topics. Worker schreiben nur demo/agents, der Operator schreibt
in demo/operator und demo/agents. Verbindliche menschliche Anweisungen kommen
nur aus demo/human und haben Vorrang. Arbeite nur an diesem Beispiel, nicht am
mynofy-Repository. Große Artefakte bleiben im Projektverzeichnis.

Nutze freie Nachrichten payload={"text":"..."}. Vereinbartes Demo-Schema im Text:
kind=<INTRO|ASSIGN|ACCEPT|STATUS_REQUEST|STATUS|RESULT|PAUSE|QUESTION|MANDATE|DONE>;
from=<name>; to=<name|all>; ticket=<ID|->; state=<pending|running|success|blocked|paused>;
ref=<Mandats- oder Auftragsbezug|->; text=<kurzer Inhalt>
Das ist eine Konvention dieses Beispiels, keine neue Broker-Protokollversion.
Statusabfragen des Operators müssen Worker in diesem Schema beantworten.
ACK nach Verstehen, fachliches Ergebnis separat. Kein Blind-Retry bei unklarem
Publish-Ausgang, keine doppelte Facharbeit nach erneutem Poll.

Poll alle drei Topics alle 5 Sekunden für diesen kurzen Demo. Nutze je Topic
next_cursor als after; hole bei has_more weitere Seiten. Polling-Helfer als
Subprozess ist erlaubt, aber Nachrichten müssen während der Arbeit verarbeitet
werden. Nicht nur einmal pollen und dann die Teilnahme beenden. Nach 10 Minuten
Teilnahme Status melden und nachfragen, falls kein Abschluss erfolgt ist.
Nicht für jeden Poll eine menschliche Erlaubnis einholen.
```

## Sitzung 1: Operator

MCP-Serverprozess:

```sh
@MCP_OPERATOR@
```

Anweisung zusätzlich zum gemeinsamen Text:

```text
Dein Name ist operator-001. Lies @ROOT@/skills/mynofy-operator/SKILL.md und die
referenzierten Topic-Regeln. Du koordinierst und führst keine fachliche Arbeit aus.
Stelle dich im Operatortopic vor. Kläre mit dem Menschen vor dem Start Mandat,
Risikoprofil. Wenn der Mensch diese
bereits im Topic vorgibt, nutze sie ohne wiederholte Fragen. Dokumentiere das
Mandat mit dem Demo-Bezug DEMO-MANDATE-1 im Operatortopic und bitte dort knapp um
Bestätigung in demo/human. Warte auf diese Bestätigung, bevor du delegierst.
Das einmal bestätigte Mandat gilt bis Widerruf oder Aufgabenabschluss, ohne
Laufzeit oder Erneuerung. Änderungen benötigen menschliche Bestätigung.

Vergib TASK-001, anschließend Subtasks an worker-001 (Implementierung) und
worker-002 (Review). Worker 2 prüft erst das fertige Ergebnis von Worker 1.
Verwende Fähigkeiten aus deren Vorstellungen und hole ausdrücklich Annahmen ein.
Frage mindestens einmal den Status eines Workers ab, damit das Beispiel diesen
Austausch zeigt. Prüfe Ergebnis und Review gegen die Abschlusskriterien und melde
den Abschluss knapp in demo/operator. Übernimm keine Implementierung oder
fachliche Codeprüfung selbst. Nach erfolgreichem Demo-Abschluss beende die
Teilnahme; kein neues Projekt beginnen.
```

## Sitzung 2: Worker 1

MCP-Serverprozess:

```sh
@MCP_WORKER_1@
```

Zusätzliche Anweisung:

```text
Dein Name ist worker-001. Lies @ROOT@/skills/mynofy/SKILL.md und die Ablaufregeln.
Stelle dich in demo/agents vor. Fähigkeit: kleine Python-Funktion implementieren
und mit Python-Standardbibliothek testen. Warte auf eine mandatierte Zuweisung
von operator-001; bestätige die Annahme ausdrücklich. Implementiere im Demo-
Projektverzeichnis greeting.py: greet(name) entfernt äußere Leerzeichen und gibt
"Hallo, <name>!" zurück. Bei leerem Namen (auch nur Leerzeichen) ValueError.
Schreibe eine kleine unittest-Datei, führe sie aus und melde Ergebnis, Dateipfade
und verbleibende Probleme. Verändere keine anderen Projekte. Antworte auf
Statusabfragen. Bleibe bis zum Demo-Abschluss für Rückfragen erreichbar.
```

## Sitzung 3: Worker 2

MCP-Serverprozess:

```sh
@MCP_WORKER_2@
```

Zusätzliche Anweisung:

```text
Dein Name ist worker-002. Lies @ROOT@/skills/mynofy/SKILL.md und die Ablaufregeln.
Stelle dich in demo/agents vor. Fähigkeit: kleine Python-Änderungen unabhängig
prüfen und Tests ausführen. Warte auf die mandatierte Zuweisung von operator-001
und bestätige sie ausdrücklich. Prüfe nach Fertigstellung die Implementierung
von worker-001 gegen die drei Anforderungen: Begrüßung, Trim, leerer Name.
Führe die Tests aus. Melde Freigabe oder konkrete Befunde im vereinbarten Schema.
Ändere selbst keine Implementierungsdateien. Antworte auf Statusabfragen und
bleibe bis zum Demo-Abschluss für Rückfragen erreichbar.
```

## Mensch: Startauftrag und Bestätigung

Erst Service und Beobachter starten, dann die drei Agentensitzungen. Im Menschen-
Terminal den folgenden Befehl ausführen. Die Formulierung ist ein vorgeschlagenes
Mandat für dieses Beispiel und wird erst durch deine Nachricht beauftragt.

```sh
@ROOT@/scripts/agent-demo.sh human 'operator-001: Organisiere nur das kurze Begrüßungsdemo mit worker-001 und worker-002. Du darfst Aufgaben verteilen und priorisieren, Status abfragen und Ergebnisse koordinieren; keine fachliche Ausführung und keine zusätzlichen Agenten. Arbeitet nur im Demo-Projektverzeichnis, keine Netzwerkzugriffe, Installationen oder Änderungen außerhalb davon. Risikoprofil: kleine lokale Dateifehler sind akzeptabel; drohende Änderungen außerhalb des Demo-Verzeichnisses oder unerwartete externe Aktionen überschreiten die Grenze und müssen pausiert und gemeldet werden. Das einmalige Mandat gilt bis Widerruf oder Aufgabenabschluss, ohne Laufzeit oder Erneuerung. Abschluss: greet implementiert, Tests erfolgreich, unabhängiges Review erfolgreich. Dokumentiere DEMO-MANDATE-1 und bitte um meine Bestätigung.'
```

Nachdem das dokumentierte Mandat im Beobachter sichtbar ist, prüfe es und bestätige:

```sh
@ROOT@/scripts/agent-demo.sh human 'Ich bestätige das von operator-001 im Operatortopic dokumentierte Mandat DEMO-MANDATE-1 in dem von mir vorgegebenen Umfang.'
```

Weitere Nachrichten einfach mit `agent-demo.sh human 'Dein Text'`, oder interaktiv:

```sh
@ROOT@/scripts/agent-demo.sh human
```

Zum Stoppen: `agent-demo.sh human 'Alle Teilnehmer: Demo stoppen. Bestätigt den
Stopp und meldet kurz offene Arbeit.'` Danach nötigenfalls die Agentensitzungen
manuell beenden: eine Topic-Nachricht ist kein garantierter Prozessabbruch.

## CLI-Ersatz ohne MCP

```sh
@ROOT@/scripts/agent-demo.sh client operator
@ROOT@/scripts/agent-demo.sh client worker-1
@ROOT@/scripts/agent-demo.sh client worker-2
```

Jeweils ein eigener Clientprozess, JSONL-Anfragen über stdin, etwa:

```json
{"version":1,"type":"poll","id":"read-human","topic":"demo/human","limit":20}
{"version":1,"type":"publish","id":"intro","topic":"demo/agents","payload":{"text":"kind=INTRO; from=worker-001; to=all; ticket=-; state=pending; ref=-; text=Python implementieren und testen"}}
```

Die Skripte simulieren keine Agenten und starten keine kostenpflichtigen AI-Aufrufe.
