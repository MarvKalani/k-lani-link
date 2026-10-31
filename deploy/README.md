# Basalt ohne Netzwerk, k-lani-link als Gegenstelle

Basalt läuft mit `network_mode: none`, ohne veröffentlichte Ports und ohne
Netzwerkinterface außer Loopback. k-lani-link spricht die API über einen Unix-Socket
in einem gemeinsamen RAM-Volume an. Nur der k-lani-link-Host hat Netzwerkzugang für
Signaling, die ausdrücklich gewählten STUN-Server und den verschlüsselten Peer.
TURN bleibt standardmäßig deaktiviert.

Beide Container verwenden `logging.driver: none`, eine schreibgeschützte
Root-Dateisystemansicht, keine Linux-Capabilities und keine Privilegienerweiterung.
Das Modell wird als einzelne, schreibgeschützte Datei eingebunden. Tokenizer- und
temporäre Dateien liegen auf tmpfs. Basalt erhält keinen Docker-Socket und keinen
Zugriff auf Projektdateien des Agentenclients. Der Entrypoint verweigert den
Start, wenn ein zusätzliches Netzwerkinterface vorhanden ist.

## Voraussetzungen

Docker Compose, NVIDIA Container Toolkit und ein Blackwell-GPU-Treiber passend
zu CUDA 13.2. Der Build lädt den festgelegten öffentlichen Basalt-Commit und wendet
den mitgelieferten Datenschutz-/Socket-Patch an; siehe
[`../integrations/basalt/privacy.patch`](../integrations/basalt/privacy.patch).
Der Basalt-Commit steht in `BASE_COMMIT`. Der Patch ändert Python-Serving, keine
CUDA-Rechenkerne. Nach einem Update zuerst Patch und Schnittstellentests prüfen.

Das Image baut die Basalt-Binärdatei in seiner eigenen CUDA-/glibc-Umgebung.
Das vermeidet eine Abhängigkeit von der glibc des Entwicklungsrechners. Das
CUDA-Devel-Basisimage wird im MVP auch zur Laufzeit verwendet; eine kleinere
Runtime-Variante ist noch kein Bestandteil dieses Builds.

## Vorbereitung

Im k-lani-link-Projekt:

```sh
cargo build --release --locked
target/release/k-lani-link init --dir pair --signaling-backend ice-exchange \
  --signaling-url https://signal.example.invalid --stun stun:stun.marvins.app:3478
```

Die Vermittlungsadresse ersetzen. Der STUN ist frei wählbar: Das
[ICE-Beispiel](../examples/ice-servers.toml) lässt `stun.marvins.app` aktiv und
führt Google sowie einen eigenen alternativen Server nur als Kommentare auf.
Nur ausdrücklich aktivierte Einträge werden verwendet; TURN bleibt aus.

Nur in `pair/host.toml` den Upstream ändern:

```toml
upstream = "http://localhost"
upstream_socket = "/run/link/basalt.sock"
pairing_state_dir = "/var/lib/link"
```

Diese Einstellungen sind oberste TOML-Felder, sie gehören nicht unter `[[ice_servers]]`.
`client.toml` bleibt ohne Socket-Einstellung und kommt auf den Firmenrechner.
Die Dateien enthalten zufällige Zugangsdaten; erfolgreiche CLI-Befehle bleiben
still. Exitcode 0 bedeutet Erfolg.

`deploy/.env.example` nach `deploy/.env` kopieren und die absoluten Pfade setzen.
`BASALT_MODEL_PATH` muss die echte Modelldatei bezeichnen, bei Symlinks deren Ziel.
Die Host-Konfiguration muss für UID 1000 lesbar sein. Die Container verwenden
diese UID, das gemeinsame Socket-Volume gehört ebenfalls UID 1000.

Der Kopplungscode hat 43 Zeichen; seine ersten 12 bestimmen den Raum. Er wird
erst nach Bestätigung des verschlüsselten Tunnels verbraucht. Das Volume
`pairing_state` speichert ausschließlich leere Sperrdateien mit gehashtem Namen,
keine Chatdaten. Es muss über Containerneustarts hinweg erhalten bleiben;
`docker compose down` ohne `--volumes` verwenden. Für eine neue vollständige
Verbindung einen neuen Code auf beiden Geräten einrichten und die Prozesse neu starten.

## Bauen und starten

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml build
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d
```

Der erste Build benötigt Internet, um Buildwerkzeuge und Python-Pakete zu beziehen.
Zur Laufzeit hat Basalt keinerlei Internetroute. Ein normaler Start lädt das
Modell. Für einen reinen Starttest kann man Basalt mit `--lazy` starten; die erste
Inferenz lädt es dann. Der mitgelieferte Smoke-Test verwendet stattdessen
synthetische Mock-Ausgaben und belegt keinen VRAM für Modellgewichte.

Der k-lani-link-Host verwendet das Host-Netzwerk, damit Docker kein zusätzliches NAT
zwischen ICE und Heimrouter erzeugt. Unter Linux funktioniert das direkt;
Docker Desktop benötigt die passende Host-Networking-Unterstützung. Basalt teilt
dieses Netzwerk ausdrücklich nicht.

Für den Firmenrechner gibt es einen [eigenständigen Docker-Client und OpenCode-Aufbau](../QUICKSTART.md).
Alternativ wird der API-Client nativ gestartet:

```sh
target/release/k-lani-link client --config pair/client.toml
```

## Über das HTTPS-Relay statt WebRTC, gekoppelt per Code

Wo der Firmenrechner nur HTTPS erreicht, läuft der Host über das
[HTTPS-Relay](https/README.md). Basalt bleibt unverändert ohne Netzwerk. In `deploy/.env`
zusätzlich setzen:

```sh
LINK_RELAY_URL=https://relay.example.com
# Ordner mit genau den Dateien host-token und client-token (zwei Tokens desselben Bereichs)
LINK_TOKEN_DIR=/absoluter/privater/ordner
```

`LINK_HOST_CONFIG` entfällt. Die Token-Dateien müssen für UID 1000 lesbar sein. Beide
Variablen bleiben auch für spätere `up -d` gesetzt, weil Compose den `offer`-Dienst
immer mit auswertet. `compose.https.yaml` braucht ein aktuelles Docker Compose (v2.24
oder neuer, wegen `!override`). Einmal koppeln:

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml -f deploy/compose.https.yaml run --rm offer
```

Das zeigt einen Code wie `472-1234-5678`, zehn Minuten gültig. Auf dem Firmenrechner
eingeben: im Kontrollfeld des Programms oder mit `Manage.ps1 -Action Pair` im
[Desktop-Profil](desktop/README.md#pair-by-code). Danach wie gewohnt starten, mit
denselben drei `-f`/`--env-file`-Angaben und `up -d`.

- Die Kopplung liegt im Volume `pairing_state`; der Host liest sie von dort. Das
  Token-Verzeichnis sieht nur der kurzlebige `offer`-Container, schreibgeschützt.
- Der Host erreicht Basalt über den Socket `/run/link/basalt.sock`; das trägt `offer`
  selbst in die Host-Konfiguration ein.
- Ein zweites `offer` wird abgelehnt, solange die Kopplung besteht. Für eine neue Kopplung
  den Stack mit `down` anhalten und das Volume `k-lani-link_pairing_state` bewusst
  löschen. Damit ist auch der bisher gekoppelte Rechner getrennt.

## Prüfen

```sh
BASALT_MODEL_PATH=/absolute/path/to/model.basalt python3 deploy/smoke.py
```

Der Test erzeugt eigene temporäre Zugangsdaten und einen eigenen Compose-Projektnamen.
Er prüft die Netzisolation, einen scheiternden ausgehenden TCP-Verbindungsversuch,
die Dateisystem-/Logging-Einstellungen sowie Chat- und Responses-Streaming mit
Reasoning über zwei echte WebRTC-Peers. Vor den Inferenzanfragen stoppt er den
Vermittler. Er entfernt anschließend ausschließlich seine eigenen Container und
seine Test-Volumes. Die Anfragen enthalten nur synthetische Testtexte. Er prüft
außerdem, dass ein verbrauchter Code nach Neustart keine weitere Kopplung erlaubt.

## Grenzen

GPU-Durchreichung vermeidet GPU-Emulation. Eine praktisch gleiche Tokenrate ist
plausibel, aber kein Ergebnis dieses Tests: Ein Leistungs-/Qualitätsvergleich mit
echter Inferenz und identischem GPU-Powerlimit steht noch aus.

tmpfs und Prozessspeicher sind keine Chatlogs, können aber abhängig vom Host in
Swap/Crash-Abbilder geraten. Docker-Daemon, Betriebssystem und GPU-Treiber liegen
außerhalb des Anwendungsschalters. Das ist eine technische Isolation und keine
DSGVO-Zertifizierung. OpenCode/Cline/Codex führen ihre lokalen Verläufe und
Netzwerkwerkzeuge auf dem Firmenrechner unabhängig davon aus.
