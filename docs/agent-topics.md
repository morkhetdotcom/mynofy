# Drei Rollen-Topics

Die maßgeblichen Rechte und Ablaufregeln stehen in den
[gemeinsamen Skill-Regeln](../skills/mynofy/references/workflow.md#topics-und-autorität).
Diese Übersicht ordnet die vom Betreiber bereitgestellten Topics ein:

- **Menschentopic:** verbindliche menschliche Anweisungen, Bestätigungen, Änderungen,
  Widerruf, Stopp und Wiederaufnahme. Nur menschliche Clients schreiben.
- **Operatortopic:** Mandat, Risiken, Übernahme und Rückmeldungen an den Menschen.
  Operator und Mensch schreiben.
- **Agententopic:** Vorstellungen, Protokollaushandlung, Aufträge, Interpretationen,
  Status und Ergebnisse. Worker, Operator und Mensch schreiben.

Alle teilnehmenden Worker und Operatoren lesen alle drei Topics. Menschliche
Beiträge außerhalb des Menschentopics erhalten keine besondere Autorität.
Topic-Namen sind Anwendungskonfiguration, keine fest eingebauten Service-Rollen.
Der Owner setzt die entsprechenden ACLs durch; der Broker interpretiert weder
Mandate noch fachliche Rollen oder Protokollvereinbarungen.

Die frühere Vier-Topic-Aufteilung ist für diese Agenten-Skills abgelöst. Separat
konfigurierte Adapter dürfen weiterhin andere Topics verwenden; daraus entsteht
keine Pflicht zu zusätzlichen Topics im Agentenablauf.
