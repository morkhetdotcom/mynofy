# Service-Peers zur Laufzeit verwalten

Die lokale Admin-API kann eingehende Peers und ihre Topic-Allowlisten ändern,
bekannte Nachbarn sperren sowie einzelne Peer-Zertifikate widerrufen. Die
Client-/Owner-Authentifizierung im Tunnel bleibt unverändert. Kein neuer
Datenkanal- oder MCP-Befehl und keine zusätzliche Abhängigkeit.

## Voraussetzungen und Befehle

Ein eigener `service.admin_socket` ist erforderlich. Für neue eingehende Peers
müssen TCP/TLS und `service.tls.peer_ca_file` bereits beim Start eingerichtet sein.
Die CA wird jetzt auch bei anfangs leerer `accepted_peers`-Liste geladen und für
optionale TLS-Clientzertifikate verwendet. Ein konfigurierter, aber fehlender oder
ungültiger CA-Pfad ist deshalb ein Startfehler; kein stilles Ignorieren der Datei.
Normale Clients benötigen weiterhin kein TLS-Clientzertifikat, sondern den
vorhandenen mynofy-Keypair-Handshake.

```sh
mynofy admin peers list --socket run/admin.sock

# Eingehenden Peer neu freigeben oder vollständig ersetzen:
mynofy admin peers set edge --topics team/events,owner/events --socket run/admin.sock
# Abweichender Zertifikats-SAN, sonst entspricht er der Service-ID:
mynofy admin peers set edge --identity edge.example --topics team/events --socket run/admin.sock

# Auch für den konfigurierten ausgehenden Upstream möglich:
mynofy admin peers disable edge --socket run/admin.sock
mynofy admin peers enable edge --socket run/admin.sock

# Eingehende Freigabe entfernen:
mynofy admin peers delete edge --socket run/admin.sock
```

`set` ersetzt die gesamte Allowlist, erweitert sie nicht implizit. Es gelten
Konfigurationslimits und Validierung: eindeutige Service-/Zertifikatsidentitäten,
vorhandene exakte Topics, keine Duplikate oder leere Allowlist. `list` liefert
lexikalisch sortierte eingehende Peers mit Sperrstatus, `next_after` und den
konfigurierten Upstream samt Sperrstatus. Paging: `--after ID --limit 1..100`.

Sperren bleibt unabhängig vom Entfernen der Freigabe erhalten. Eine spätere
Neuanlage hebt die Sperre nicht auf; dafür ausdrücklich `enable` verwenden.
`delete` entfernt nur eingehende Freigaben und ändert keine Upstream-Adresse.
Eine identische `set`-/Sperroperation verändert bestehende Links nicht erneut.

## Einzelnes Zertifikat widerrufen

```sh
mynofy admin peers revoke-certificate --certificate edge-leaf.pem --socket run/admin.sock
mynofy admin peers revoked-certificates --socket run/admin.sock
# Alternativ mit bekanntem Fingerprint:
mynofy admin peers revoke-certificate --fingerprint 'sha256:BASE64' --socket run/admin.sock
```

Die erste PEM-Zertifikatsdatei bzw. der erste Zertifikatseintrag muss das
Leaf-Zertifikat sein. Der CLI berechnet SHA-256 über dessen vollständige DER-
Darstellung. Der Fingerprint hat das Format `sha256:` plus kanonisches Standard-
Base64 von 32 Hashbytes. Er ist kein SPKI-/Public-Key-Pin und kein TOFU-Verfahren.
Alternativ zeigt `admin status` unter
`service_links.registered_certificates` die Leaf-Fingerprints aktiver Nachbarn.

Der lokale Widerruf wirkt auf eingehende **und** ausgehende Peer-Links. Er
ersetzt keine CA-/SAN-Prüfung und ist keine CA-weite CRL-/OCSP-Infrastruktur.
Es gibt keinen automatischen Wiederfreigabebefehl für ein widerrufenes
Zertifikat: ein neues, gültiges und nicht widerrufenes Zertifikat provisionieren;
lokales Nachladen von TLS-Dateien ist nicht Teil dieses Ausbaus. Die Sperrliste
ist lokal auf höchstens 10.000 Fingerprints begrenzt. Die Listenansicht unterstützt
`--after FINGERPRINT --limit 1..100`.

## Wirkung auf aktive Verbindungen

Peer-Änderung/-Entfernung oder Sperre invalidiert die betreffende Registrierung.
Zertifikatswiderruf invalidiert nur Links mit dem betroffenen Leaf-Fingerprint.
Manager werden benachrichtigt und brechen auch bei laufender I/O ab. Ihre
Tunnel-/Route-Worker werden beendet, Kanalressourcen freigegeben und gelernte
Routen entfernt. Andere Peer-Links bleiben bei unveränderter Policy bestehen.
Sperren und anschließendes schnelles Entsperren reaktiviert keine alte Lease;
der Peer muss neu authentifizieren.

Die RAM-Policy wirkt vor der Admin-Antwort. Physisches Schließen/Worker-Cleanup
folgt asynchron; die Antwort bestätigt keinen globalen Stillstand aller externen
Aktionen. Bereits beim Owner angenommene Commands werden nicht zurückgerollt.
Betroffene Clients können einen unklaren Ausgang haben. Es gibt keine automatische
Command-Wiederholung oder Sitzungswiederaufnahme. Ein ausdrücklich neuer Zugriff
kann bei weiterhin gültiger Freigabe einen frischen Link herstellen.

Lokale Topics und direkte Clientverbindungen bleiben verfügbar. Eine Allowlist
beschränkt Route-/Tunnelöffnung, nicht die verschlüsselten Befehle im Owner-Tunnel;
die aktuelle Owner-ACL bleibt die maßgebliche Clientautorisierung.

## Speicherung und Grenzen

Peer-Freigaben, Sperren und Zertifikatswiderrufe sind Sicherheitskonfiguration.
Sie wirken sofort im RAM und werden geordnet asynchron im bestehenden
Security-Journal gespeichert. Bei voller/nicht verfügbarer Queue erfolgt vor
Mutation ein Fehler. Der Admin-Security-Status zeigt den dauerhaften Stand;
eine Laufzeitbestätigung ist weiterhin kein synchroner Disk-Commit.

Neustart lädt den gespeicherten Sicherheitsstand, ein Admin-Export enthält ihn.
Die Originalkonfiguration wird nicht zurückgeschrieben. Neue optionale CFG-Felder:
`disabled_peers` und `revoked_peer_certificates`. Alte Snapshots ohne Peer-Felder
übernehmen einmal ihre Startkonfiguration als Baseline; danach gilt die gespeicherte
Peer-Policy. Neue Journalvarianten sind mit älteren Binaries nicht lesbar.

Ein Topic mit Peer-Freigabe lässt sich erst nach Entfernen dieser Freigabe löschen
(`topic_in_use`). Beim Wiederanlauf verschwundene, ungespeicherte dynamische RAM-
Topics verlieren auch ihre zugehörigen Peer-Grants; eine spätere Neuanlage erbt
sie nicht. Andere unerwartet fehlende Topic-Verweise bleiben ein Startfehler.

Listener, CA-/Dienstzertifikatsdateien, Upstream-Adresse und Remote-Owner-Routen
bleiben Startkonfiguration. Dieser Ausbau ist kein generischer Config-Reload,
kein dynamisches Routingprotokoll und keine automatische Topic-Ankündigung.

## Verifikation

Unit-Tests prüfen Validierung, Revisionen, Idempotenz, Security-Queue-Atomarität,
Export, Journal-/Snapshot-Restore einschließlich alter Snapshots und verlorener
RAM-Topics. Echte CLI-/mTLS-Tests prüfen Freigabe bei anfangs leerer Peer-Liste,
Änderung, Sperren/Entsperren, Entfernen, Tunnelabbruch, Weiterbetrieb lokaler Topics
und Zertifikatswiderruf in beiden Richtungen. Die bestehende Link-Suite deckt
weiter Mehrhop, Owner-Auth/ACL, Kapazitäten und Rotation ab.

```sh
cargo test --locked -p mynofy-server runtime_peer
cargo test --locked -p mynofy-cli --test links
```
