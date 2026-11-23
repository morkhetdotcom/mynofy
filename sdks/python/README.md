# mynofy Python-SDK


The broker uses stateless polling: ACK/NACK, delivery IDs and confirmation counters have been removed. Application results are published messages. See [upgrade notes](../../docs/message-client-confirmation.md).

Eigenständiger `asyncio`-Client ab Python 3.11, ohne CLI-Unterprozess oder
Server-Core-Abhängigkeit. Direkte WSS-Verbindungen mit TLS-/Hostname-Prüfung und
lokale Unix-Sockets mit Pfad-/Peer-UID-Prüfung. Wireformat: vorhandenes Protobuf v1.
Die API ist wie das Protokoll noch ein veränderlicher Prototyp.

## Installation aus dem Repository

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install ./sdks/python
```

Drei direkte Laufzeitabhängigkeiten: offizielle [Protobuf-Runtime](https://pypi.org/project/protobuf/),
[websockets](https://pypi.org/project/websockets/) und
[pyca/cryptography](https://pypi.org/project/cryptography/). Kein Fork und kein
zweiter Kryptoanbieter. `grpcio-tools` dient ausschließlich der Bindings-Generierung,
nicht dem laufenden SDK. Die `.proto`-Quelle liegt im Repository unter
`protocol/protobuf/mynofy.proto`; generierte Python-Typen werden mitgeliefert.

## Unix-Socket: Bytes senden und lesen

Der Public Key muss beim Owner registriert sein; eine Anmeldung registriert keinen
Client und vergibt keine Rechte. Schlüssel aus `mynofy keygen` sind kompatibel.
Als derselbe normale OS-Benutzer wie der lokale Service ausführen.

```python
import asyncio
from pathlib import Path
from mynofy import Client, Key

async def main():
    key = Key(Path('.local/unix/client.pk8').read_bytes())
    async with await Client.connect_unix(
        '.local/unix/run/mynofy.sock', owner='local', key=key,
    ) as client:
        print(client.info)  # Owner, stabile Client-ID, U32-Neustartzähler
        sent = await client.publish(
            'local/events', 'Hallo'.encode('utf-8'), content_type='text/plain',
        )
        page = await client.poll('local/events', limit=20)
        for item in page.items:
            print(item.message.id, item.message.payload)
            # Nach Verstehen/Zuordnen; das ist kein fachlicher Abschluss:
        # Nach Verarbeitung oder dauerhafter eigener Übernahme lokal speichern:
        cursor = page.next_cursor
        following = await client.poll('local/events', after=cursor, limit=20)
        print(sent.message_id, following.has_more)

asyncio.run(main())
```

Bei `has_more` weitere Seiten sofort abholen. Poll startet keine Hintergrundschleife.
Fehlender Cursor liest den aktiven Bestand erneut. Nach bekanntem Service-Neustart
anhand von `client.info.restart_counter` gespeicherte Cursor verwerfen und mit
lokalem Arbeitsstand abgleichen; kein automatisches Wiederholen von Fachaktionen.
Das SDK speichert weder Cursor noch Tickets oder Mandate und führt keine fachliche Bestätigung aus.
Poll reads without recording reader state. Losing its response leaves client-local progress unchanged.

## WSS

```python
client = await Client.connect_wss(
    'wss://localhost:8080', owner='local', key=key,
    ca_file='.local/demo/ca.crt', timeout=10,
)
# Benutzen und anschließend await client.close(), oder async with client.
```

Nur `wss://`, mindestens TLS 1.3, CA- und Hostname-Prüfung. `ca_file=None` nutzt
die Betriebssystem-Trustroots. Keine Option zum Abschalten der Prüfung und keine
Übernahme von HTTP-/SOCKS-Proxy-Umgebungsvariablen. Unix verwendet unter Linux
`SO_PEERCRED`, unter macOS `getpeereid`; native macOS-Verifikation folgt in der CI.
Nicht unterstützte Peer-UID-Abfrage führt zum Abbruch, nicht zu schwächerer Prüfung.

## Ereignisse und weitere Operationen

```python
from datetime import datetime, timedelta, timezone
from mynofy import Event, Parameter

catalog = await client.get_catalog()
# IDs und fachliche Parameterbedeutung aus Katalog/Anwendungsvereinbarung:
event = Event(catalog=1, catalog_version=1, category=12, action=42, status=2,
              parameters=[Parameter(u64=2**64 - 1)])
await client.publish('local/events', event,
                     expires=datetime.now(timezone.utc) + timedelta(seconds=60))
rights = await client.get_permissions('local/events')
own = await client.list_messages(topic='local/events', limit=20)
# Beim Paging Filter UND limit beibehalten:
if own.HasField('next_cursor'):
    own_next = await client.list_messages(topic='local/events', limit=20,
                                          cursor=own.next_cursor)
status = await client.get_message(sent.message_id)
```

Bytes werden explizit kodiert, maximal 4096 Byte; Events enthalten höchstens drei
numerische/boolesche Parameter. Keine implizite String-/JSON-Konvertierung. Python-
Integer und Protobuf erhalten `u64`/`i64` ohne Fließkommaumweg. Rückgaben sind die
mitgelieferten Protobuf-Typen (`mynofy.pb`); `get_catalog` liefert ein geprüftes
JSON-Dictionary. Optionale Felder mit `HasField` unterscheiden.

## Fehler, Parallelität und Schlüsselwechsel

- `BrokerError`: abgewiesene Anfrage mit `code`, `detail`, optional `retry_after_ms`.
  Keine automatische Wiederholung oder Warte-/Backoff-Strategie.
- `TransportError`: Verbindung nach I/O-/Protokollfehler verworfen. Bei Mutation
  kann `outcome_unknown=True` gelten: Publish/Rotation nicht blind erneut
  senden, sondern beim Owner und anhand des eigenen Zustands abgleichen.
- Cancellation während eines Requests verwirft ebenfalls die Verbindung. Auch
  bei `CancelledError` kann eine Mutation bereits angenommen worden sein.
- Eine Operation je Client. Gleichzeitiger weiterer Aufruf ergibt `BusyError`,
  keine unbegrenzte Warteschlange. Für Parallelität getrennte Verbindungen verwenden.
- Verbindung wird nicht automatisch neu hergestellt. Owner-/Client-Identität und
  Neustartzähler nach expliziter Neuverbindung prüfen.

Clientseitiger Wechsel mit `await client.rotate(new_key)`: neuen Schlüssel **vorher**
selbst sicher speichern. Nach Erfolg ist die alte Sitzung geschlossen; mit neuem
Key und unveränderter `client_id=old_client_id` neu verbinden. Bei unklarem Commit
beide Keys behalten und den neuen Zugang prüfen. Das SDK schreibt keine Keydateien.

`Key.generate()` erzeugt Ed25519, `public_key`, `token_id`, `client_id` liefern die
Registrierungsdaten. `private_bytes()` exportiert die Rust-kompatible PKCS#8-v2-
Darstellung; Import akzeptiert Standard-v1 und die exakt geprüfte Rust-v2-Form.
Python garantiert keine vollständige Speicherlöschung aller Schlüsselkopien.

## Grenzen

Kein Peer-/Owner-Tunnelzugang in dieser ersten Python-Version; Rust/CLI beherrschen
ihn bereits. Keine Admin-API, Topic-Anlage, Agentenlogik, Zustandsdateien oder
Polling-Daemons im SDK. Der Service bleibt alleiniger ACL-Entscheider.

## Entwicklung und Konformität

```sh
python -m pip install -e './sdks/python[dev]'
python sdks/python/generate.py
./scripts/test-python-sdk.sh
cargo test --locked -p mynofy-protocol --features crypto sdk_conformance
```

Die Tests benötigen das gebaute Rust-Binary, OpenSSL CLI und einen normalen
Benutzer. Sie starten eigene Dienste in temporären Verzeichnissen, keine vorhandene
Demo wird verändert. WSS/Unix, Auth/ACL, Katalog, Bytes/Events, große Integer,
Cursor, Bestätigungen, Python↔Rust-Keyformate und Rotation werden real geprüft;
Fehlerpeers prüfen Timeout, falsche Antwort, Cancellation und verlorene Resultate.

`protocol/conformance/sdk-v1.json` enthält dieselben Wire-/Auth-/Rotationstranskripte
für Rust und Python sowie spätere SDKs. Regeneration der Vektoren:

```sh
cargo run --quiet --locked -p mynofy-protocol --features crypto --example sdk_vectors > protocol/conformance/sdk-v1.json
```

## Owner-authenticated producer identity

Read `item.message.producer_id` for the stable client identity stamped by the owner.
It is immutable, survives authorized key rotation and storage restore, and is
separate from any sender or actor claim inside the payload. Match workflow roles
to this value. This is an assertion over the protected owner connection, not a
portable cryptographic message signature.
