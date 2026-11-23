# Produktprüfung: Interaktionen und Ereigniskatalog

Stand: 11. November 2026. Grundlage ist der eingebaute Entwurf `mynofy`, Version 1. Diese Prüfung enthält Produktvorschläge, keine zusätzliche Laufzeitfunktion. Verfügbar sind JSON-Publish/Poll, ACL, Ablauf, technische ACK/NACK, Producer-Abfragen und Katalogabruf. Typisierte Ereignisse, freie Byte-Payloads mit hartem 4096-Byte-Limit, Protobuf und lokal konfigurierte Client-Adapter sind implementiert. Fachliche Ablaufprofile und automatische Protokollaushandlung bleiben Entwurf; Service-Tunnel sind inzwischen implementiert.

## Produktperspektive

Ein Producer möchte einen anderen Teilnehmer informieren, um eine Entscheidung bitten, eine Aktion anstoßen oder einen kompakten Zustand mitteilen. Menschen handeln über eine Anwendung, Geräte über einen Client oder Gateway und Agenten über ihr SDK. Jeder kann Producer und Consumer sein. Die fachliche Rolle kommt aus Anwendung und Topic; der Broker authentifiziert den tatsächlichen Client und vergibt keine impliziten Rechte an einen angeblich vertretenen Menschen oder ein Gerät.

Die kleine Ereignisvariante soll wiederkehrende, eindeutig definierte Mitteilungen ohne Texte transportieren. Freie Bytes bis 4096 Byte dienen Kurztexten und kleinen anwendungsspezifischen Inhalten. Große Artefakte bleiben außerhalb des Brokers. Ein URL-/Hash-Verweis benötigt eventuell die freie Variante; numerische IDs sind nur dann ausreichend, wenn die Anwendung ihre Bedeutung bereits kennt.

In den folgenden Tabellen sind nur Ausdrücke in der Spalte „Vorhandene Werte“ Kombinationen vorhandener Kennungen. Die Tripel folgen der unten beschriebenen einheitlichen Semantik; konkrete Parametertypen und Ereignissignaturen sind noch Entwurf. Fehlende Namen in anderen Spalten sind Vorschläge. Der Katalog macht einen Trigger nicht ausführbar, erzeugt keine Workflows und ist kein Zustandsautomat.

## Einheitliche Semantik statt zusätzlicher Zustandsnamen

Die aktuelle Produktentscheidung nutzt vorhandene Tripel `(Kategorie, Aktion, Status)`: `PENDING` bedeutet, dass genau diese Aktion aussteht; `SUCCESS`, dass genau diese Aktion erfolgreich eingetreten oder abgeschlossen ist. Beim Start bedeutet SUCCESS „gestartet“, nicht „Job erledigt“. Für die Übernahme lautet die vorhandene Schreibweise `ACQUIRE`, nicht `AQUIRE`. Kein zusätzliches REQUEST/EVENT-Feld und keine neuen Status ACCEPTED/RUNNING sind für dieses Grundmuster erforderlich.

Wer handeln darf, hängt zusätzlich vom vereinbarten Ablaufprofil, Topic, Lauf/Versuch, Artefaktstand und der Rolle der veröffentlichenden Anwendung ab. Ein Tripel allein ist keine Ausführungsfreigabe. Derselbe Agent kann einen ausstehenden Start melden und nach erfüllten Voraussetzungen seinen erfolgreichen Start bestätigen. Eine PENDING-Meldung ist keine an jeden Leser gerichtete Aufforderung. Ein Beobachter darf daraus keine eigene Ausführung ableiten.

Die Tabelle ist größer gefasst, damit daraus später ein AI-Skill mit konkreten Reaktionsregeln entstehen kann. Sie ist noch kein installierter Skill. Die [maschinenlesbaren 50 UCs](../protocol/catalogs/use-cases-v1.json) enthalten pro UC Auslösertripel, Meldungen, benötigten Kontext, erlaubte Empfängerreaktion und Fehlerregel. Alle referenzierten Kennungen existieren bereits; die Signaturen/Profiles sind Entwurf. Der Katalogabruf liefert weiterhin den Ereigniskatalog, nicht automatisch diesen UC-Entwurf.

## 50 Anwendungsfälle

Die Spalte „Anforderung → Meldungen“ beschreibt Alternativen, keine Anweisung, alle Ergebnisse zu senden. Wenn ein UC nur eine Beobachtung beschreibt, wird keine künstliche PENDING-Phase eingeführt. Die Meldungslisten sind jeweils auf den UC beschränkt; nicht jede Operation kann jeden Fehler oder jeden Erfolg liefern. Insbesondere ist ein bekannt positives Einzelresultat niemals eine Freigabe für eine andere Aktion.

### Aufträge

| UC | Ablauf / Beteiligte | Anforderung → Meldungen | Kontext | Empfängerregel / Fehlerfall |
| --- | --- | --- | --- | --- |
| UC-01: Auftrag starten | Mensch/Anwendung → Worker | `(JOB, START, PENDING)` → `(JOB, START, SUCCESS)`, `(JOB, START, FAILED)` | Lauf, Versuch, Auftragsdefinition | Worker prüft Voraussetzungen, übernimmt nach Vergaberegel und meldet gestarteten Lauf. Start fehlgeschlagen: keine Fertigstellung behaupten. |
| UC-02: Auftrag übernehmen | Worker ↔ Koordinator | `(JOB, ACQUIRE, PENDING)` → `(JOB, ACQUIRE, SUCCESS)`, `(JOB, ACQUIRE, FAILED)` | Lauf, Versuch, Kandidat, Vergabeinstanz | Nur bestätigter Inhaber beginnt exklusive Arbeit. BUSY/CONFLICT: nicht arbeiten; Meldung allein vergibt keinen Job. |
| UC-03: Arbeitsschritt ausführen | Worker → Anwendung/Agent | `(JOB, EXECUTE, PENDING)` → `(JOB, EXECUTE, SUCCESS)`, `(JOB, EXECUTE, FAILED)` | Lauf, Versuch, Schrittnummer, bekannte Eingaben | Nachfolger wartet auf SUCCESS dieses Schritts und prüft weitere Voraussetzungen. FAILED/PARTIAL: gesperrte Folgeschritte nicht starten. |
| UC-04: Auftrag abschließen | Worker → Auftraggeber | `(JOB, COMPLETE, PENDING)` → `(JOB, COMPLETE, SUCCESS)`, `(JOB, COMPLETE, FAILED)` | Lauf, Versuch, Ergebnisstand | Auftraggeber akzeptiert Ergebnis nur für aktuellen Versuch. FAILED: Ergebnis nicht als verwendbar markieren. |
| UC-05: Auftrag abbrechen | Mensch/Anwendung → Worker | `(JOB, CANCEL, PENDING)` → `(JOB, CANCEL, CANCELLED)`, `(JOB, CANCEL, CONFLICT)` | Lauf, Versuch, Abbruchziel | Worker stoppt soweit möglich und meldet CANCELLED. CONFLICT bei bereits abgeschlossenem Auftrag: Wirkung klären. |
| UC-06: Start zurückstellen | Worker → Auftraggeber | `(JOB, START, PENDING)` → `(JOB, START, DEFERRED)`, `(JOB, START, FAILED)` | Lauf, Versuch, fehlende Voraussetzung | DEFERRED meldet Aufschub; Coordinator entscheidet über spätere Freigabe. Kein stiller Timer oder automatischer Retry durch Broker. |
| UC-07: Auftrag einreihen | Anwendung → Scheduler | `(JOB, QUEUE, PENDING)` → `(JOB, QUEUE, SUCCESS)`, `(JOB, QUEUE, FAILED)` | Lauf, Versuch, Zielqueue | Scheduler meldet erst SUCCESS wenn tatsächlich eingereiht. RESOURCE_EXHAUSTED: keine Annahme behaupten. |
| UC-08: Auftrag aus Queue nehmen | Scheduler → Worker | `(JOB, DEQUEUE, PENDING)` → `(JOB, DEQUEUE, SUCCESS)`, `(JOB, DEQUEUE, FAILED)` | Lauf, Versuch, Queue, Vergaberegel | SUCCESS meldet Entnahme; exklusive Vergabe separat prüfen. NOT_FOUND: nicht aus veraltetem Queue-Ereignis starten. |
| UC-09: Fortschritt melden | Worker → Mensch/Monitor | Beobachtung ohne Anforderung → `(JOB, UPDATE, PARTIAL)`, `(JOB, UPDATE, SUCCESS)`, `(JOB, UPDATE, FAILED)` | Lauf, Versuch, processed/total mit festen Einheiten | Empfänger aktualisiert Anzeige; startet keinen Folgeschritt. PARTIAL ist kein Abschluss; total=0/ungültige Werte nach Profil behandeln. |
| UC-10: Auftrag erneut versuchen | Producer → Worker | `(JOB, START, PENDING)` → `(JOB, START, SUCCESS)`, `(JOB, START, FAILED)` | Gleicher fachlicher Lauf, neuer Versuch, neue Message-ID | Neuen Versuch nur nach expliziter Producer-Entscheidung und Duplikatprüfung starten. Unklarer vorheriger Ausgang: keine automatische erneute Nebenwirkung. |

### AI

| UC | Ablauf / Beteiligte | Anforderung → Meldungen | Kontext | Empfängerregel / Fehlerfall |
| --- | --- | --- | --- | --- |
| UC-11: Code erzeugen | Mensch/Planer → Coding-Agent | `(JOB, CREATE, PENDING)` → `(JOB, CREATE, SUCCESS)`, `(JOB, CREATE, FAILED)` | Lauf, Versuch, Repository- und Eingabestand | Agent erzeugt Artefakt; CREATE/SUCCESS bedeutet erzeugt, nicht geprüft. FAILED/PARTIAL: Review nur mit ausdrücklich verwendbarem Stand. |
| UC-12: Code prüfen | Coding-Agent → Review-Agent | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Lauf, Versuch, unveränderlicher Artefaktstand, Reviewprofil | Review-Agent prüft; SUCCESS erfüllt genau dieses Reviewprofil. FAILED erfüllt Freigabebedingung nicht; genaue Ursache separat zuordnen. |
| UC-13: Änderungen nach Review | Review-Agent → Coding-Agent | `(JOB, UPDATE, PENDING)` → `(JOB, UPDATE, SUCCESS)`, `(JOB, UPDATE, FAILED)` | Lauf, neuer Artefaktstand, bekannte Reviewanforderungen | Coding-Agent überarbeitet und fordert VALIDATE/PENDING für neuen Stand an. Alte erfolgreiche Prüfung gilt nicht für geänderten Stand. |
| UC-14: Tests ausführen | Coding-Agent → Test-Agent/CI | `(JOB, EXECUTE, PENDING)` → `(JOB, EXECUTE, SUCCESS)`, `(JOB, EXECUTE, FAILED)` | Lauf, Artefaktstand, vereinbarte Testschritt-ID | SUCCESS dieses Testschritts kann definierte Qualitätsbedingung erfüllen. Testfehler und fehlende Testausführung unterscheiden; beides keine Freigabe. |
| UC-15: Information abrufen | Agent → Such-/Recherche-Agent | `(RESOURCE, QUERY, PENDING)` → `(RESOURCE, QUERY, SUCCESS)`, `(RESOURCE, QUERY, FAILED)` | Lauf, bekannte Frage/Quellenregeln, Ergebnisreferenz | Empfänger stellt Ergebnis extern oder als freie Payload bereit. NOT_FOUND/UNAVAILABLE: keine Antwort erfinden. |
| UC-16: Ergebnis vergleichen | Zwei Agenten → Vergleichs-Agent | `(RESOURCE, COMPARE, PENDING)` → `(RESOURCE, COMPARE, SUCCESS)`, `(RESOURCE, COMPARE, FAILED)` | Lauf, zwei feste Ergebnisstände, Vergleichskriterium | SUCCESS besagt Vergleich ausgeführt; Gleichheit separat über MATCH/MISMATCH melden. FAILED ist kein Beleg für unterschiedliche Inhalte. |
| UC-17: Agentenübergabe | Planer-Agent → Fach-Agent | `(JOB, ACQUIRE, PENDING)` → `(JOB, ACQUIRE, SUCCESS)`, `(JOB, ACQUIRE, FAILED)` | Lauf, Versuch, bestätigter Übergabe-/Vergabestand | Neuer Agent arbeitet erst nach passender Vergabe; alter beendet exklusive Arbeit. Verlorene Bestätigung: keine doppelte Übernahme unterstellen. |
| UC-18: Tool-Zugriff prüfen | Agent → Policy-Anwendung | `(SECURITY, VALIDATE, PENDING)` → `(SECURITY, VALIDATE, SUCCESS)`, `(SECURITY, VALIDATE, FAILED)` | Lauf, konkrete Operation/Ziel, Regelstand | Nur SUCCESS des autorisierten Prüfers und passende Ausführungsregeln erlauben Tool-Aufruf. PERMISSION_DENIED: Operation unterlassen; Meldung ändert keine ACL. |

### Menschen

| UC | Ablauf / Beteiligte | Anforderung → Meldungen | Kontext | Empfängerregel / Fehlerfall |
| --- | --- | --- | --- | --- |
| UC-19: Veröffentlichung freigeben | Anwendung → Mensch über UI | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Lauf, Artefaktstand, explizites menschliches Freigabeprofil | UI meldet SUCCESS nur nach ausdrücklicher Freigabe; Anwendung prüft passendes Profil. FAILED bedeutet nicht freigegeben; DEFERRED vertagt Entscheidung. |
| UC-20: Kurznachricht senden | Mensch ↔ Mensch | `(MESSAGE, SEND, PENDING)` → `(MESSAGE, SEND, SUCCESS)`, `(MESSAGE, SEND, FAILED)` | Exakte Topics, freie Kurztext-Payload, vereinbarte Antwortkonvention | UI zeigt Inhalt; freier Text startet keine Maschinenaktion. Zustellung/ACK belegt weder Lesen noch Zustimmung. |
| UC-21: Kenntnisnahme melden | Mensch über UI → Anwendung | Beobachtung ohne Anforderung → `(MESSAGE, RECEIVE, SUCCESS)`, `(MESSAGE, RECEIVE, FAILED)` | Lauf/Nachricht, explizites Kenntnisnahmeprofil | SUCCESS darf UI-Kenntnisnahme dokumentieren, keine Freigabe. Ohne explizite Benutzerhandlung nicht aus Poll oder ACK ableiten. |
| UC-22: Wartung ankündigen | Betreiber → Menschen/Geräte | `(SYSTEM, SCHEDULE, PENDING)` → `(SYSTEM, SCHEDULE, SUCCESS)`, `(SYSTEM, SCHEDULE, FAILED)` | Wartungslauf, Termin/Zeitzone, festgelegte Teilnehmer | Empfänger informiert oder plant lokal; keine sofortige Abschaltung. Ablauf der Nachricht führt keine Wartung aus. |
| UC-23: Entscheidung zurückstellen | Mensch → wartender Agent | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, DEFERRED)`, `(JOB, VALIDATE, FAILED)` | Lauf, Artefaktstand, Freigabeprofil | DEFERRED lässt den abhängigen Schritt gesperrt. Ausbleibende Entscheidung ist keine Zustimmung. |
| UC-24: An Betreiber eskalieren | Monitor/Agent → Mensch | Beobachtung ohne Anforderung → `(EVENT, SEND, SUCCESS)`, `(EVENT, SEND, FAILED)` | Lauf, Anlass, Eskalationsstufe, zuständige Person | UI informiert zuständige Person nach eigener Eskalationsregel. Keine Garantie, dass jemand übernimmt; Antwort separat erforderlich. |

### Geräte

| UC | Ablauf / Beteiligte | Anforderung → Meldungen | Kontext | Empfängerregel / Fehlerfall |
| --- | --- | --- | --- | --- |
| UC-25: Gerät einschalten | Mensch/Regel → Gerät | `(SYSTEM, START, PENDING)` → `(SYSTEM, START, SUCCESS)`, `(SYSTEM, START, FAILED)` | Gerät im Topic, Lauf, gewünschter Zustand | Gerät meldet SUCCESS nach tatsächlichem Start laut Profil. BUSY/FAILED: Anzeige nicht als eingeschaltet setzen. |
| UC-26: Gerät ausschalten | Mensch/Regel → Gerät | `(SYSTEM, STOP, PENDING)` → `(SYSTEM, STOP, SUCCESS)`, `(SYSTEM, STOP, FAILED)` | Gerät, Lauf, gewünschter Zustand, Ausführungsregeln | Gerät meldet erfolgreichen Stillstand. TIMEOUT belegt keinen Stillstand; Istzustand abfragen. |
| UC-27: Sollwert ändern | Regler/Mensch → Gerät | `(RESOURCE, UPDATE, PENDING)` → `(RESOURCE, UPDATE, SUCCESS)`, `(RESOURCE, UPDATE, FAILED)` | Gerät/Eigenschaft im Topic, Lauf, Einheit, Sollwert | SUCCESS erst wenn im Profil definierter Sollwert übernommen ist. INVALID_ARGUMENT: Wert ablehnen, nicht still kürzen. |
| UC-28: Temperatur beobachten | Sensor → Monitor/Regler | Beobachtung ohne Anforderung → `(TELEMETRY, READ, SUCCESS)`, `(TELEMETRY, READ, FAILED)` | Sensor im Topic, Einheit/Skalierung, Messwert, Messzeit | READ/SUCCESS berichtet Beobachtung; Regel prüft Alter und Grenzwerte. Veraltete Messung nicht zur aktuellen Steuerung verwenden. |
| UC-29: Zählerstand melden | Zähler → Anwendung | Beobachtung ohne Anforderung → `(METRICS, READ, SUCCESS)`, `(METRICS, READ, FAILED)` | Zähler im Topic, Einheit, absolut/delta, Sequenz/Zeitraum | Anwendung interpretiert nach festem Messprofil. Reset/Umlauf/Duplikate berücksichtigen, Delta nicht doppelt addieren. |
| UC-30: Grenzwert melden | Sensor/Monitor → Mensch/Agent | Beobachtung ohne Anforderung → `(RESOURCE, VALIDATE, FAILED)` | Überwachtes Objekt, Regel, Messstand, Schwellwertprofil | FAILED meldet verletzte Bedingung; Empfänger folgt vereinbarter Alarmregel. FAILED allein definiert weder Priorität noch automatische Gegenmaßnahme. |
| UC-31: Wieder normal | Sensor/Monitor → Mensch/Agent | Beobachtung ohne Anforderung → `(RESOURCE, VALIDATE, SUCCESS)` | Dieselbe Regel/Instanz wie Grenzwertmeldung, neuer Messstand | SUCCESS meldet wieder erfüllte Bedingung. Unzugeordnete Normalmeldung darf keinen anderen Alarm schließen. |
| UC-32: Gerät zurücksetzen | Betreiber/Automatisierung → Gerät | `(SYSTEM, RESET, PENDING)` → `(SYSTEM, RESET, SUCCESS)`, `(SYSTEM, RESET, FAILED)` | Gerät, Lauf, Resetumfang und Berechtigung | SUCCESS meldet den definierten abgeschlossenen Reset. Wiederholte Nachricht darf nicht unkontrolliert mehrfach resetten. |

### Betrieb

| UC | Ablauf / Beteiligte | Anforderung → Meldungen | Kontext | Empfängerregel / Fehlerfall |
| --- | --- | --- | --- | --- |
| UC-33: Dienst starten | Supervisor → Dienst | `(SYSTEM, START, PENDING)` → `(SYSTEM, START, SUCCESS)`, `(SYSTEM, START, FAILED)` | Dienst, Lauf, Start-/Bereitschaftsprofil | SUCCESS belegt nur den im Profil genannten Startzustand. Prozessstart ist nicht automatisch Readiness. |
| UC-34: Dienst anhalten | Supervisor → Dienst | `(SYSTEM, SHUTDOWN, PENDING)` → `(SYSTEM, SHUTDOWN, SUCCESS)`, `(SYSTEM, SHUTDOWN, FAILED)` | Dienst, Lauf, Umfang des geordneten Stopps | SUCCESS erst nach erfüllter Shutdown-Bedingung. TIMEOUT/FAILED: verbleibende Arbeit und Zustand klären. |
| UC-35: Verbindung aufbauen | Anwendung → Connector | `(NETWORK, CONNECT, PENDING)` → `(NETWORK, CONNECT, SUCCESS)`, `(NETWORK, CONNECT, FAILED)` | Ziel, Verbindungslauf, Authentifizierungsregeln | SUCCESS meldet aufgebaute geprüfte Verbindung. UNAVAILABLE/TIMEOUT: keine sichere Verbindung behaupten. |
| UC-36: Verbindung verloren | Connector → Monitor | Beobachtung ohne Anforderung → `(NETWORK, DISCONNECT, SUCCESS)`, `(NETWORK, DISCONNECT, FAILED)` | Verbindungsinstanz, Beobachtungszeit | SUCCESS berichtet vollzogene Trennung; Monitor entscheidet über Reconnect. Alte Trennung nicht mit neuer Verbindung verwechseln. |
| UC-37: Lebenszeichen | Dienst → Monitor | Beobachtung ohne Anforderung → `(SYSTEM, UPDATE, SUCCESS)`, `(SYSTEM, UPDATE, FAILED)` | Festes Heartbeatprofil, Instanz, Beobachtungszeit/Sequenz | UPDATE/SUCCESS ist nur nach vereinbartem Profil ein Lebenszeichen. Monitor erzeugt TIMEOUT selbst; ausbleibende Message tut nichts im Broker. |
| UC-38: Bereitschaft prüfen | Monitor → Dienst | `(SYSTEM, VALIDATE, PENDING)` → `(SYSTEM, VALIDATE, SUCCESS)`, `(SYSTEM, VALIDATE, FAILED)` | Dienstinstanz, Bereitschaftsprofil, Prüfzeit | SUCCESS erfüllt konkrete Readiness-Bedingung. FAILED/UNAVAILABLE: abhängige Starts gesperrt lassen. |

### Daten

| UC | Ablauf / Beteiligte | Anforderung → Meldungen | Kontext | Empfängerregel / Fehlerfall |
| --- | --- | --- | --- | --- |
| UC-39: Backup speichern | Anwendung → Backup-Worker | `(STORAGE, SAVE, PENDING)` → `(STORAGE, SAVE, SUCCESS)`, `(STORAGE, SAVE, FAILED)` | Lauf, Quellstand, Ziel und Sicherungsprofil | SUCCESS meldet gespeicherten Backupstand. PARTIAL/FAILED: Backup nicht als vollständig verwendbar markieren. |
| UC-40: Backup wiederherstellen | Betreiber → Restore-Worker | `(STORAGE, LOAD, PENDING)` → `(STORAGE, LOAD, SUCCESS)`, `(STORAGE, LOAD, FAILED)` | Lauf, unveränderliche Backupreferenz, Ziel | SUCCESS meldet nach Profil wiederhergestellten Zustand. Teilergebnis ist kein Gesamterfolg; erneute Ausführung explizit prüfen. |
| UC-41: Cache invalidieren | Datenquelle → Cache-Anwendung | `(CACHE, DELETE, PENDING)` → `(CACHE, DELETE, SUCCESS)`, `(CACHE, DELETE, FAILED)` | Schlüssel/Objekt im Topic oder freie Payload, Revision | SUCCESS meldet tatsächlich entfernten/invalidierten Eintrag. Verpasste Revision über maßgeblichen Zustand erkennen. |
| UC-42: Konfiguration ändern | Betreiber → Anwendung | `(RESOURCE, UPDATE, PENDING)` → `(RESOURCE, UPDATE, SUCCESS)`, `(RESOURCE, UPDATE, FAILED)` | Konfigurationsobjekt, Revision, validierter Inhalt | SUCCESS meldet im Profil definierten angewendeten Stand. Mynofy-Startkonfiguration wird dadurch nicht automatisch neu geladen. |
| UC-43: Artefakt bereit | Builddienst → Mensch/Agent | Beobachtung ohne Anforderung → `(RESOURCE, CREATE, SUCCESS)`, `(RESOURCE, CREATE, FAILED)` | Lauf, Artefaktstand, externe Referenz | CREATE/SUCCESS informiert über verfügbares Artefakt. Geändertes oder fehlendes Artefakt erneut prüfen; kein Dateitransfer. |
| UC-44: Datensatz geändert | Anwendung → abhängiger Dienst | Beobachtung ohne Anforderung → `(DATABASE, UPDATE, SUCCESS)`, `(DATABASE, UPDATE, FAILED)` | Objekt, Revision, Schema-/Änderungsprofil | UPDATE/SUCCESS löst nur im passenden Profil ein Nachladen aus. Keine atomare DB-Transaktion plus Publish garantiert. |

### Koordination

| UC | Ablauf / Beteiligte | Anforderung → Meldungen | Kontext | Empfängerregel / Fehlerfall |
| --- | --- | --- | --- | --- |
| UC-45: Schlüssel prüfen | Betreiber/Agent → Policy-Dienst | `(SECURITY, VALIDATE, PENDING)` → `(SECURITY, VALIDATE, SUCCESS)`, `(SECURITY, VALIDATE, FAILED)` | Schlüsselreferenz ohne Secret, Owner, Prüfzeit | SUCCESS meldet positives Prüfergebnis. Entzug/Ablauf kann danach eintreten; aktuelle Serviceprüfung bleibt maßgeblich. |
| UC-46: Ressource reservieren | Worker ↔ Koordinator | `(RESOURCE, ACQUIRE, PENDING)` → `(RESOURCE, ACQUIRE, SUCCESS)`, `(RESOURCE, ACQUIRE, FAILED)` | Ressource, Lauf, Inhaber und ggf. externe Leasegeneration | SUCCESS nur aus verlässlicher Vergabeinstanz als Reservation verwenden. Broker bietet keine Lease; abgelaufene Reserve nicht benutzen. |
| UC-47: Ressource freigeben | Inhaber → Koordinator | `(RESOURCE, RELEASE, PENDING)` → `(RESOURCE, RELEASE, SUCCESS)`, `(RESOURCE, RELEASE, FAILED)` | Ressource, Lauf, gültige Vergabeinstanz/Generation | SUCCESS bestätigt Freigabe im externen Koordinator. Veraltete Freigabe darf keine neue Reservation lösen. |
| UC-48: Gruppe um Entscheidung bitten | Anwendung → mehrere Menschen/Agenten | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Lauf, Stand, Teilnehmermenge, Quorumregel | Koordinator sammelt passende Entscheidungen und entscheidet nach Regel. Zustellzähler sind weder Teilnehmermenge noch Quorum. |
| UC-49: Auf Ergebnis warten | Agent/Anwendung → Agent/Anwendung | `(JOB, COMPLETE, PENDING)` → `(JOB, COMPLETE, SUCCESS)`, `(JOB, COMPLETE, FAILED)` | Lauf, Versuch, erforderliche Arbeitsschritte | Nur passendes COMPLETE/SUCCESS entsperrt den vereinbarten Folgeschritt. Timeout/Ablauf/Verbindungsverlust lässt Ausgang unklar. |
| UC-50: Umgebungen verbinden | Gerät im LAN → lokaler Agent → zentraler Dienst → Mensch | `(JOB, VALIDATE, PENDING)` → `(JOB, VALIDATE, SUCCESS)`, `(JOB, VALIDATE, FAILED)` | Owner je Topic, Lauf, Stand, Rollen und direkte Verbindungen | Fachliche Semantik bleibt gleich; jeder Schritt prüft seinen Kontext. Service-Tunnel bleiben Entwurf; derzeit direkte Owner-Verbindungen nötig. |

## Ausformulierter Ablauf: Code und Review

Pro Lauf nutzt die Anwendung drei exakte Topics, beispielsweise `workflow/4711/communication` für Aufgabe, Abstimmung und menschliche Entscheidungen, `workflow/4711/status` für Job-/Schrittmeldungen und `workflow/4711/interpretations` für regelmäßig erneuerte Client-Regeln. Sie provisioniert alle drei samt ACL. Agenten können im Kommunikationstopic ein passendes Anwendungsprotokoll vereinbaren; der Mensch kann daran mitwirken. Details und offene Implementierungsfragen stehen in der [Drei-Topic-Koordination](agent-coordination.md).

1. Die Anwendung veröffentlicht den Arbeitsauftrag. Ziel/Commit/Anweisung bei Bedarf als freie Bytes; ein kleiner typisierter Trigger reicht nur bei bereits bekanntem Kontext.
2. Codex pollt das Kommunikationstopic und bearbeitet den Auftrag. Sein technisches ACK bestätigt einen Zustellversuch, keine Fertigstellung.
3. Codex veröffentlicht die Fertigmeldung und den konkreten Artefaktstand. Das Ereignis darf nicht als neuer Reviewauftrag interpretiert werden, wenn das Topic diese Bedeutung nicht ausdrücklich trägt.
4. Claude beobachtet den vereinbarten Reviewauslöser in Kommunikation oder Status und meldet entweder eine fachliche Entscheidung oder einen technischen Prüfungsfehler. Das Prüfprofil muss diese Fälle unterscheiden; eine fehlgeschlagene Prüfung erfüllt die Freigabebedingung nicht. Ein Kommentar kann eine separate freie Nachricht sein; benötigte Inhalte sind nicht atomar mit einer zweiten Message gekoppelt. Wenn sie zusammengehören müssen, eine einzelne freie Payload benutzen.
5. Codex wartet auf die Entscheidung zum richtigen Lauf und Artefaktstand. Eine alte Freigabe oder verlorene/abgelaufene Entscheidung darf keine neue Ausführung freigeben. Bei Timeout bleibt der Ausgang unklar; ein Producer entscheidet selbst über Abfrage oder neuen Versuch.

Es werden keine Reply-Felder, Lesestatus-Garantien, automatische Wiederholung oder atomare Verknüpfung von ACK und Antwort eingeführt. Bei mehreren konkurrierenden Workern gelten UC-02 und UC-46; das [Ablaufmodell](workflow-semantics.md) beschreibt die Grenzen der Vergabe.

## Bewertung aller Kategorien

| Gruppe | Vorhandene Kategorien | Empfehlung |
| --- | --- | --- |
| Produktübergreifende Abläufe | `SYSTEM`, `JOB`, `SCHEDULER`, `RESOURCE`, `NETWORK`, `EVENT`, `MESSAGE`, `METRICS`, `TELEMETRY`, `SECURITY` | Behalten. Unterschiede EVENT/MESSAGE sowie METRICS/TELEMETRY noch präzise definieren; nicht dieselbe Meldung mehrfach kategorisieren. |
| Infrastruktur und Diagnose | `CORE`, `ENGINE`, `MEMORY`, `THREAD`, `FILESYSTEM`, `STORAGE`, `DATABASE`, `CACHE`, `RPC`, `HTTP`, `LOCK`, `MUTEX`, `ATOMIC`, `SERIALIZATION`, `DESERIALIZATION`, `LOG`, `DEBUG` | Nützlich für Maschinen und Betriebsdiagnose. Vor einer Kürzung klären, wie viel Low-Level-Diagnose der Basiskatalog bieten soll. |
| Spezifische Domänen | `ASSET`, `BUNDLE`, `RENDER`, `SHADER`, `TEXTURE`, `GPU`, `AUDIO`, `INPUT`, `ANIMATION`, `SCENE`, `ADDRESSABLES`, `STREAMING`, `GAMEPLAY`, `PLAYER`, `AI`, `PHYSICS` | Als Kandidaten für spätere optionale Domänenkataloge markieren, vorerst behalten. `GPU`/`STREAMING` sind auch außerhalb von Spielen nützlich. `AI` bezeichnet eine Domäne, nicht die Identität eines Agenten; ein Agentenauftrag kann schlicht JOB sein. |
| Entfernt | `UNITY` (200), `UNITY_OBJECT` (201), `UNITY_SCENE` (202), `UNITY_ASSET` (203) | Aus dem eingebauten Entwurf entfernt. IDs bleiben reserviert/unbesetzt, keine Neunummerierung anderer Werte. |

Ergebnis: 43 verbleibende Kategorien, 49 unveränderte Aktionen und 21 unveränderte Statuswerte. Die ursprünglichen Importprüfsummen bleiben als Herkunftsnachweis bestehen. Version 1 ist noch ein veränderlicher Entwurf; Clients sollen ihn nach einem Service-Update neu abrufen. Nach Veröffentlichung einer stabilen Version dürfen deren Bedeutungen nicht unter derselben Version geändert werden.

## Aktionen und Status: Wo die Basis reicht

Die CRUD-Aktionen, `START`/`STOP`, `EXECUTE`, `SCHEDULE`, `COMPLETE`, `CANCEL`, `CONNECT`/`DISCONNECT`, `QUEUE`/`DEQUEUE`, `VALIDATE`, `QUERY` und `LOOKUP` decken viele Abläufe ab. Ressourcen- und Synchronisationsaktionen wie `ALLOCATE`, `MAP`, `LOCK`, `ACQUIRE` und ihre Gegenstücke bleiben sinnvoll für Diagnose, bieten aber keine entsprechenden Broker-Funktionen. `SEND`/`RECEIVE` beschreiben Kommunikation; sie ersetzen keine fachliche Benennung des Inhalts.

Bei Status sind `SUCCESS`, `FAILED`, `TIMEOUT`, `CANCELLED`, `PARTIAL`, `SKIPPED` und `DEFERRED` brauchbar, sobald eindeutig ist, auf welche Operation sie sich beziehen. `INVALID_ARGUMENT`, `NOT_FOUND`, `ALREADY_EXISTS`, `BUSY`, `LOCKED`, `CONFLICT`, `UNAVAILABLE`, `RESOURCE_EXHAUSTED`, `PERMISSION_DENIED`, `NOT_IMPLEMENTED` und `CORRUPTED` decken technische Gründe gut ab. `UNSPECIFIED` ist gültig, aber keine Lösung für eine unklare Ereignisbedeutung.

`PENDING` bedeutet nun einheitlich „diese Aktion steht aus“. Ob ein Job in einer Queue liegt oder ein Worker ihn übernommen hat, wird durch `QUEUE/SUCCESS` beziehungsweise `ACQUIRE/SUCCESS` gemeldet. `RETRY` startet keine Wiederholung. `SUCCESS` bedeutet Erfolg genau der benannten Aktion im vereinbarten Profil, keine allgemeine Freigabe.

## Abgleich nach dem vereinheitlichten Ablaufmodell

Die frühere Empfehlung zusätzlicher generischer Status `ACCEPTED`/`RUNNING` und eines REQUEST/EVENT-Feldes ist durch die Entscheidung für Aktion plus Status überholt. Für Grundabläufe genügen vorhandene Aktionen: Übernahme über ACQUIRE, gestarteter Job über START, ausgeführter Schritt über EXECUTE und Gesamtabschluss über COMPLETE. `PENDING` ist immer auf die benannte Aktion bezogen. Separate Schritttopics und Profile unterscheiden z. B. Tests, Review und menschliche Freigabe, ohne automatisch neue globale Enums einzuführen.

Die tatsächlichen Lücken liegen vor allem bei festen Ereignissignaturen: Welche Parameter bedeuten was, welche Rollen dürfen welche Meldungen erzeugen und welche konkreten Voraussetzungen entsperren welche Folgeaktion? Prüffehler und fachlich negatives Ergebnis können beide eine Freigabe verhindern; die Diagnose kann über vorhandene technische Fehlerstatus oder eine freie Erläuterung erfolgen. Ein `VALIDATE/FAILED` darf nicht ohne Kontext als „Code falsch“ erklärt werden.

Für menschliche Kenntnisnahme, Alarm, Heartbeat und Fortschritt reichen vorhandene Tripel als Basis für ausdrücklich benannte Profile. Ob zusätzliche Aktionen die Verständlichkeit verbessern, wird später aus diesen Profilen abgeleitet; sie werden hier nicht hinzugefügt. Ein universeller ausführbarer Automat oder beliebige sichere Aufgabendelegation lässt sich aus den Enums allein nicht ableiten.

## Grenzen des kleinen Parametersatzes

Drei primitive Werte mit höchstens 24 reinen Wertebytes sind für Boolzustände, Zähler und lokal bekannte numerische IDs häufig ausreichend. Die Feldbedeutung muss aus einer unveränderlichen Signatur kommen, etwa Fortschritt `(processed: U64, total: U64)`; diese Signatur ist ein Vorschlag und noch nicht installiert.

Nicht vollständig abgedeckt sind freie Aufgabenbeschreibungen, Reviewkommentare, variable Referenzen, UUID-/Hashwerte und mehrere zusammengehörige Messwerte mit zusätzlicher Zeit-/Einheiteninformation. Der jetzige Grundtypensatz enthält keinen Byte-/String-/UUID-Typ für kleine Ereignisse. Dafür zunächst die freie Variante verwenden oder den Kontext ins pro Lauf vereinbarte Topic legen. Numerische Kürzel ohne vereinbarte Zuordnung sind keine verlustfreie Lösung. Insbesondere ersetzen Message-ID und Cursor keine langfristig eindeutige Workflow-/Artefaktidentität.

Ein verbreiteter Fall ist das Versenden eines Triggers samt kleinem Text. Eine typisierte Message plus separate freie Message hätte keine atomare Zusammengehörigkeit. Wenn beides zwingend zusammen benötigt wird, kann zunächst die einzelne freie Payload beides enthalten; eine optionale Kombination beider Varianten wäre eine eigene, noch nicht beschlossene Erweiterung.

## Was sich durch den Katalog nicht lösen lässt

- Keine exklusive Jobvergabe oder At-most-once-Wirkung durch einen Enum-Wert.
- Kein gespeicherter aktueller Gerätezustand und keine Historie allein durch zeitlich befristete Nachrichten.
- Keine sichere Ausführung beliebiger Aktionen allein durch Publish-Recht. Empfänger prüfen den fachlichen Auftrag und ihre Ausführungsregeln selbst.
- Kein gesicherter Lesestatus eines Menschen durch ACK; keine Liste aller erwarteten Empfänger allein aus Zustellzählern.
- Keine Authentizität eines behaupteten Ursprungs hinter einem Gateway allein durch dessen Katalogwerte.
- Keine automatische Timeraktion, Offline-Meldung, Eskalation oder verteilte Transaktion.

Diese Grenzen gehören zur Anwendung oder erfordern eine spätere ausdrücklich modellierte Funktion. Der Kern bleibt eine schlanke, begrenzte Kommunikationsschicht.

Die ausführbaren Rollenregeln stehen im [Worker-Skill](../skills/mynofy/SKILL.md) und [Operator-Skill](../skills/mynofy-operator/SKILL.md).
