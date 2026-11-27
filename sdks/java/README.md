# mynofy Java-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Eigenständiger Client ab Java 17 für direkte WSS- und lokale Unix-Verbindungen.
Vorhandenes Protobuf v1, ohne CLI-Unterprozesse oder Server-Core-Abhängigkeit.
API und Protokoll sind noch veränderliche Prototypen.

## Einbinden

Das Maven-Artefakt ist noch nicht veröffentlicht. Lokal installieren:

```sh
mvn -f sdks/java/pom.xml -DskipTests install
```

Im eigenen Maven-Projekt:

```xml
<dependency>
  <groupId>dev.mynofy</groupId>
  <artifactId>mynofy-client</artifactId>
  <version>0.3.0-SNAPSHOT</version>
</dependency>
```

Zwei direkte Laufzeitabhängigkeiten: offizielle
[Protobuf-Runtime](https://protobuf.dev/reference/java/java-generated/) und
[Java-WebSocket](https://github.com/TooTallNate/Java-WebSocket), dazu dessen
transitive SLF4J-API. Das SDK installiert keinen Logging-Provider. TLS, Ed25519
und AES-GCM verwenden ausschließlich die JDK-Implementierungen. Kein gRPC,
JNI-Zusatzpaket, JSON-Parser oder Protobuf-Compiler zur Laufzeit. Die generierten
Typen liegen im JAR unter `dev.mynofy.protocol.Wire`.

## Lokal senden und lesen

Public Key und Topic-Rechte müssen beim Owner registriert sein. `mynofy keygen`
erzeugt direkt kompatible Schlüssel. Derselbe normale OS-Benutzer wie beim Service:

```java
import dev.mynofy.Client;
import dev.mynofy.Key;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.charset.StandardCharsets;

class LocalExample {
    public static void main(String[] args) throws Exception {
        var key = Key.parse(Files.readAllBytes(Path.of(".local/unix/client.pk8")));
        var config = Client.Config.unix(
            Path.of(".local/unix/run/mynofy.sock"), "local", key);
        try (var client = Client.connect(config)) {
            System.out.println(client.info());
            var sent = client.publish("local/events",
                Client.bytes("Hallo".getBytes(StandardCharsets.UTF_8), "text/plain"), null);
            var page = client.poll("local/events", null, 20);
            for (var item : page.getItemsList()) {
                System.out.println(item.getMessage());
                // Nach Verstehen/Zuordnen; kein fachlicher Abschluss:
            }
            System.out.println(sent.getMessageId() + " " + page.getNextCursor());
        }
    }
}
```

`poll` liest sofort eine Seite; bei `has_more` weitere Seiten mit `next_cursor`
abholen. Cursor erst nach Verarbeitung oder dauerhafter eigener Übernahme speichern.
`null` liest den aktiven Bestand erneut. Nach bekanntem Neustart anhand von
`info().restartCounter()` neu ohne Cursor lesen und mit lokalem Arbeitsstand
abgleichen. Das SDK speichert keinen Fortschritt und bestätigt nicht automatisch.
Poll reads without recording reader state; the client keeps its progress locally.

## WSS und Unix-Sicherheit

```java
var config = Client.Config.wss(
    java.net.URI.create("wss://localhost:8080"), "local", key,
    Path.of(".local/demo/ca.crt"));
```

Ohne CA-Datei (`null`) gelten die JDK-Trustroots. Nur WSS mit TLS 1.3 und
Hostname-Prüfung, keine abschaltbare Zertifikatsprüfung, Redirects oder Übernahme
von Proxy-Umgebungsvariablen. Unix verwendet den JDK-SocketChannel, prüft den
Pfad, Modus 0600 und Kernel-Peer-Identität über `SO_PEERCRED`. Aktuelle OS-Identität
über `UnixSystem`, unabhängig von der veränderlichen Java-Property `user.name`.
Dafür werden die JDK-Module `jdk.net` und `jdk.security.auth` benötigt. Fehlende
Peer-Credentials führen zum Abbruch. Linux ist nativ getestet; macOS folgt in der CI.

## Weitere Operationen

- `publish(topic, payload, expires)`: bis zu 4096 rohe Bytes oder ein Katalogevent;
  `expires` ist `java.time.Instant`, mit `null` entscheidet die Topic-Policy.
- `Client.event(Wire.Event)`: Katalog 1/v1, gültige Enum-IDs, höchstens drei typisierte
  Parameter. `Client.bytes` kopiert die Eingabe in einen unveränderlichen ByteString.
- `listMessages(topic, cursor, limit)`: eigene aktive Nachrichten. Optionaler Filter
  und Cursor jeweils `null`; beim Paging Filter und Seitengröße beibehalten.
- `getMessage(id)`, `getPermissions(topic)`, `getCatalog("mynofy", 1)`.
  Katalogdefinition als JSON-Bytes in `getDefinition()`; die Anwendung verwendet
  ihren eigenen Parser. Das SDK interpretiert diese JSON-Definition nicht.

Rückgaben sind generierte Protobuf-Typen. Optionale Felder mit `has...()` prüfen.
`uint64` verwendet den vollständigen Bitbereich eines Java-`long`: Werte oberhalb
von `Long.MAX_VALUE` erscheinen negativ, bleiben aber unverändert. Dezimalausgabe
mit `Long.toUnsignedString`, Eingabe mit `Long.parseUnsignedLong`; `uint32`
entsprechend mit `Integer.toUnsignedLong`. Kein Umweg über `double`.

Beispiel für ein Event, IDs aus Katalog/Anwendungsvereinbarung:

```java
var event = dev.mynofy.protocol.Wire.Event.newBuilder()
    .setCatalog(1).setCatalogVersion(1)
    .setCategory(12).setAction(42).setStatus(2)
    .addParameters(dev.mynofy.protocol.Wire.Parameter.newBuilder()
        .setU64(Long.parseUnsignedLong("18446744073709551615")))
    .build();
client.publish("local/events", Client.event(event), null);
```

## Abbruch, Parallelität und Fehler

Synchrone, blockierende API; die WebSocket-Bibliothek betreibt ihre I/O-Threads.
Eine Operation pro Client. Gleichzeitige Aufrufe liefern `Client.BusyException`
ohne zusätzliche fachliche Queue. Für Parallelität getrennte Verbindungen nutzen.
`Config.timeout` begrenzt eine Operation, Standard zehn Sekunden. Verbindungsaufbau
inklusive Authentifizierung und der gesamte Schlüsselwechsel haben jeweils ein
solches Gesamtbudget. Poll-Intervalle bleiben Aufgabe der Anwendung.

`close()` von einem anderen Thread oder Unterbrechen des aufrufenden Threads
bricht den Austausch ab und verwirft die Verbindung. Der Interrupt-Status bleibt
erhalten. `Client.TransportException.outcomeUnknown()` weist auf möglicherweise
bereits angenommene Mutationen hin. Den Grund über `getCause()` prüfen, etwa
`InterruptedException` oder `SocketTimeoutException`. Kein blindes erneutes Publish.

`Client.BrokerException.error()` enthält Code, Detail und optional `retry_after_ms`.
Keine automatische Wiederholung, kein Backoff oder Reconnect. Nach Fehlern muss
die Anwendung explizit neu verbinden und Identität/Neustart prüfen.

## Schlüssel und Rotation

`Key.generate()`, `publicKey()`, `tokenId()` und `clientId()` liefern die Daten zur
Provisionierung. `privateBytes()` exportiert das gemeinsame PKCS#8-v2-Format;
Dateien selbst mit Modus 0600 speichern. Import prüft Format und Zusammengehörigkeit
von Private/Public Key durch JDK-Signatur und -Verifikation.

`Key.parse(der)` akzeptiert das exakt geprüfte v2-Format von Rust/Python/Go. Bei
kanonischem PKCS#8 v1 zusätzlich den rohen Public Key angeben:
`Key.parse(der, publicKeyBytes)`. Das JDK bietet keine direkte Ableitung des Public
Keys aus diesem Seed; dafür wird kein zusätzlicher Kryptoanbieter eingebunden.
Die API garantiert keine vollständige Speicherlöschung aller Schlüsselkopien.

`client.rotate(newKey)` verlangt, dass die Anwendung den neuen Schlüssel vorher
sicher gespeichert hat. Nach Erfolg ist die alte Sitzung geschlossen. Neu mit dem
neuen Schlüssel und der **ursprünglichen** `Config.clientId` verbinden. Bei
unklarem Commit beide Schlüssel behalten und den neuen Zugang prüfen. Das SDK
schreibt keine Schlüsseldateien.

## Tests und Generierung

Als normaler Benutzer mit Java 17, Maven, Rust/Cargo und OpenSSL:

```sh
./scripts/test-java-sdk.sh
# Optional zusätzlich Python-Schlüsselinterop:
MYNOFY_PYTHON=/pfad/zum/venv/bin/python ./scripts/test-java-sdk.sh
```

Die Tests starten temporäre Dienste, prüfen Unix/WSS, Auth/ACL, Bytes/Events,
Integergrenzen, Cursor, Rotation sowie Rust- und optional Python-Zugang.
Gemeinsame Wire-/Auth-/Rotationstranskripte kommen aus
`protocol/conformance/sdk-v1.json`. Deterministische Fehlerpeers prüfen Antwortverlust,
ungültige Frames und Welcome-Daten, Größenlimit, Timeout, Interrupt, Close und Busy.
JUnit und `protobuf-java-util` einschließlich JSON-Hilfen sind ausschließlich Testabhängigkeiten.
Vorhandene Demo-Zustände werden nicht verändert.

Bindings liegen im Repository. Nur zur Entwicklung mit offiziellem protoc 36.2:

```sh
PROTOC=/pfad/zu/protoc python3 sdks/java/generate.py
```

Der Generator übernimmt die gemeinsame `.proto`-Quelle mit Java-Ausgabeoptionen
und den Ereigniskatalog; keine zweite manuell gepflegte Protokolldefinition.

Kein Peer-/Owner-Tunnel, Admin-Zugang oder Agenten-/Polling-Zustandshelfer in dieser
Version. Rust/CLI unterstützen Owner-Tunnel. Weitere Sprachen: [SDK-Übersicht](../../docs/sdks.md).

## Owner-authenticated producer identity

Read `item.getMessage().getProducerId()` for the stable client identity stamped by the owner.
It is immutable, survives authorized key rotation and storage restore, and is
separate from any sender or actor claim inside the payload. Match workflow roles
to this value. This is an assertion over the protected owner connection, not a
portable cryptographic message signature.

For cursor continuity, compare `ConnectionInfo.bootId()` between authenticated sessions. A null value indicates a legacy owner.
