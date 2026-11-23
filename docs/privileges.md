# Betriebssystemrechte beim Start

Das Binary legt unter Linux erhöhte Unix-Rechte vor dem Erzeugen der Tokio-Runtime ab. Auf macOS verweigert es privilegierte Starts und verlangt einen bereits unprivilegierten Launcher. Vorher werden nur die globalen UID/GID-Argumente gelesen und validiert. Konfiguration, TLS- und Client-Schlüssel, Adapterregeln sowie Daten- und Admin-Sockets werden erst mit der endgültigen Identität geöffnet. Das gilt für alle CLI-Kommandos, einschließlich `admin`, `keygen` und `check-config`.

Normalerweise startet mynofy direkt unter einem gewöhnlichen Benutzer beziehungsweise einem dedizierten Dienstkonto:

```sh
mynofy serve --config /etc/mynofy/config.json
```

Wenn der Launcher unter Linux Root-Rechte mitgibt, sind beide numerischen Zielwerte zwingend:

```sh
mynofy --run-as-uid 1001 --run-as-gid 1001 serve --config /etc/mynofy/config.json
```

1001 ist hier nur ein Beispiel für das vom Betreiber eingerichtete Dienstkonto. UID/GID 0, der reservierte Wert 4294967295, unvollständige und doppelte Angaben werden abgewiesen. Es gibt weder eine automatische Auswahl von `nobody` noch eine Ableitung aus `SUDO_UID` oder eine Option zum Weiterlaufen als Root. Die Optionen können auch nach dem Kommando stehen. Ein Abbruch der Rechteumstellung beendet das Programm vor seinem normalen Start; kein stiller Fallback.

## Reihenfolge und Wirkung

Bei einem Root-Start werden zusätzliche Gruppen entfernt und anschließend primäre GID und UID dauerhaft umgestellt. Unter Linux werden jeweils reale, effektive und gespeicherte IDs auf das Ziel gesetzt, auch beim bereits unprivilegierten Start. Die Umstellung erfolgt im einzigen Anwendungsthread; erst danach entstehen Worker und Hintergrundaufgaben. Auf macOS wird ein privilegierter Start auch mit Zielangaben abgewiesen. Der aktuelle sichere OS-Wrapper `nix` schließt Apples Gruppen-APIs aus; ein unvollständiger Wechsel mit geerbten Root-Gruppen ist keine Alternative. Bereits unprivilegierte Starts und Zielangaben für dieselbe UID/GID bleiben möglich; gemischte reale/effektive Identitäten werden abgewiesen. Ein vorgeschalteter Launcher muss eine reguläre Benutzeridentität oder Root übergeben; eigene Setuid-/Setgid-Wrapper sind kein unterstütztes Betriebsmodell.

Linux entfernt zusätzlich effektive, erlaubte, vererbbare und Ambient-Capabilities, deaktiviert `keep capabilities` und setzt unwiderruflich `no_new_privs`. Mit vorhandener `CAP_SETPCAP` wird auch das Capability-Bounding-Set geleert. Ein gewöhnlicher Benutzer kann sein geerbtes Bounding-Set üblicherweise nicht verändern; leere aktive Capability-Sets und `no_new_privs` verhindern trotzdem zusätzliche Rechte durch Setuid-/Setgid-Programme oder File-Capabilities bei `exec`. Vom CLI-Adapter gestartete Programme erben diese Einschränkung.

Ein bereits gewöhnlicher Benutzer behält seine normalen zusätzlichen Gruppen, falls er sie nicht entfernen darf. Unter Linux wird Gruppe 0 dabei nicht akzeptiert. Auf macOS verwaltet der Launcher die zusätzlichen Gruppen. Ein Identitätswechsel, dessen zusätzliche Gruppen sich nicht sicher entfernen lassen, wird abgewiesen. Für einen Server empfiehlt sich ein eigenes Dienstkonto mit gezielt gesetzten Gruppen. Linux-spezifische Capability-/`no_new_privs`-Garantien gelten nicht auf macOS. Unter anderen Nicht-Unix-Plattformen sind UID/GID-Optionen nicht verfügbar; dort ist kein OS-Rechteabbau implementiert.

## Dateien, Netzwerk und Admin

Der Zielbenutzer muss die Konfiguration und Schlüssel lesen, den Socket-Elternpfad verwenden und den gewünschten Netzwerkport binden dürfen. mynofy liest keine Dateien als Root vor, behält keine Capability für niedrige Ports und ändert keine Dateibesitzer automatisch. Bestehende Socket-Dateien werden weiterhin nicht überschrieben. Neue Unix-Sockets gehören unmittelbar der Ziel-UID/GID; der getrennte Admin-Socket prüft diese UID als Peer. Ein Admin-CLI aus einer Root-Shell braucht deshalb ebenfalls die Zielangaben.

Bei systemd beziehungsweise Containern ist bereits der Start mit `User=`/`Group=` beziehungsweise einer passenden Container-UID/GID vorzuziehen. Das In-Process-Bootstrap schützt zusätzlich den versehentlichen privilegierten Aufruf. Die einbindbaren Server-/Client-Bibliotheken ändern keine Prozessidentität ihres Hosts; deren Launcher trägt diese Verantwortung.

## Prüfung

`cargo test --locked --workspace` umfasst Argument-Unit-Tests und Linux-Prozesstests. Die Linux-Tests kontrollieren alle Runtime-Threads über `/proc`: reale/effektive/gespeicherte/Dateisystem-IDs, Capability-Sets und `no_new_privs`; außerdem Socket-Eigentum und Admin-Zugriff. Linux-Root-Tests prüfen den Abbruch vor Datei-I/O und den tatsächlichen Wechsel auf eine andere UID/GID. Der Adaptertest kontrolliert die Vererbung und abgewiesene Rückkehr zu UID/GID 0 im gestarteten Programm.

Vorhandene CLI-Integrationsszenarien werden bei einem Root-Testlauf mit Python unter UID/GID 65534 erneut ausgeführt, statt die produktive Startprüfung zu umgehen. Der Test-Runner selbst wechselt keine Identität. Ein Root-Testlauf braucht die dafür erforderlichen `setuid`/`setgid`-Rechte und für diese Testprozesse lesbare/ausführbare Build-Pfade. Native macOS-Laufzeitprüfungen müssen auf macOS erfolgen.
