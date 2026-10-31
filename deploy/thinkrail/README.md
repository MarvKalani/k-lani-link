# ThinkRail Nightly / Pi → k-lani-link → Basalt

Dieses Linux-Labor führt den echten ThinkRail-/Pi-Agenten gegen einen lokalen
Basalt-Server über zwei verschlüsselte WebRTC-Peers aus. Es ist die Grundlage
für die geplante Video-Demo und Ressourcenmessung. Beide Peers laufen zunächst
auf demselben Rechner; die Verbindung zwischen Firmen- und Heimnetz ist ein
gesonderter Test mit der [normalen Host-Konfiguration](../../docs/REFERENCE.de.md).

Festgelegt ist **ThinkRail 0.3.0-nightly.7 mit Pi 1.4.0**. Version, Commit,
Downloadadresse und SHA256 stehen in [`release.json`](release.json).
Der Container aktualisiert sich nicht während eines Laufs. Eine spätere Nightly
wird durch eine bewusste Änderung dieser Datei und erneute Tests übernommen.

## Aufbau

```text
Browser auf dem Host → 127.0.0.1:24242 → lokales UI-Gateway → Unix-Socket
                                                           ↓
                                            ThinkRail / Pi (network=none)
                                                           ↓ Unix-Socket
k-lani-link-Client ⇄ DTLS + Inhaltsverschlüsselung ⇄ k-lani-link-Host
                                                           ↓ Unix-Socket
                                               Basalt (network=none)
```

Das lokale Labor benötigt weder STUN noch TURN. Die beiden k-lani-link-Peers und
der lokale Testvermittler teilen einen Netzwerkraum mit ausgehendem Default-Deny.
Ein kleiner separater Guard setzt die Regeln; nur er erhält `NET_ADMIN`.
Docker-DNS wird anhand des ursprünglichen Zielports gesperrt, auch nach DNAT.
Nach Tunnelaufbau wird der Vermittler gestoppt. Alle weiteren Requests laufen
über den bestehenden authentifizierten DataChannel.

ThinkRail und Basalt haben jeweils nur Loopback. HTTP-API und UI benutzen private
Unix-Sockets mit 0600. Der Agent erhält weder Docker-Socket noch Host-Netzwerk.
Das UI-Gateway bindet ausschließlich an Host-Loopback, prüft Host/Origin und
beschränkt Browserverbindungen per CSP. Abhängigkeiten, externe Fonts, CDN-Skripte,
Internetrecherche und Paketdownloads im Agenten funktionieren in diesem Profil nicht.

## Voraussetzungen und erster Start

Linux x86_64, Docker mit Compose/BuildKit, Python ≥3.11, Git, UID/GID 1000,
NVIDIA Container Toolkit, CUDA-13.2-kompatibler Treiber und eine passende GPU.
Referenz: RTX 5070 Ti, 16 GiB VRAM, 32 GiB Host-RAM. Für andere Benutzer-IDs
zuerst Compose-Benutzer, Socket-Volumes und Image-Verzeichnisse anpassen.
Nur eine Laborinstanz pro Host verwenden; Projektname und UI-Port sind fest.
Modelldateien müssen bereits lokal vorhanden sein; sie werden nicht mitgeliefert.

Aus dem Repository-Stamm zunächst das k-lani-link-Image bauen und private Daten anlegen:

```sh
docker build -f deploy/link.Dockerfile -t k-lani-link-host:local .
python3 -B deploy/thinkrail/prepare.py \
  --preset coder --model-file /absolute/path/to/Qwen3.8-FN-GSQ-RCO-Coder.basalt
docker compose --env-file .thinkrail-test/compose.env \
  -f deploy/thinkrail/compose.yaml build
python3 -B deploy/thinkrail/manage.py up
python3 -B deploy/thinkrail/verify.py
```

Der Build benötigt Internet. Basalt wird aus einem festgelegten öffentlichen
Git-Commit gebaut und um den Serving-Datenschutzpatch ergänzt. ThinkRail wird
als offizielles Nightly-Binary heruntergeladen und vor Verwendung geprüft.

UI: <http://127.0.0.1:24242/>. Ressourcen: <http://127.0.0.1:24242/stats>.
Das vorbereitete Projekt enthält nur synthetische Testdateien. Das Modell wird
über `/v1/models` entdeckt; ThinkRail startet erst bei mindestens 262.144 Tokens
Backend-Kontext. Das Profil verwendet int8-KV, maximal 16.384 Ausgabetokens und
ein Basalt-Reasoningbudget von 2.048 Tokens. Kontext umfasst Ein- und Ausgabe.

## Zwei Modell-Packs

Mit dem [verwalteten Profil](../managed/README.md) werden beide lokalen Dateien
schreibgeschützt eingebunden. Beim Einrichten `prepare.py --managed --parallel 2`
zusätzlich die Coder-Datei als `--model-file` und NVFP4 als
`--nvfp4-model-file` angeben. Der Helfer erzeugt einen privaten Modellkatalog
und einen separaten Bedienerschlüssel, den die KI-Anwendung nicht erhält.

```sh
python3 -B deploy/thinkrail/manage.py select-model --preset nvfp4
# Zurück zur Coding-Demo:
python3 -B deploy/thinkrail/manage.py select-model --preset coder
```

Der Wechsel wartet auf vorherige Anfragen, hält folgende kurz zurück und tauscht
nur den Modellprozess aus. Tunnel, Kopplungscode und Ressourcenaufnahme bleiben
bestehen. Anschließend in der Anwendung das geladene Modell (`coder` oder
`nvfp4`) auswählen. Der Client kann den bisherigen Gesprächsverlauf erneut senden;
ein KV-Cache wird nicht zwischen Modellen übertragen. Es ist immer nur eines
der beiden großen Packs geladen. Alte unverwaltete Labs müssen ausdrücklich auf
das neue Profil migriert werden; `select-model` baut kein neues Kopplungspaar mehr.

Die lokal untersuchten Dateien: Coder 56,652 GiB mit Modell-ID
`Qwen3.8-Flash-Next IQ3_XXS`; NVFP4 121,976 GiB mit ID
`Qwen3.8-Flash-Next NVIDIA-NVFP4`. Die Coder-Datei unterscheidet sich in Datei-
und Expertengröße von der offiziellen IQ3_XXS-Zeile mit 74,4/40,0 GiB.
Eine Top-1-Übereinstimmung von 83,96 % wird ihr deshalb nicht zugewiesen.
Die 91,46 % aus der NVFP4-Tabelle sind ein Referenztest des Anbieters,
keine hier neu gemessene Coding-Erfolgsquote. Die zuvor berichteten
43–46 bzw. 19–26 tok/s sind frühere Betreiberangaben, kein Ergebnis dieses Labors.

## Ressourcen aufnehmen

```sh
python3 -B deploy/thinkrail/manage.py record --duration 14400
# Agentenaufgabe ausführen; anschließend:
python3 -B deploy/thinkrail/manage.py stop-recording
```

Der Host-Sampler liest einmal pro Sekunde Docker-Statistiken für ThinkRail und
Basalt. Ausgaben unter `.thinkrail-test/measurements/<UTC-Zeit>/`:

- `metadata.json`: Image-/Containerkennungen, Modell-Preset, Dateiname, Größe, Kontext.
- `containers.csv`: CPU, RAM roh/Working Set/Limit, Block-I/O, Netzwerkzähler, Prozesse.
- `system.csv`: Host-Auslastung, verfügbarer RAM, Swap und GPU-Werte.
- `samples.jsonl`: dieselben Messwerte strukturiert; `summary.json`: Container-Spitzenwerte.

100 % Container-CPU entspricht einem Kern. Working Set ist Speicherverbrauch
abzüglich inaktiver Dateiseiten. GPU-Auslastung und Watt gelten für die ganze GPU;
Basalt-Prozess-VRAM wird gesondert zugeordnet. Host-Werte enthalten andere Programme.
Netzwerkzähler erfassen Unix-Sockets nicht. Vor Container-/Modellwechsel die Aufnahme
beenden. Fehlende GPU-Werte werden nicht als null Verbrauch ausgegeben.
`metrics/overlay.txt` eignet sich als lokale Textquelle für die spätere Videoaufnahme.
Metriken enthalten keine Prompts, Antworten oder Zugangsdaten.

## Lebensdauer, lokale Daten und Prüfung

```sh
python3 -B deploy/thinkrail/manage.py status
python3 -B deploy/thinkrail/manage.py down
# Nach beendetem Tunnel: ausdrücklich neues Paar erzeugen und starten.
python3 -B deploy/thinkrail/manage.py new-pair
```

Ein verbrauchter Code wird nicht automatisch reaktiviert. `up` meldet nach einem
vollständigen Tunnelabbruch den nötigen neuen Code. `new-pair` bewahrt die lokalen
Arbeitsstände und die Sperrdateien; `down` löscht keine Volumes. Kein `down -v` verwenden.

k-lani-link und Basalt erzeugen keine Anwendungs-/Inhaltslogs. ThinkRail speichert
seinen Agentenverlauf, Arbeitsstände und Terminalzustand **lokal** im privaten
Home-Volume. Seine Log-/Cache-Verzeichnisse liegen auf tmpfs, stdout/stderr sowie
Docker-Logs sind deaktiviert. `--no-analytics` allein genügt nicht, um alle
ThinkRail-Meldungen ins Internet zu verhindern; die Netzisolation ist maßgeblich.
Host, Browser, Swap und Docker-Daemon bleiben außerhalb dieser Anwendungsgarantie.

Der separate synthetische Langkontexttest läuft über die echte k-lani-link-Verbindung:

```sh
docker compose --env-file .thinkrail-test/compose.env -f deploy/thinkrail/compose.yaml \
  exec -T client python -B - < deploy/thinkrail/long-context-check.py
```

Ein erfolgreicher Coder-Test verarbeitete 260.963 Eingabetokens und erinnerte die
Markierung vom Anfang korrekt. Das ist ein Funktionstest mit repetitivem Text,
kein Qualitäts-/Durchsatzbenchmark. Lange Agentensitzungen mit Kontextkompaktierung
und ein Vergleich beider Packs bleiben eigenständige Prüfungen.

Der geprüfte Nightly-Agentenlauf mit Pi 1.4.0 lieferte drei Thinking-Abschnitte,
drei Werkzeugaufrufe und eine anschließend unabhängig geprüfte Datei (zwei Zeilen,
40 Bytes). Die Ressourcenaufnahme umfasst diesen Lauf sowie einen separaten
Langkontexttest. Rust: 22 Tests bestanden, ein externer Vermittlertest bewusst
standardmäßig übersprungen; Clippy, WASM-Protokollbuild und Cargo-Audit ohne Befund.
Die fünf Basalt-Datenschutztests bestanden im aus öffentlichen Quellen gebauten Image.

Für eine prüfbare Quellweitergabe aus dem Repository-Stamm:

```sh
python3 -B deploy/source-bundle.py
```

Das Paket unter `.release/` enthält nur ausgewählte Quellverzeichnisse mit
SHA256-Manifest, keine privaten Laufzeitdaten, Modelle oder Messungen.
Die GitHub-Workflowdatei prüft den GPU-freien Teil; sie veröffentlicht nichts.
Der eigene Code verwendet [BSL 1.1](../../LICENSE); die kommerzielle
1.x-Nutzerlizenz und Freigrenze stehen in [LICENSING.md](../../LICENSING.md).

Für die Video-Demo gibt es einen [Ablauf und einen Startprompt](DEMO.md).
Ein Video wird durch diese Skripte noch nicht aufgenommen.
