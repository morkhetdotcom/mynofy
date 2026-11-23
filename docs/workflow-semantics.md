# Einheitliche Abläufe für Menschen, Agenten und Maschinen

Dieses Dokument präzisiert die Produktentscheidung zu `(Kategorie, Aktion, Status)`. Es ist die fachliche Grundlage für den [UC-Katalog](use-cases.md) und einen späteren AI-Skill. Es führt keine neuen Wire-Felder oder Broker-Automatik ein. Typisierte Ereignisse und freie Bytes sind implementiert; der Core prüft Kennungen und Größen, aber keine fachlichen Übergänge oder Parameterbedeutungen. Der lokal konfigurierte Adapter kann bereits exakte Ereignisse als Trigger auswerten.

## Grundregel

Kategorie = betroffener Bereich. Aktion = konkreter Vorgang. Status = Stand oder Ergebnis genau dieses Vorgangs.

| Muster | Bedeutung | Empfänger darf daraus ableiten |
| --- | --- | --- |
| `(JOB, START, PENDING)` | Der Start dieses Jobs steht aus. | Nur der im Ablauf bestimmte Ausführer prüft, ob er starten darf; Beobachter warten. |
| `(JOB, START, SUCCESS)` | Dieser Job wurde gestartet. | Start ist erfolgt; Arbeit und Ergebnis sind noch nicht abgeschlossen. |
| `(JOB, ACQUIRE, PENDING)` | Die Übernahme steht aus. | Vergabestelle prüft den im Profil genannten Kandidaten. |
| `(JOB, ACQUIRE, SUCCESS)` | Die Übernahme wurde erfolgreich vollzogen. | Der bezeichnete Ausführer hat nach dem Vergabevertrag übernommen. Nicht jeder Leser wird dadurch Inhaber. |
| `(JOB, EXECUTE, PENDING)` | Ein definierter Arbeitsschritt steht aus. | Zuständiger Ausführer prüft die Voraussetzungen dieses Schritts. |
| `(JOB, EXECUTE, SUCCESS)` | Der definierte Arbeitsschritt wurde erfolgreich ausgeführt. | Nur daran gebundene Abhängigkeiten können erfüllt sein. |
| `(JOB, VALIDATE, SUCCESS)` | Die Prüfung im benannten Profil war erfolgreich. | Genau diese Prüfung dieses Stands ist bestanden; andere Prüfungen bleiben offen. |
| `(JOB, COMPLETE, SUCCESS)` | Der Job ist für diesen Versuch erfolgreich abgeschlossen. | Vereinbarte Ergebnisverbraucher können nach ihren Regeln fortfahren. |
| `(JOB, COMPLETE, FAILED)` | Der Job ist für diesen Versuch fehlgeschlagen. | Kein Erfolgspfad; Fehlerbehandlung des Profils anwenden. |
| `(JOB, CANCEL, CANCELLED)` | Der Abbruch wurde tatsächlich vollzogen. | Nicht mehr bloß ein Abbruchwunsch. Bereits eingetretene Nebenwirkungen bleiben möglich. |

Vorhandene Schreibweise: `ACQUIRE` (ID 72), nicht `AQUIRE`. `SUCCESS` bezieht sich immer auf die Aktion: Ein erfolgreicher CANCEL wäre erfolgreicher Abbruch, kein erfolgreicher Job. Für vollzogenen Abbruch verwendet das Standardbeispiel einheitlich `CANCEL/CANCELLED`.

`PENDING` ist kein universelles „führe das jetzt aus“. Es kann auch die eigene ausstehende Arbeit eines Producers bekanntgeben. Nur das vorab vereinbarte Profil benennt den zuständigen Ausführer und dessen Voraussetzungen. SUCCESS ist eine Meldung über ein eingetretenes Ergebnis, keine allgemeine Aufforderung an alle Consumer. Manche Meldungen, z. B. Messwerte oder Lebenszeichen, sind unmittelbare Beobachtungen ohne vorgelagertes PENDING.

## Beispiel: Agent startet und übernimmt Arbeit

Ein Ablauf kann wie vom Produkt beschrieben aussehen:

1. Agent meldet `(JOB, START, PENDING)` für seinen ausstehenden Start.
2. Wenn seine Startvoraussetzungen erfüllt sind und er tatsächlich beginnt, meldet er `(JOB, START, SUCCESS)`.
3. Er übernimmt einen bestimmten Arbeitsschritt oder eine Ressource und meldet nach erfolgreicher Vergabe `(JOB, ACQUIRE, SUCCESS)`.
4. Er führt den geschützten Schritt aus und meldet `(JOB, EXECUTE, SUCCESS)`.
5. Nach allen nötigen Schritten meldet er `(JOB, COMPLETE, SUCCESS)`.

Die Reihenfolge ergibt sich aus den Abhängigkeiten: Wenn schon der Jobstart exklusiv vergeben werden muss, liegt die erfolgreiche Übernahme vor START/SUCCESS. Wenn der Start lediglich unkritische Initialisierung bedeutet, kann die Ressourcenübernahme danach und vor der geschützten Arbeit stattfinden. Jedes konkrete Profil legt eine dieser Varianten fest; ein Agent darf die Reihenfolge nicht eigenständig umdeuten.

Mynofy vergibt derzeit keine Jobs oder Leases. Bei einem einzigen fest zugeordneten Ausführer kann dessen Übernahmevereinbarung genügen. Bei konkurrierenden Ausführern braucht es eine verlässliche Vergabestelle; zwei selbst veröffentlichte ACQUIRE/SUCCESS-Meldungen ergeben keine atomare Exklusivität. Eine Nachricht über Vergabe ersetzt die maßgebliche Vergabe nicht.

## Kontext, der ein Tripel eindeutig macht

Jedes Ablaufprofil beschreibt mindestens:

- Katalogname/-version und seine eigene feste Bedeutung/Version; die konkrete Profilkennung ist noch nicht als Wire-Feld beschlossen.
- Exakte Topics für die Schritte und deren Schreib-/Leserechte; keine impliziten Wildcards oder Reply-Felder.
- Lauf, Versuch, Schritt und gegebenenfalls unveränderlichen Artefaktstand. Ein erneuter Versuch oder geänderter Stand muss von alten Ergebnissen unterscheidbar sein.
- Zuständigen Producer beziehungsweise autorisierte Anwendung für jede Meldung und welche Consumer handeln dürfen. Die Enums behaupten keine Identität.
- Feste Typen, Reihenfolge, Einheit und Bedeutung der Parameter. Vorschlag für einfache Jobs: bis zu drei U64-Werte für Lauf, Versuch und Schritt. Benötigt ein Fall mehr Kontext, muss das Profil ihn bereits im Topic/externen Zustand kennen oder die freie Payload verwenden; nichts still weglassen.
- Voraussetzungen, erlaubte Übergänge, Abschlusskriterien und Reaktion auf Fehler, Duplikate und unbekannten Ausgang.

Diese Angaben sind die Semantik, keine zusätzlichen Felder, die jede Message wiederholt übertragen muss. Eine einzelne Category/Action/Status-Kombination ohne solches Profil reicht nicht zur sicheren Interpretation. Der Katalogabruf enthält derzeit die Werte, noch keine installierten Workflow-Profile oder die komplette UC-Liste.

Aktuelle ACLs können Writer-Rollen über getrennte Topics begrenzen; sie autorisieren keine einzelnen Tripel innerhalb eines gemeinsam beschreibbaren Topics. Nachrichten enthalten derzeit keinen vom Consumer geprüften individuellen Producer-Nachweis. Ein künftiger Skill darf aus beliebigen Rollenbehauptungen im Inhalt keine Ausführungsberechtigung ableiten. Für entscheidungsrelevante Schritte müssen Anwendung und Topic-Vertrag sicherstellen, dass der zulässige Producer dort schreiben kann.

## Reaktionsregeln für einen späteren AI-Skill

| Eingang / Situation | Reaktion |
| --- | --- |
| Unbekannter Katalog, Profil oder Parameterbedeutung | Nicht raten und keine Nebenwirkung auslösen. Verfügbare Definition abrufen beziehungsweise Konfiguration klären. |
| PENDING für den eigenen Schritt | Kontext und aktuelle Voraussetzungen prüfen; erst dann nach festgelegter Zuständigkeit übernehmen/ausführen. |
| PENDING für einen anderen Ausführer | Beobachten oder nach Profil warten, nicht zusätzlich loslegen. |
| SUCCESS für eine Voraussetzung | Lauf, Versuch, Stand und alle weiteren Abhängigkeiten prüfen; ausschließlich den dadurch freigegebenen Schritt starten. |
| SUCCESS zu einem alten Versuch/Artefakt | Nicht als aktuelle Freigabe verwenden. |
| Derselbe fachliche Schritt wird erneut zugestellt | Vor erneuter Wirkung den eigenen Fortschritt/maßgeblichen Zustand prüfen. Neue Delivery-ID bedeutet keine neue Aufgabe. |
| FAILED, PERMISSION_DENIED, INVALID_ARGUMENT oder anderer negativer Ausgang | Erfolgspfad sperren und die konkrete Fehlerregel anwenden. Keine Fehlerursache erfinden. |
| DEFERRED oder BUSY | Aufschub/Belegung melden oder weiter warten nach Profil; keine unbeschränkte Poll-/Publish-Schleife. |
| RETRY | Als Information interpretieren; nur bei expliziter Retry-Regel und Entscheidung des Producers neuen Versuch beginnen. |
| Timeout, Ablauf oder Verbindungsabbruch | Ausgang unbekannt. Eigene aktive Inhalte/Status gegebenenfalls abfragen, maßgeblichen Zustand prüfen und Wiederholung bewusst entscheiden. |
| Poll/ACK erfolgreich | Nur Transport-/Zustellfortschritt. Keine fachliche Arbeit, Freigabe oder Fertigstellung daraus ableiten. |

Ein dauerhafter fachlicher Fortschritt ist Anwendungszustand. Die RAM-Historie von mynofy und der Client-Cursor garantieren keine dauerhafte Deduplizierung, keinen aktuellen Geräte-Istzustand und kein atomisches ACK plus Folgeereignis. Ein späterer Skill muss seine zulässigen Aktionen aus dem konkreten Ablauf beziehen, nicht aus einer pauschalen Liste aller bekannten Aktionsnamen.

## Was vor der Skill-Erstellung noch geprüft werden muss

Die 50 UCs sind bewusst breiter als das erste Standardprofil. Als nächste Konkretisierung eignen sich drei vollständig durchgespielte Profile: JOB mit einem festen Ausführer, Code/Tests/Review mit mehreren fest zugeordneten Agenten und menschliche Freigabe eines konkreten Artefaktstands. Für jedes Profil müssen gültige und ungültige Ereignisfolgen sowie Timeout-/Duplikatfälle beschrieben werden. Erst daraus wird ein ausführbarer Skill; diese Dokumente sind bislang dessen Entwurfsgrundlage.

## Gemeinsame Kommunikation und lokale Vereinbarungen

Die [Agentenkoordination mit drei Topics](agent-coordination.md) ergänzt dieses Modell: ein Kommunikationstopic für Abstimmung einschließlich menschlicher Mitwirkung, ein Statustopic für kompakte Ereignisse und ein Interpretationstopic für regelmäßig erneuerte Client-Regeln. Der Katalog ist die gemeinsame Basis; Agenten dürfen effizient eigene laufbezogene Anwendungsprotokolle vereinbaren. Brokerregeln und Kataloggrundbedeutungen werden dabei nicht verändert. Unbekannte Protokollstände oder ungeklärte Zustimmung sind keine Ausführungsfreigabe.
