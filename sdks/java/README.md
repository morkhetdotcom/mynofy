# mynofy Java-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Stand-alone client from Java 17 for direct WSS and local Unix connections. Existing prototype
v1, without CLI subprocesses or server core dependency. API and protocol are still variable
prototypes.

## Einbinden

The Maven artifact has not yet been published.

```sh
mvn -f sdks/java/pom.xml -DskipTests install
```

In his own Maven project:

```xml
<dependency>
  <groupId>dev.mynofy</groupId>
  <artifactId>mynofy-client</artifactId>
  <version>0.4.0-SNAPSHOT</version>
</dependency>
```

Two Direct Term Dependencies: Official
[Protobuf-Runtime](https://protobuf.dev/reference/java/java-generated/) and
[Java-WebSocket](https://github.com/TooTallNate/Java-WebSocket), and its transitory SLF4J - API.
The SDK installs no logging provider. TLS, Ed25519 and AES-GCM use the JDK
-Implementations. No gRPC, additional JNI package, JSON -parser or protobuf compiler at runtime. JAR
unter `dev.mynofy.protocol.Wire`.

<a id="lokal-senden-und-lesen"></a>

## Send locally and read

Public key and topic rights must be registered with the owner. `mynofy keygen` creates
compatible keys directly.The same normal OS user as the service:

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
                // After Verstehen/Zuordnen; no professional degree:
            }
            System.out.println(sent.getMessageId() + " " + page.getNextCursor());
        }
    }
}
```

`poll` reads a page immediately; at `has_more` more pages with `next_cursor`
collect. save cursor only after processing or permanently own takeover. `null` reads the active
stock again. read again after a known restart using `info().restartCounter()` without cursor and
compare it with local work status. SDK does not store any progress and does not confirm
automatically. Poll reads without recording reader state; the client keeps its progress locally.

<a id="wss-und-unix-sicherheit"></a>

## WSS and Unix security

```java
var config = Client.Config.wss(
    java.net.URI.create("wss://localhost:8080"), "local", key,
    Path.of(".local/demo/ca.crt"));
```

Without CA file (`null` ) apply the JDK -Trustroots. Only WSS with TLS 1.3 and hostname check,
no turn-off certificate check, redirects or adoption of proxy environment variables. Unix uses
the JDK -SocketChannel, checks the path, mode 0600 and kernel peer identity over `SO_PEERCRED`.
Current OS identity over `UnixSystem`, regardless of the changeable Java property `user.name`.
For this purpose, the JDK modules `jdk.net` and `jdk.security.auth` are required.

<a id="weitere-operationen"></a>

## Other operations

- `publish(topic, payload, expires)`: up to 4096 raw bytes or a catalog event; `expires` is
  `java.time.Instant`, with `null` decides the topic policy.
- `Client.event(Wire.Event)`: Catalogue 1/v1, valid enum IDs, maximum three typed parameters.
  `Client.bytes` copies the input into an immutable ByteString.
- `listMessages(topic, cursor, limit)`: own active messages. Optional filter and cursor each
  `null`; maintained with paging filter and page size.
- `getMessage(id)`, `getPermissions(topic)`, `getCatalog("mynofy", 1)`. Catalog definition as
  JSON -Bytes in `getDefinition()` The application uses its own parser. SDK interpretiert diese
  according to its JSON definition.

Returns are generated protobuf types. `has...()` check. `uint64` Uses the full bit range of a
Java- `long` Values above `Long.MAX_VALUE` appear negative, but remain unchanged.
`Long.toUnsignedString`, Eingabe with `Long.parseUnsignedLong`; `uint32` entsprechend with
`Integer.toUnsignedLong` No detour through `double`.

Example of an event, IDs from catalog/application protocol agreement:

```java
var event = dev.mynofy.protocol.Wire.Event.newBuilder()
    .setCatalog(1).setCatalogVersion(1)
    .setCategory(12).setAction(42).setStatus(2)
    .addParameters(dev.mynofy.protocol.Wire.Parameter.newBuilder()
        .setU64(Long.parseUnsignedLong("18446744073709551615")))
    .build();
client.publish("local/events", Client.event(event), null);
```

<a id="abbruch-parallelität-und-fehler"></a>

## Abort, parallelism and errors

Synchronous, blocking API; the WebSocket library operates its I/O-Threads. one operation per
client. Simultaneous calls provide `Client.BusyException` without additional technical queue.
Use separate connections for parallelism. `Config.timeout` limits an operation, standard ten
seconds. Connection setup including authentication and the entire key change each have such a
total budget. Poll intervals remain the task of the application.

`close()` from another thread or interrupting the calling thread
discontinues the exchange and discards the connection. The interrupt state is retained.
`Client.TransportException.outcomeUnknown()` indicates possibly already assumed mutations. Check
the reason about `getCause()`, such as `InterruptedException` or `SocketTimeoutException`.

`Client.BrokerException.error()` contains code, detail and optionally `retry_after_ms`.
No automatic repetition, no backoff or reconnect. After errors, the application must explicitly
reconnect and check identity/restart.

<a id="schlüssel-und-rotation"></a>

## Key and rotation

`Key.generate()`, `publicKey()`, `tokenId()` and `clientId()` provide the data for
Provisioning. `privateBytes()` exports the common PKCS #8-v2 format; save files even with mode
0600. Import verifies format and togetherness of Private/Public Key by JDK signature and
verification.

`Key.parse(der)` accepts the exactly tested v2 format of Rust/Python/Go.
canonical PKCS #8 v1 additionally specify the raw public key: `Key.parse(der, publicKeyBytes)`.
JDK does not provide a direct derivation of the public key from this seed; no additional crypto
provider is included. API does not guarantee complete memory erasure of all key copies.

`client.rotate(newKey)` requires that the application first use the new key
After success, the old session is closed. New with the new key and the **original**
`Config.clientId` If the commit is unclear, keep both keys and check the new access. SDK does
not write key files.

<a id="tests-und-generierung"></a>

## Testing and generation

As a normal user with Java 17, Maven, Rust/Cargo and OpenSSL:

```sh
./scripts/test-java-sdk.sh
# Optional additional Python keyinterop:
MYNOFY_PYTHON=/pfad/zum/venv/bin/python ./scripts/test-java-sdk.sh
```

The tests start temporary services, check Unix/WSS, Auth/ACL, Bytes/Events, integer limits,
cursor, rotation, and rust and optionally Python access. Common Wire-/Auth-/Rotationstranskripte
comes from `protocol/conformance/sdk-v1.json`. Deterministic error peers check response loss,
invalid frames and welcome data, size limit, timeout, interrupt, close and busy. JUnit and
`protobuf-java-util` including JSON helps are only test dependencies. Existing demo states are
not changed.

Bindings are in the repository, only for development with official protoc 36.2:

```sh
PROTOC=/pfad/zu/protoc python3 sdks/java/generate.py
```

The generator handles the common `.proto` source with Java output options and the event catalog;
no second manually maintained protocol definition.

Kein Peer-/Owner-Tunnel, admin access, or Agenten-/Polling-state helper in this version.
Rust/CLI support owner tunnels. Other languages: [SDK - Overview](../../docs/sdks.md).

## Owner-authenticated producer identity

Read `item.getMessage().getProducerId()` for the stable client identity stamped by the owner. It is immutable, survives authorized key rotation and storage restore, and is separate from any sender or actor claim inside the payload. Match workflow roles to this value. This is an assertion over the protected owner connection, not a portable cryptographic message signature.

For cursor continuity, compare `ConnectionInfo.bootId()` between authenticated sessions. A null value indicates a legacy owner.
