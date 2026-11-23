# Kontrollierter Schlüsselwechsel

Entwurf gemäß der Projektentscheidung vom 11. November 2026. Der clientseitig gestartete Ablauf ist im lokalen Prototyp implementiert; die konkreten [Frames und kryptografischen Bindungen](protocol.md) sind dokumentiert. Nur der Client initiiert den Wechsel; serverseitige Offers sind nicht vorgesehen. Dies ist ein Wechsel des Client-Tokens auf Anwendungsebene, keine TLS-Renegotiation und kein TLS-Traffic-Key-Update.

## Identität und Gültigkeit

Die stabile Client-ID wird beim ersten Einrichten aus dem ersten Public Key abgeleitet: `client:` gefolgt von dessen `token_id`. Sie bleibt bei allen späteren autorisierten Schlüsselwechseln gleich. Der aktuelle Token-Fingerprint identifiziert dagegen genau einen aktuellen Schlüssel. Anzeigenamen wie `codex` sind keine Eigentumskennung und können nicht durch Neuanlegen eine alte Identität übernehmen.

Registrierung und Wechsel gelten pro Owner. Ein Wechsel bei einem Owner aktualisiert keine anderen Owner. Der Client muss die zugehörigen aktuellen privaten Schlüssel pro Owner verwalten. Eine globale Synchronisierung ist nicht vorgesehen. Die konzeptionellen Regeln des Wechsels sind festgelegt; die unten genannten konkreten Frames und kryptografischen Parameter sind Implementierungsaufgaben.

Der Service hält die Zuordnung von stabiler Identität, aktuellem Schlüssel, Gültigkeitszeitraum und Sperrstatus im RAM. Beim Prozessstart wird die Baseline aus der [Konfigurationsdatei](configuration.md) geladen und der aktuelle [Security-Stand](security-journal.md) wiederhergestellt. Laufzeit-Wechsel und Sperren werden automatisch asynchron gespeichert. Der Client benötigt entsprechende Gültigkeitsmetadaten. Ein roher Ed25519-Public-Key enthält selbst weder Ablaufdatum noch Widerrufsinformation; diese stammen aus der Registrierung. Ein TLS-Zertifikat hat einen eigenen, davon getrennten Lebenszyklus.

Ein Wechsel ist nur zulässig, solange der bestehende Token noch gültig und nicht gesperrt ist und die Verbindung damit authentifiziert wurde. Ist der Token gesperrt oder bereits abgelaufen, muss er über einen vertrauenswürdigen Verwaltungsweg neu eingerichtet werden. Ein abgelaufenes oder widerrufenes, zur Verbindung benötigtes Zertifikat darf ebenfalls nicht durch diesen Ablauf umgangen werden. Eine manuelle Wiederanbindung an eine alte stabile Identität ist eine ausdrückliche Betreiberentscheidung, kein Recht aus dem neuen Public Key.

## Vorgesehener Ablauf mit Shared Secret

1. Der Client erkennt das bevorstehende Ablaufdatum und fragt auf der bestehenden authentifizierten WSS-Verbindung an, ob der Schlüsselwechsel beginnen soll.
2. Die Gegenstelle bestätigt. Pro Identität darf nur ein Wechsel gleichzeitig ausstehen. Beide Seiten kennen eine eindeutige Wechsel-ID und eine kurze Frist.
3. Der Initiator erzeugt ein kryptografisch zufälliges, einmaliges Shared Secret und überträgt es ausschließlich über diese authentifizierte, verschlüsselte Verbindung. Es erscheint nicht in URLs, Logs, Topic-Nachrichten oder allgemeinen Statusabfragen.
4. Der Client erzeugt ein neues Schlüsselpaar. Er behält den alten Schlüssel zunächst lokal und verschlüsselt den neuen Public Key mit authentifizierter Verschlüsselung unter dem Shared Secret. Stabile Client-ID, Service-ID, Wechsel-ID und alter Token-Fingerprint müssen gegen Austausch gebunden werden.
5. Der Service entschlüsselt und prüft den Public Key. Vor Übernahme muss zusätzlich der Besitz des neuen privaten Schlüssels nachgewiesen werden, etwa mit einer neuen signierten Challenge. Die Verschlüsselung des Public Keys allein beweist diesen Besitz nicht.
6. Der Service prüft unmittelbar vor dem Commit erneut Gültigkeit und Sperrstatus des alten Tokens sowie die unveränderte Identitätszuordnung. Der neue Schlüssel wird atomar derselben stabilen Client-ID zugeordnet; der alte Schlüssel wird ersetzt. Eigentum und vorhandene ACL bleiben bestehen. Der Wechsel erweitert keine Rechte.
7. Der Service bestätigt den Commit. Der Client prüft die neue Anmeldung und verwirft den alten Schlüssel erst, nachdem der Erfolg eindeutig festgestellt wurde. Shared Secret und ausstehender Wechselzustand werden nach Erfolg, Abbruch oder Fristende verworfen.

Rechteentzug, Token-Widerruf und Ablauf haben Vorrang vor einem noch nicht abgeschlossenen Wechsel. Ein eingeleiteter Wechsel darf die Gültigkeit nicht selbst verlängern. Ausstehende normale Auth-Challenges, die einen ersetzten oder entzogenen Schlüssel betreffen, werden ungültig; Annahme des Proofs und Bindung der Sitzung müssen gegen die aktuelle Konfiguration atomar geprüft werden.

## Fristende und Neustart des Vorgangs

Jeder Vorgang hat eine feste Frist aus der Konfiguration. Wird er bis dahin nicht abgeschlossen, verfallen Shared Secret, reservierter neuer Schlüssel und ausstehende Challenges dieses Vorgangs. Ein neuer Versuch startet vollständig mit neuer Wechsel-ID und neuem Shared Secret. Ein alter Vorgang wird nicht nach Fristende fortgesetzt. Es gibt pro stabiler Client-ID ein Rate-Limit für gestartete Wechsel, gemeinsam über alle Verbindungen; ein Timeout setzt dieses Budget nicht zurück.

Vor dem atomaren Commit bleibt der bisherige Schlüssel maßgeblich. Nach einem erfolgreichen Commit ist der neue Schlüssel maßgeblich, auch wenn die Antwort verloren ging. Ein Fristablauf rollt einen bereits übernommenen Schlüssel nicht zurück. Der Client behält deshalb den neuen privaten Schlüssel und kann dessen Anmeldung versuchen. Scheitert ein unvollständiger Ablauf, startet er mit dem noch gültigen akzeptierten Schlüssel neu; ist keiner mehr gültig oder vorhanden, ist eine neue Einrichtung nötig. Es wird kein automatischer Rückfall auf einen widerrufenen oder abgelaufenen Schlüssel erlaubt.

On restart, the service restores the synchronized security journal. With `security_storage.sync: true`, `rotation_complete` confirms that the new key was synchronized. Durable confirmation is the default. Explicit `security_storage.sync: false` can lose unsynced changes on a crash. A lost commit response or failed synchronization leaves the outcome unknown; retain both keys and reconcile without automatically replaying the commit.

Rechteentzug, Widerruf und Ablauf haben Vorrang vor einem noch offenen Commit. Die Prüfung und der Commit erfolgen serialisiert gegen die aktuelle Konfigurationsgeneration. Ein Limit schützt nicht vor jedem verteilten Angriff; zusätzliche Listener-/Quellenlimits greifen bereits vor erfolgreicher Authentifizierung.

## Umsetzung des lokalen Prototyps

Der Client initiiert `rotation_start`; das bestätigende Offer enthält Frist und neue Owner-Gültigkeit. Erst danach erzeugt der Client das Shared Secret und sendet es mit `rotation_confirm`. Der Service antwortet mit Nonce und Proof-Challenge, ohne das Secret zurückzusenden. `rotation_commit` transportiert den AES-256-GCM-geschützten Public Key und den Besitznachweis des neuen privaten Schlüssels. Die Ersetzung ist ein atomarer Core-Schritt ohne Netzwerk-I/O innerhalb des Commits.

Shared Secrets werden im Serverzustand durch `zeroize` beim Verwerfen gelöscht. Der CLI-Client hält alte und neue private Dateien getrennt; ein neuer Schlüssel überschreibt keinen bestehenden Pfad. Der aktuelle lokale Prototyp unterstützt manuell clientseitig angestoßene Rotation. Ein serverseitiger Anstoß ist nicht vorgesehen. Automatische Ablaufwarnungen und ein dauerhafter Wechselzustand sind nicht implementiert. Eine unabhängig neu provisionierte Identität wird aus ihrem ersten Public Key abgeleitet und übernimmt keine vorhandenen ACLs oder Inhalte. Reguläre clientinitiierte Rotation erhält dagegen die stabile Client-ID.

Ein Public Key ist nicht geheim. Das zusätzliche Shared Secret kann den Wechsel an einen vereinbarten Vorgang binden, schützt aber ohne den bereits authentifizierten TLS-Kanal keinen Schlüsselaustausch. Die Berechtigung zum Wechsel stammt aus der gültigen alten Identität und der Bestätigung, nicht aus bloßer Kenntnis eines vom Client selbst mitgebrachten Secrets.
