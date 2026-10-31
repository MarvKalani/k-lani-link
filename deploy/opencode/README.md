# OpenCode als zweiter k-lani-link-Client

Das Profil verwendet **OpenCode 2.2.0**, mindestens die angefragte Version 2.1.4.
Offizielles Release, Commit und SHA256 stehen in [`release.json`](release.json).
OpenCode wird separat gebaut.

```text
OpenCode (network=none) → Loopback-API → privater Unix-Socket
       → k-lani-link-Client ⇄ verschlüsselter WebRTC-Tunnel ⇄ k-lani-link-Host
       → Unix-Socket → Basalt (network=none)
```

Der zweite Client verwendet denselben Basalt-Server und denselben bestehenden
k-lani-link-Tunnel wie das erste Labor. Es läuft kein zweites großes Modell auf der
GPU. Standardmäßig ist eine Anfrage aktiv. Das optionale
[verwaltete Profil](../managed/README.md) ermöglicht parallele Anfragen aus beiden
Anwendungen, wenn Backend-Slots und Transportgrenzen passend eingestellt sind.
Der geprüfte Parallelbetrieb verwendet zwei Slots und 6 GiB Backend-RAM ohne Swap.
Ein Backend-Neustart mit
derselben Modellkonfiguration benötigt keine neue Kopplung, solange beide Peers laufen.

## Voraussetzungen und Swap

Linux x86_64, UID/GID 1000, Docker Compose/BuildKit, Python ≥3.11 und die
NVIDIA-/Basalt-Voraussetzungen aus [der Host-Anleitung](../../docs/REFERENCE.de.md).

**Dieses Testprofil startet nur mit vollständig deaktiviertem Host-Swap.**
Vor `swapoff` muss ausreichend RAM für die ausgelagerten Seiten frei sein.
Erst große eigene Testprozesse beenden und `free -h` sowie `swapon --show`
prüfen; bei fehlendem RAM nicht blind abschalten.

Im eingerichteten Testrechner wurde Basalt zunächst gestoppt, der vorhandene
Swap-Inhalt sicher zurückgeladen und dann `swapoff --all` ausgeführt. Die
vorhandene Swap-Unit ist nur bis zum nächsten Neustart maskiert. Die Swap-Datei
und der Eintrag in `/etc/fstab` bleiben erhalten. Nach einem Neustart muss Swap
vor neuen Tests erneut deaktiviert werden; die Starter verweigern sonst den Test.
Es wird im Repository keine globale Host-Konfiguration automatisch verändert.

Das gewählte Profil `../thinkrail/compose.no-swap.yaml` setzt Basalt auf 4 GiB
Container-RAM, ohne Container-Swap, mit `--mmap-experts --prefill 512`. Die
Experten bleiben dateibasiert und werden über den Dateicache gelesen. Kontext
262.144 und int8-KV bleiben erhalten. Dieses Profil verändert Speicherverhalten
und Geschwindigkeit; frühere Messungen mit größerem residentem RAM sind keine
direkten Vergleichswerte. OpenCode erhält 1 GiB RAM ohne Container-Swap.
Für das verwaltete Profil kommt `../thinkrail/compose.managed.yaml` danach hinzu
und erhöht das Backend-Limit auf 6 GiB. Ein früherer Paralleltest mit 4 GiB führte
zu einem OOM-Abbruch; der dokumentierte 6-GiB-Test lief ohne OOM durch.

## Bestehendes Labor verwenden

Im bereits vorbereiteten Labor wählt eine private Markerdatei das Swap-freie
Profil für den Manager aus:

```sh
touch .thinkrail-test/no-swap.profile
docker compose --env-file .thinkrail-test/compose.env \
  -f deploy/thinkrail/compose.yaml -f deploy/thinkrail/compose.no-swap.yaml build basalt guard
python3 -B deploy/thinkrail/manage.py up --backend-only

docker build -f deploy/opencode/Dockerfile -t k-lani-opencode:2.2.0 .
python3 -B deploy/opencode/prepare.py
python3 -B deploy/opencode/manage.py up
```

`prepare.py` erzeugt neue private Dateien und überschreibt kein bestehendes Setup.
Die Referenz setzt das API-Volume `k-lani-thinkrail-test_api_socket` voraus.
Für eine erste Installation können die Basisdateien mit
`deploy/thinkrail/prepare.py --model-file /absolute/path/to/model.basalt`
vorbereitet werden. Dafür zuerst das k-lani-link-Image bauen. Anschließend nur
`basalt host guard` aus der Basiskonfiguration bauen und `manage.py up --backend-only`
verwenden. Der Name des Basisprojekts bleibt aus Kompatibilitätsgründen erhalten;
ein ThinkRail-Image wird für den Betrieb mit `--backend-only` nicht benötigt.

OpenCode: <http://127.0.0.1:24244/>. Ressourcen: <http://127.0.0.1:24245/>.
Das synthetische Projekt liegt im Container unter `/work`.
Die Stats liegen auf einer separaten Origin, damit OpenCodes Service Worker sie
nicht durch die App-Shell ersetzt. Beide Ports werden ausschließlich an Host-Loopback gebunden.

## Isolation und lokale Daten

OpenCode hat ausschließlich Loopback. Sein API-Client spricht den privaten
k-lani-link-Socket über eine lokale Bridge an. Der UI-Zugang erfolgt über einen
weiteren privaten Unix-Socket und ein Gateway auf Host-Loopback. Host/Origin
werden geprüft; die CSP sperrt externe Browserressourcen. Der separate Netzwerk-
Guard sperrt ausgehendes IPv4, IPv6 und Docker-DNS. Ein internes zufälliges
Serverpasswort wird vom Gateway gesetzt und nicht im Browser-URL übertragen.

Nur der Provider `link` ist erlaubt. Updates, Modellsynchronisierung,
Sharing und Warming sind deaktiviert; Netzisolation bleibt die maßgebliche Grenze.
Der Container erhält keinen Docker-Socket, keine Modellgewichte und keine
Anmeldedaten anderer Anbieter. Das Root-Dateisystem ist schreibgeschützt, Linux-
Capabilities sind entfernt und der Docker-Logtreiber ist deaktiviert.

OpenCode speichert seinen lokalen Agentenverlauf im privaten Home-Volume.
Sein Log-/Cache-Verzeichnis liegt auf tmpfs, der Server läuft mit Log-Level
`none`, stdout/stderr werden verworfen. Das ist keine Behauptung, dass der Client
keinen lokalen Verlauf hat. Projektdateien und diese lokale Datenbank verursachen
weiterhin normale Dateisystemschreibzugriffe; deaktivierter Swap verhindert
zusätzliche Swap-Schreiblast, nicht sämtliche SSD-Schreibvorgänge.

## Reproduzierbarer Funktionstest und Messung

```sh
python3 -B deploy/opencode/verify.py
python3 -B deploy/opencode/manage.py record --duration 3600
docker compose --env-file .opencode-test/compose.env -f deploy/opencode/compose.yaml \
  exec -T opencode python -B - < deploy/opencode/agent_check.py
python3 -B deploy/opencode/manage.py stop-recording
```

Der Agent liest `seed.txt`, schreibt `opencode-test.txt`, prüft die Datei mit
einem Shell-Befehl und liefert eine Abschlussmarkierung. Die Prüfung speichert
nur zusammengefasste Ereigniszahlen, keine Inferenztranskripte. Ein erneuter Lauf
benötigt eine frische Ergebnisdatei/Fixture; bestehende Ergebnisse werden nicht
unbemerkt überschrieben.

Geprüft: ein echter Basalt-Agentenlauf mit vier Reasoning-Abschnitten und drei
Werkzeugaufrufen, korrektes Ergebnis, unveränderte Ausgangsdatei und kein Commit.
Die Abnahme prüft zusätzlich Netzsperren, den gestoppten Vermittler, Swap auf Host
und Containern sowie die tatsächlich gemappte Modelldatei. Der Kontext ist auf
256K konfiguriert; der OpenCode-Funktionstest füllt dieses Fenster nicht vollständig.

Der Sampler läuft als eigener Prozess. CSV-/JSON-Dateien liegen unter
`.opencode-test/measurements/`. Metadaten nennen Modell, Speicherprofil und
Dateisystemgerät. Jede Probe enthält Swap-Gesamtgröße, Nutzung und kumulative
Swap-Lese-/Schreibzähler. Wird Swap aktiviert oder nicht mehr prüfbar, endet eine
mit `--require-no-swap` gestartete Messung als ungültig. GPU-Werte beziehen sich
wie im ersten Labor auf die gesamte GPU; nur Prozess-VRAM wird Basalt zugeordnet.

## Modell auf schnellem Speicher

Der eingerichtete Test verwendet das identisch geprüfte Coder-Pack auf einem
schnellen lokalen Datenträger (RAID 0 über zwei NVMe-SSDs, ext4). Docker bindet die
Modelldatei schreibgeschützt ein. Ein Durchsatz von 14,3 GB/s wurde in diesem Test nicht neu gemessen.
Modellzugriff, Betriebssystemcache und sequenzieller Speicherdurchsatz sind
unterschiedliche Messgrößen. Das ursprüngliche Modell wurde nicht gelöscht;
das NVFP4-Pack wurde für diesen Test nicht kopiert oder gestartet.

## Beenden

`manage.py down` beendet nur den OpenCode-Aufbau und dessen Messung, ohne Volumes
zu löschen. Der gemeinsame Basalt-/k-lani-link-Aufbau bleibt bestehen. Vor einer
neuen Kopplung im Basislabor OpenCode stoppen und danach neu starten, damit die
neuen lokalen Zugangsdaten geladen werden. Die eigene k-lani-link-Lizenz ist in [LICENSING.md](../../LICENSING.md) erklärt.

## Optional: WordCraft und GridCraft für die englische Demo

`craft-release.json` pinnt die offiziellen Linux-Pakete einschließlich SHA256.
Das zusätzliche Image enthält die unveränderten CLIs, ihre Lizenzhinweise und
lokale Fonts. Zur Laufzeit bleiben Netzwerk, Logging und Swap gesperrt. Kein
zusätzlicher Modellanbieter wird konfiguriert. WordCraft und GridCraft sind
Dokumentwerkzeuge, keine LLM-Dienste.

```sh
docker build -f deploy/opencode/craft/Dockerfile -t k-lani-opencode-craft:2.2.0 .
python3 -B deploy/opencode/prepare-demo.py
python3 -B deploy/opencode/manage.py up
```

Der Marker `.opencode-test/craft.profile` aktiviert die optionale Compose-Datei.
`prepare-demo.py` legt nur fiktive Kundendaten, die Aufgabenbeschreibung und das
Originalbild unter `/work/demo` ab. Das Ausgabe-Verzeichnis ist zunächst leer;
der lokale Agent erzeugt die Rechnung, Tabelle und Webseite. Eine erneute
Vorbereitung überschreibt kein bestehendes Demo-Verzeichnis.

WordCraft erhält Lesezugriff auf `/work/demo` und Schreibzugriff auf dessen
`output`-Verzeichnis über seine eigenen MCP-Root-Flags. Die gepinnte GridCraft-
CLI bietet diese Flags nicht; hier ist der gesamte Container die Dateigrenze.
Darum ausschließlich synthetische Demo-Daten im gemounteten Arbeitsverzeichnis
verwenden. Beide MCP-Server werden als lokale stdio-Prozesse gestartet.

Die öffentliche Demo soll Basalts unveränderte Ausgabe zeigen. Aufbau,
Aufgabenbeschreibung, Bedienung und unabhängige Prüfung durch den Operator
werden getrennt dokumentiert. Bei Fehlern erhält derselbe lokale Agent einen
sichtbaren Folgeauftrag; die Ergebnisdateien werden nicht heimlich ersetzt.
Video und Messung sind ausdrücklich aktivierte Testartefakte und werden weder
ins Quellpaket aufgenommen noch automatisch veröffentlicht.

## Latenz messen

```sh
docker exec -i k-lani-opencode-test-opencode-1 python -B - \
  --socket /run/link-api/api.sock --key-file /run/secrets/client.toml \
  < deploy/measure-latency.py
```

Der Test misst 200 vollständige `GET /v1/models`-Anfragen nach zehn Aufwärmrunden,
ohne Modellgenerierung und ohne Payloads oder Schlüssel zu speichern. Die Werte
enthalten beide HTTP-Enden, Inhaltsverschlüsselung, WebRTC und Basalts kleine
Modellabfrage. Sie sind keine isolierte Netzwerk-RTT und keine Zeit zum ersten
Token. Im lokalen Labor lagen Median/p95 bei 0,780/1,060 ms; beide Peers liefen
auf demselben Host. Die Internetstrecke Firma–Zuhause wurde damit nicht gemessen.

## Zwei getrennte Rechner

Die [englische Kurzanleitung](../../QUICKSTART.md) verwendet den eigenständigen
Client-Stack und ein konfigurierbares `LINK_API_VOLUME`. So benötigt der
OpenCode-Rechner keinen lokalen Basalt-Container. Für dieses Remote-Profil
Compose direkt verwenden; die Labor-Messung erwartet weiterhin lokale Container.

## Tokenraten und Hardware

`manage.py live` zeigt aktuelle Werte ohne eine Messhistorie zu schreiben.
`record` exportiert zusätzlich `performance.csv`. Die optionale Basalt-Erweiterung
exportiert nur numerische Zähler in eine tmpfs-Datei; es werden keine Tokens,
Prompts oder Antworten aufgezeichnet. Live-Decode (2 Sekunden), letzte Decode-/
Prefill-Rate und Zeit zum ersten Token werden getrennt angezeigt. Fehlende oder
veraltete Werte erscheinen als nicht verfügbar. Hardware-, Betriebssystem- und
Docker-Metadaten stehen im Dashboard und in `metadata.json`.
