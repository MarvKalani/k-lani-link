# Audio8, Ming und Nemotron über k-lani-link

Dieses Compose-Profil startet Audio8-TTS-Realistic-Air und Ming 3.0 TTS 0.5B
mit NVIDIA-CUDA sowie Nemotron 3.5 ASR 0.6B über NeMo-Speech.cpp auf der CPU.
Es ist für Linux amd64 mit Docker Compose ab 2.30 und NVIDIA Container Toolkit
vorgesehen. Die Dienste verwenden UID 1000 und zwei private Unix-Sockets.
Beide Container laufen ohne Netzwerk und mit schreibgeschütztem Root-Dateisystem.

## Lokale Dateien und Start

Lege deine Stimmreferenz als PCM-WAV und das zugehörige UTF-8-Transkript in ein
privates Verzeichnis außerhalb des Checkouts. Verwende für die beiden Dateien
`reference.wav` und `reference.txt` Modus 0600 und für das Verzeichnis 0700.
Der Betreiber kann seine eigene Referenz einsetzen.

Kopiere [`.env.example`](.env.example) in eine private Datei und setze die
absoluten Pfade. Verwende vollständige, lokal lesbare Modellverzeichnisse.
Bei Hugging-Face-Snapshots müssen die Dateien innerhalb der Mounts auflösbar sein.
Setze `LINK_SPEECH_MING_SOURCE` auf den lokalen Checkout des Ming-Inferenzcodes.
Die übrigen Modellpfade zeigen auf [Audio8][audio8], [Ming 0.5B][ming] und
das NVIDIA-GGUF `nemotron-3.5-asr-streaming-0.6b.q8_0.gguf`.

Lege `LINK_SPEECH_SOCKET_DIR` als Verzeichnis mit Modus 0700 an. UID 1000 muss
die Modell-Dateien lesen und in das Socket-Verzeichnis schreiben können.

```sh
docker compose --env-file "$HOME/.config/k-lani-link/speech.env" \
  -f deploy/speech/compose.yaml up -d --build
```

Der Build lädt die Python-Abhängigkeiten und das offizielle NeMo-CPU-Release
mit festgelegtem SHA-256. Die Inferenz verwendet ausschließlich die Mounts.
Modelle, Stimmreferenz, Transkript und Schlüssel gehören in lokale Verzeichnisse.
Die Socket-Dateien sind 0600. Die OpenAI-kompatible URL bleibt durch den
bestehenden Link-API-Key geschützt. Die Speech-Container erhalten diesen Key
und die Pairing-Daten nicht.

## Auf dem Modellrechner

Ersetze vorhandene `speech = true`- und `transcriptions = true`-Einträge in
der privaten `host.toml`, wenn Compose die bisherigen Sprachdienste übernehmen soll.
Die beiden neuen Einträge verwenden dieselbe OpenAI-URL wie Chat und andere Modelle.

```toml
[[backends]]
upstream = "http://localhost"
upstream_socket = "/absolute/private/path/to/speech-sockets/tts.sock"
speech = true
allow_unverified_backend = true

[[backends]]
upstream = "http://localhost"
upstream_socket = "/absolute/private/path/to/speech-sockets/nemo.sock"
transcriptions = true
allow_unverified_backend = true
```

`allow_unverified_backend` überspringt für diese beiden Dienste das Basalt-spezifische
Gesundheitsprofil. Der Container und die Dateirechte isolieren die Dienste.
Verwende für einen containerisierten Link-Host zusätzlich
[`compose.host.yaml`](compose.host.yaml) nach den übrigen Compose-Dateien und
setze beide Socket-Pfade in der TOML auf `/run/link-speech/tts.sock` und
`/run/link-speech/nemo.sock`. Starte den Host danach mit dem aktuellen Link-Build neu.

## Auf dem Client-Rechner

Für lokale Verarbeitung vor dem Tunnel setze in der privaten `client.toml`
einen oder beide Einträge. Das geht auch mit einem anderen OpenAI-kompatiblen
Audio-Dienst auf Loopback. Eine optionale Backend-Authentifizierung steht als
`upstream_api_key` im jeweiligen Eintrag.

```toml
[local_audio.speech]
upstream = "http://localhost"
upstream_socket = "/absolute/private/path/to/speech-sockets/tts.sock"

[local_audio.transcriptions]
upstream = "http://localhost"
upstream_socket = "/absolute/private/path/to/speech-sockets/nemo.sock"
```

Bei einem Link-Client in Docker ergänze [`compose.client.yaml`](compose.client.yaml)
und verwende wieder `/run/link-speech/` als Socket-Verzeichnis in der TOML.
Starte den nativen `k-lani-link client` mit dieser Konfiguration neu.
Ein konfigurierter Audio-Endpunkt funktioniert auch ohne aktiven Tunnel.
Ist sein lokaler Dienst ausgefallen, gibt die Route einen Fehler zurück.
Chat und unkonfigurierte Audio-Routen gehen über den Tunnel. Eine konfigurierte
Route ist für alle Modell-IDs lokal. `GET /v1/models` ergänzt die lokalen Modelle
um die übrigen Modelle vom Host und markiert lokale Einträge mit `"local": true`.
Für getrennte Rechner benötigt der Client eigene lokale Modelle und eine eigene
Stimmreferenz. Der native Client unterstützt Unix-Sockets auf Unix und
lokale HTTP-Dienste auf Loopback auch unter Windows.

## Verwenden

Lies die Modell-IDs aus `GET /v1/models`. Verwende für TTS
`audio8-tts-realistic-air` oder `ming-3.0-tts-0.5b` mit `voice: "local-reference"`
über `POST /v1/audio/speech`. Beide liefern PCM-WAV. Schicke eine WAV-Datei
als Multipart-Upload mit `model`, `language=de` und `file` an
`POST /v1/audio/transcriptions`. Der Link erlaubt 8 MiB pro Anfrage.

Audio8 und Ming nutzen die GPU nacheinander. Der Prozess gibt den GPU-Speicher
nach jeder Anfrage frei. Unter 4 GiB freiem GPU-Speicher antwortet der Dienst
mit HTTP 503. Nemotron läuft auf der CPU.

[Audio8][audio8] steht unter Apache-2.0, [Ming][ming] unter MIT.
[NeMo-Speech.cpp][nemo] steht unter Apache-2.0 und [Nemotron][nemotron] unter
OpenMDW 1.1. Die NeMo-Lizenzen und Notices sind im Image unter
`/opt/nemo/share/licenses/nemo-speech` enthalten.

## Im Browser-Chat

Der Chat kann die Spracherkennung des Browsers nutzen. Im lokalen Modus prüft
er das Sprachpaket und verlangt mit `processLocally` die Verarbeitung auf
diesem Gerät. Der Online-Modus fragt vor dem Mikrofonstart, ob der Browser
Audio an seinen Anbieter schicken darf. Das Diktat landet als bearbeitbarer
Text im Eingabefeld. Beim Vorlesen wählt der Chat eine Stimme derselben
Sprache, die der Browser mit `localService` als lokal meldet.

Die Browserfunktionen gelten für den geöffneten Chat. Für IDEs und andere
Programme stehen die Audio-Routen des nativen Client-Gateways zur Verfügung.
Ein späteres ONNX-Modell im Browser kann die Spracherkennung oder Vorlesestimme
im Chat ersetzen. Für die Auswahl zählen deutsche Sprachqualität, Modellgröße
und gemessene Latenz auf dem Zielgerät.

[Web Speech API](https://webaudio.github.io/web-speech-api/) ·
[ONNX Runtime Web](https://onnxruntime.ai/docs/tutorials/web/)

[audio8]: https://huggingface.co/Edge0/Audio8-TTS-Realistic-Air
[ming]: https://huggingface.co/inclusionAI/Ming-3.0-tts-0.5B
[nemo]: https://github.com/NVIDIA/NeMo-Speech.cpp/releases/tag/v0.3.0
[nemotron]: https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b
