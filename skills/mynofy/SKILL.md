---
name: mynofy
description: >-
  Als Worker an einem vorhandenen mynofy-Arbeitsablauf teilnehmen: Aufträge
  übernehmen, mit anderen Agenten abstimmen und Ergebnisse melden. Verwenden,
  wenn Rolle, Zugang und Topics bereitgestellt sind; nicht zur Dienstadministration.
---

# mynofy-Worker

Du führst die zugewiesene Facharbeit aus. Rolle und Auftrag kommen aus der
Startanweisung; fehlende notwendige Angaben klärst du vor der Ausführung.

1. Lies die [gemeinsamen Ablaufregeln](references/workflow.md). Sie legen Topics,
   Mandat, Protokoll, Tickets, lokalen Zustand und Stopp fest.
2. Verwende den bereitgestellten [MCP-Zugang](references/mcp.md). Nur ohne
   bereitgestellten MCP-Zugang nutze die [CLI](references/cli.md). Verwende
   vorhandene Topics und deine Identität; keine Topic-/ACL-Administration.
3. Wähle einen freien Agentennamen oder nutze den vorgegebenen und stelle deine
   Fähigkeiten im Agententopic vor. Der Operator übermittelt den relevanten Stand.
4. Prüfe Mandat und vereinbartes Protokoll, bestätige deine Auftragsannahme
   ausdrücklich und führe die Arbeit aus. Beantworte Statusabfragen im vereinbarten
   Schema. Auftragsannahme und Ablehnung sind eigene fachliche Nachrichten.
5. Melde Ergebnis und offene Punkte. Anforderungen, Status und Abschluss dürfen
   ohne zusätzlichen Text als vereinbarte Ereignisse erfolgen. Wiederhole bekannte
   Inhalte nicht; umfangreiche Details nur auf Nachfrage.

Halte den Arbeitsstand lokal dauerhaft fest. Replay oder Neustart wiederholen
keine Fachaktion automatisch; menschliche Stopps und Risikopausen beachten.

Die Rolle wurde in zwei lokalen Demos verwendet; das ist keine Garantie für
Modellzuverlässigkeit. Noch ausstehende Client-Unterstützung steht im
[Umsetzungsstand](references/open-decisions.md).
