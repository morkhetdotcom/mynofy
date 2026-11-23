# CLI-Ersatzweg für den Entwurf

Verwende diesen Weg ausschließlich, wenn keine MCP-Anbindung bereitgestellt ist,
nicht zur automatischen Wiederholung fehlgeschlagener MCP-Aufrufe.
Verwende ein bereitgestelltes `mynofy`-Binary. Diese Beispiele zeigen vorhandene
Schnittstellen; tatsächliche Owner, Topic-Namen und Pfade kommen aus dem Auftrag.
Private Schlüssel werden als Dateipfad angegeben, nicht im Topic veröffentlicht.

Direkt über einen geschützten lokalen Socket:

```sh
mynofy client --socket /provided/mynofy.sock --key /provided/agent.pk8 --owner owner
```

Direkt über TLS/WebSocket:

```sh
mynofy client --url wss://provided-host:8092 --ca /provided/ca.crt --key /provided/agent.pk8 --owner owner
```

Über einen bereitgestellten Einstiegspunkt zum Remote-Owner:

```sh
mynofy client --url wss://provided-proxy:8090 --ca /provided/proxy-ca.crt --key /provided/agent.pk8 --owner owner --via-topic project/work --owner-ca /provided/owner-ca.crt
```

Bei abweichendem Owner-Zertifikatsnamen `--owner-name` verwenden. Nach
Schlüsselrotation kann `--client-id` die bereits bestehende stabile Identität
angeben. Keine eigene oder ungeprüfte CA-/Owner-Zuordnung erfinden.

Die CLI liest JSONL von stdin und liefert die jeweilige Antwort als JSONL auf
stdout. Sie erzeugt keine Poll-Schleife und keine regelmäßigen Bekanntgaben von
selbst. Ein ausführender Agent beziehungsweise ein passender Client-Treiber muss
Intervalle und Fristen steuern; ein solcher Skill-Helfer ist noch nicht gebaut.

Beispiele für einzelne Anfragen auf einer authentifizierten Verbindung:

```json
{"version":1,"type":"get_catalog","id":"catalog","name":"mynofy","catalog_version":1}
{"version":1,"type":"get_permissions","id":"permissions","topic":"project/work"}
{"version":1,"type":"poll","id":"poll","topic":"project/work","limit":20}
```

Beim nächsten Poll desselben Topics den gelieferten `next_cursor` als `after`
verwenden. `has_more` und Seiten-/Byte-Limits beachten. Cursor pro Owner und Topic
getrennt halten. Menschen-, Operator- und Agententopic werden unabhängig gelesen,
ohne gemeinsame atomare Reihenfolge.

The broker has no ACK/NACK commands or delivery IDs. Publish acceptance, rejection and completion as separate messages using the negotiated application protocol. Store read progress locally; losing a cursor can replay active messages.

Publish verwendet bestehende Payload-Varianten: bekannte typisierte Ereignisse
oder freie Inhalte bis 4096 tatsächliche Bytes. Die CLI unterstützt eine
JSON-Kompatibilitätsdarstellung; ein eigenes Ticket-/Nachrichten-Wireformat wird
hier nicht neu definiert. Große Artefakte gehören nicht in die Nachricht.

`--progress FILE` speichert unter Unix Poll-Fortschritt und Owner-Neustartwissen.
Es speichert keine vollständige Ticketbearbeitung, verhindert nicht allein doppelte
Fachaktionen und ist kein globales Stoppregister. Nach bekanntem Owner-Neustart
können noch aktive Nachrichten erneut gelesen werden. Der vorhandene Binary-Adapter
bestätigt Trigger vor der Ausführung und ist kein Ersatz für einen dauerhaften
Agenten-Arbeitsstand.
