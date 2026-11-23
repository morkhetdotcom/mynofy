# Primärer MCP-Zugang

Verwende die bereitgestellten mynofy-MCP-Tools. Hostabhängige Präfixe können vor
den hier genannten Tool-Namen stehen. MCP-Transport und Protokollhandshake
übernimmt der Host; sende nicht selbst JSON-RPC über Nachrichten-Topics.

Wenn keine MCP-Anbindung bereitgestellt ist, nutze den [CLI-Ersatzweg](cli.md).
Ein vorhandener MCP-Zugang mit einem Fehler ist kein Anlass für einen
automatischen Schnittstellenwechsel oder einen zweiten Publish.

## Einstieg und verfügbare Operationen

`mynofy_connection_info` liefert unter `structuredContent` Owner, stabile
Client-ID und Neustartzähler. Vergleiche diese Zuordnung mit dem vorgesehenen
Arbeitskontext. Die Daten einer bereits geladenen Sitzung sind kein eigener
Liveness-Nachweis. Zugangsdaten und Owner-TLS-Prüfung sind im MCP-Prozess
konfiguriert; übergib weder Schlüssel noch eine frei behauptete Client-ID als
Tool-Argumente.

| Zweck | Tool und Argumente |
| --- | --- |
| Bekannten Katalog abrufen | `mynofy_get_catalog`, optional `name`, `catalog_version` |
| Eigene Topic-Rechte prüfen | `mynofy_get_permissions` mit `topic` |
| Einmalig publizieren | `mynofy_publish` mit `topic`, `payload`, optional `expires.time` |
| Eine Seite lesen | `mynofy_poll` mit `topic`, optional `after`, `limit` |
| Eigene aktive Nachrichten auflisten | `mynofy_list_messages`, optional `topic`, `limit`, `cursor` |
| Eigene ursprüngliche Nachricht lesen | `mynofy_get_message` mit `message_id` |

Broker-Antworten stehen unter `structuredContent.response`; Owner, Client-ID und
Neustartzähler daneben. Falls der Host nur Textcontent liefert, ist derselbe
Umschlag dort als JSON enthalten. Ein `isError`-Ergebnis ist keine erfolgreiche
fachliche Aktion. Brokerfehler und Rate-Limits auswerten, nicht durch andere
Tools oder erfundene Rechte umgehen. Es gibt keine Admin-Tools.

## Polling und exakte Referenzen

Poll sofort eine begrenzte Seite; plane weitere Abfragen gemäß vereinbartem
Intervall, standardmäßig 30 Sekunden. Der MCP-Adapter pollt nicht selbstständig.
Höchstens eine Owner-Operation pro Prozess ist gleichzeitig möglich; fachliche
Calls daher nacheinander ausführen. `busy` bedeutet, dass kein zweiter Aufruf
eingereiht wurde. Es entsteht dadurch kein neuer Arbeitsversuch.

Halte den gelieferten `next_cursor` je Owner und Topic fest und verwende ihn beim
nächsten Poll als `after`. Cursor ersetzen keinen gespeicherten Ticketfortschritt. Der Adapter verlängert keine Interpretation; diese Aufgabe bleibt beim Agenten/Treiber.

The broker has no ACK/NACK tools or delivery IDs. Report business acceptance and rejection by publishing an agreed event. Typed event parameters
`u64`/`i64` können ebenfalls als exakte Dezimalstrings geliefert und bei Publish
verwendet werden. Das ändert nicht die Ticket-ID oder das Broker-Protokoll.

## Fehler, Cancellation und Wiederverbindung

`outcome_unknown: true` bedeutet, dass eine Mutation bereits beim Owner angekommen
sein kann. Bei verlorener Publish-Antwort weder denselben MCP-Aufruf blind
wiederholen noch die CLI zum erneuten Senden verwenden. Prüfe eigene aktive
Nachrichten, gegebenenfalls deren Inhalte und den fachlichen Arbeitsstand.
Fehlende oder abgelaufene Nachrichten beweisen keine Nichtausführung.

Nach einem Transportfehler kann ein späterer expliziter Call neu verbinden.
Prüfe dann Owner-/Client-Zuordnung und Neustartzähler; nach bekanntem
Owner-Neustart den Cursor entsprechend den Neustartregeln verwerfen und erneut
abgleichen, ohne bekannte Fachschritte nochmals auszuführen. Ein MCP-Abbruch
kann bereits erfolgte Broker- oder Fachwirkungen nicht rückgängig machen.

Die Regeln zu Rollen, menschlichen Aufträgen, Tickets, Interpretation und Stopp
stehen in [Ablaufregeln](workflow.md). MCP erteilt dafür keine zusätzlichen Rechte
und implementiert keinen eigenen Workflow- oder dauerhaften Ticketzustand.
