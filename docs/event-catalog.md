# Eingebauter Ereigniskatalog: mynofy, Version 1

Erster Entwurf zur gemeinsamen Prüfung. Die Kategorien, Aktionen und Statuswerte stammen aus `logging-engine/morkhet_logging_engine/src/provider_strings.rs`; die numerischen IDs stammen aus `types.rs`. Quellprüfsummen stehen in der [maschinenlesbaren Definition](../protocol/catalogs/mynofy-v1.json). `logging-engine` bleibt unverändert und ist keine Build- oder Laufzeitabhängigkeit.

Der Katalog ist über `get_catalog` abrufbar und besitzt die feste numerische Katalog-ID `1`. Protobuf und die zwei Payload-Varianten sind implementiert: typisierte Ereignisse sowie freie Bytes mit hart maximal 4096 Byte. Generierte Enums und numerische Mitgliedschaftsprüfungen vermeiden einen Katalogscan pro Ereignis. Fachliche Signaturen/Profiles und automatischer Katalogabgleich bleiben Entwurf.

## Beschlossenes Zielmodell

- Ein eingebauter Katalog mit Name `mynofy` und Version `1`; kompakte numerische Referenzen in den Ereignisnachrichten. Die numerische Katalogkennung und das konkrete Handshake-Format bleiben vor der Wire-Umsetzung festzulegen.
- Eingebaute Definitionen beim Build in Enums/statische Tabellen überführen. Kein Parsen, Nachladen oder Registrieren von Katalogdefinitionen im Nachrichtenpfad.
- Optionale eigene Kataloge sind eine spätere Erweiterung: vollständig beim Start laden und validieren, danach unveränderlich halten. Keine dynamische Katalogübertragung durch Messages.
- Typisierte Ereignisse: Kategorie, Aktion, Status und bis zu drei primitive Werte, zusammen höchstens 24 Byte reine Werte. Typ-/Feldkennungen und Nachrichtenmetadaten kommen hinzu; dies ist keine Aussage über die vollständige Wire-Größe.
- Freie Payload: höchstens 4096 tatsächliche Bytes. Kein großer Puffer in jedem kleinen Ereignis und keine automatische Aufteilung.
- Fachliche Ereignisse besitzen dieselbe Nachrichtenhülle und dieselben ACL-/Ablaufregeln wie freie Bytes. Ein fachlicher `SUCCESS` ist kein Transport-ACK und umgekehrt.

## Übernommene Werte

ID `0` ist in allen drei Namensräumen reserviert und ungültig. Insbesondere ist `UNSPECIFIED` ein gültiger Status mit ID `1`. Die numerischen Lücken aus der Vorlage bleiben bestehen; keine automatische Neunummerierung. Diese Zahlen bezeichnen den Katalog und entscheiden noch nicht über dessen Protobuf-Darstellung.

### Kategorien

43 Einträge.

| ID | Name |
| ---: | --- |
| 1 | `CORE` |
| 2 | `ENGINE` |
| 3 | `SYSTEM` |
| 10 | `MEMORY` |
| 11 | `THREAD` |
| 12 | `JOB` |
| 13 | `SCHEDULER` |
| 20 | `FILESYSTEM` |
| 21 | `STORAGE` |
| 22 | `DATABASE` |
| 23 | `CACHE` |
| 30 | `NETWORK` |
| 31 | `RPC` |
| 32 | `HTTP` |
| 40 | `LOCK` |
| 41 | `MUTEX` |
| 42 | `ATOMIC` |
| 43 | `ASSET` |
| 44 | `RESOURCE` |
| 45 | `BUNDLE` |
| 46 | `SERIALIZATION` |
| 47 | `DESERIALIZATION` |
| 48 | `RENDER` |
| 49 | `SHADER` |
| 50 | `TEXTURE` |
| 51 | `GPU` |
| 52 | `EVENT` |
| 53 | `MESSAGE` |
| 54 | `AUDIO` |
| 55 | `INPUT` |
| 56 | `ANIMATION` |
| 57 | `SCENE` |
| 58 | `ADDRESSABLES` |
| 59 | `STREAMING` |
| 100 | `GAMEPLAY` |
| 101 | `PLAYER` |
| 102 | `AI` |
| 103 | `PHYSICS` |
| 300 | `LOG` |
| 301 | `METRICS` |
| 302 | `TELEMETRY` |
| 303 | `DEBUG` |
| 304 | `SECURITY` |

### Aktionen

49 Einträge.

| ID | Name |
| ---: | --- |
| 1 | `CREATE` |
| 2 | `DESTROY` |
| 3 | `LOAD` |
| 4 | `SAVE` |
| 10 | `START` |
| 11 | `STOP` |
| 12 | `INIT` |
| 13 | `SHUTDOWN` |
| 14 | `RESET` |
| 20 | `READ` |
| 21 | `WRITE` |
| 22 | `UPDATE` |
| 23 | `DELETE` |
| 30 | `COMPARE` |
| 31 | `MATCH` |
| 32 | `MISMATCH` |
| 33 | `VALIDATE` |
| 40 | `EXECUTE` |
| 41 | `SCHEDULE` |
| 42 | `COMPLETE` |
| 43 | `CANCEL` |
| 50 | `CONNECT` |
| 51 | `DISCONNECT` |
| 52 | `SEND` |
| 53 | `RECEIVE` |
| 60 | `ALLOCATE` |
| 61 | `FREE` |
| 62 | `MAP` |
| 63 | `UNMAP` |
| 70 | `LOCK` |
| 71 | `UNLOCK` |
| 72 | `ACQUIRE` |
| 73 | `RELEASE` |
| 80 | `QUEUE` |
| 81 | `DEQUEUE` |
| 82 | `WAKE` |
| 83 | `SLEEP` |
| 90 | `REGISTER` |
| 91 | `UNREGISTER` |
| 92 | `ATTACH` |
| 93 | `DETACH` |
| 100 | `OPEN` |
| 101 | `CLOSE` |
| 102 | `FLUSH` |
| 103 | `SYNC` |
| 110 | `COMMIT` |
| 111 | `ROLLBACK` |
| 120 | `QUERY` |
| 121 | `LOOKUP` |

### Status

21 Einträge.

| ID | Name |
| ---: | --- |
| 1 | `UNSPECIFIED` |
| 2 | `SUCCESS` |
| 3 | `FAILED` |
| 4 | `TIMEOUT` |
| 5 | `CANCELLED` |
| 6 | `RETRY` |
| 7 | `PENDING` |
| 8 | `PARTIAL` |
| 9 | `INVALID_ARGUMENT` |
| 10 | `NOT_FOUND` |
| 11 | `ALREADY_EXISTS` |
| 12 | `BUSY` |
| 13 | `LOCKED` |
| 14 | `CONFLICT` |
| 15 | `UNAVAILABLE` |
| 16 | `RESOURCE_EXHAUSTED` |
| 17 | `PERMISSION_DENIED` |
| 18 | `NOT_IMPLEMENTED` |
| 19 | `SKIPPED` |
| 20 | `DEFERRED` |
| 21 | `CORRUPTED` |

### Parameter-Grundtypen

| ID | Typ | Reine Wertebytes |
| ---: | --- | ---: |
| 1 | `U32` | 4 |
| 2 | `U64` | 8 |
| 3 | `I32` | 4 |
| 4 | `I64` | 8 |
| 5 | `F32` | 4 |
| 6 | `F64` | 8 |
| 7 | `BOOL` | 1 |

Die Typen stammen ebenfalls aus `types.rs`. Dessen native Byte-Reihenfolge, Speicherlayout, Layout-IDs und generische Flagbits werden nicht als Netzwerkformat übernommen. Eigene Flags sind bislang ohne festgelegte Bedeutung und bleiben außerhalb dieser Basis. Ebenso wird Logging-Severity nicht stillschweigend zu einer mynofy-Pflichtangabe.

## Prüfung anhand von Anwendungsfällen

Die [Produktprüfung mit 50 Anwendungsfällen](use-cases.md) ordnet den gesamten Katalog ein. `UNITY`, `UNITY_OBJECT`, `UNITY_SCENE` und `UNITY_ASSET` wurden aus dem eingebauten Entwurf entfernt. Ihre IDs 200–203 bleiben unbesetzt und dürfen nicht für andere Begriffe wiederverwendet werden. Die Quellprüfsummen bezeichnen weiterhin die ursprüngliche Importbasis, nicht einen unveränderten aktuellen Katalog.

Der Entwurf enthält jetzt 43 Kategorien, 49 Aktionen und 21 Statuswerte. Weitere domänenspezifische Kategorien sind im Prüfbericht als Kandidaten für spätere optionale Kataloge markiert; sie wurden noch nicht entfernt. Neue Begriffe und Signaturen im Bericht sind Vorschläge, keine bereits verfügbaren Katalogwerte.

Die typisierten Payloads prüfen noch keine fachlichen Ereignissignaturen. Für ausführbare Profile sind festzulegen: erlaubte Kombination, Bedeutung, Typ und Reihenfolge der Parameter sowie Versionierungsregeln. Fachliche Status lösen keine automatische Wiederholung oder Broker-Zustandsänderung aus. Das Protobuf-Format verwendet separate numerische Felder; kein natives EventKey-Speicherlayout.

## Vom Client abrufen

Nach erfolgreicher Anmeldung kann jeder Client die eingebaute Definition abfragen, auch ohne Topic-Rechte:

```json
{"version":1,"type":"get_catalog","id":"schema-1","name":"mynofy","catalog_version":1}
```

Die Antwort ist ein `catalog`-Frame mit `request_id` und dem vollständigen Objekt unter `catalog`, einschließlich Kategorien, Aktionen, Status, Parametertypen und vorgesehenen Limits. Das [vollständige Beispiel](../protocol/v1/catalog-examples.json) und das [Metadatenschema](../protocol/catalogs/catalog.schema.json) beschreiben das Format. Der CLI-Client akzeptiert die Abfrage wie andere JSONL-Commands; die Rust-Client-Bibliothek bietet `get_catalog(&mut socket, "mynofy", 1).await` und liefert das Katalogobjekt zurück.

Unbekannter Name oder nicht verfügbare Version ergeben `not_found`; fehlende Anmeldung ergibt `unauthorized`. Bestehende Client-Rate-Limits und Antwort-Byte-Limits gelten auch hier. Der Abruf verändert keinen Zustand und richtet keinen Katalog ein. Clients können die Antwort pro Owner/Name/Version zwischenspeichern; solange Version 1 ein Entwurf ist, nach einem Service-Update erneut abrufen.

Die eingebundene JSON-Definition wird pro Prozess einmal geparst und auf gültige, eindeutige IDs/Namen geprüft, beim Engine-Start vor Annahme von Commands. Mehrere Engines teilen dieselbe unveränderliche Definition. Einzelabfragen kopieren die vorbereiteten Metadaten in ihre begrenzte Antwort; keine erneute Interpretation der Katalogdatei und keine Datei-I/O im Request-Pfad. Für Ereignisse sind generierte Enums und statische numerische Prüfungen bereits implementiert.

## Einheitliches Aktions-/Statusmodell

[Workflow-Semantik](workflow-semantics.md) legt die Bedeutung vorhandener Tripel als Grundlage fest: `(JOB, START, PENDING)` = Start steht aus, `(JOB, START, SUCCESS)` = gestartet, `(JOB, ACQUIRE, SUCCESS)` = übernommen, `(JOB, COMPLETE, SUCCESS)` = abgeschlossen. Zusätzliche Status RUNNING/ACCEPTED oder ein REQUEST/EVENT-Feld sind für dieses Grundmodell nicht vorgesehen. Die [50 UCs als Daten](../protocol/catalogs/use-cases-v1.json) referenzieren nur vorhandene Kennungen. Neue Wire-Felder oder ein installierter AI-Skill entstehen dadurch nicht.

Der Katalog ist außerdem die Basis für [laufbezogene Vereinbarungen zwischen Agenten und Menschen](agent-coordination.md). Die Aushandlung eigener Anwendungsprotokolle installiert keine zusätzlichen Kataloge im Dienst.
