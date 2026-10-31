# Codex mit Basalt über k-lani-link

Codex verwendet seinen eigenen Agentenkern, die lokalen Werkzeuge und die
Responses API. k-lani-link überträgt Anfragen und SSE-Daten unverändert über den
zusätzlich verschlüsselten WebRTC-Kanal. Basalt führt nur die Inferenz aus;
Datei- und Shell-Werkzeuge laufen auf dem Codex-Rechner.

## Providerprofil erzeugen

Zuerst Basalt und beide k-lani-link-Endpunkte gemäß [Einrichtungsanleitung](../../QUICKSTART.md)
einrichten. Der lokale API-Endpunkt des Clients muss erreichbar sein.
`model` muss exakt einer ID aus dessen `GET /v1/models` entsprechen. Für die hier
vorhandene Coder-Datei meldet Basalt `Qwen3.8-Flash-Next IQ3_XXS`.

```sh
cargo build --release --locked
target/release/k-lani-link codex-profile \
  --config pair/client.toml \
  --output "$HOME/.codex/link.config.toml" \
  --model 'Qwen3.8-Flash-Next IQ3_XXS' --context 32768

codex --profile link
```

Den Ausgabepfad an den jeweiligen Rechner anpassen. Das übergeordnete Verzeichnis
muss existieren. Bestehende Dateien werden nicht überschrieben. Neben dem Profil
entsteht `link.config.models.json`. Der geprüfte Pilot verwendet 32768
Kontexttokens; der Katalog darf höchstens die tatsächlich konfigurierte
Basalt-Kapazität angeben. Ein größerer Kontext ist separat zu prüfen.

Das Profil enthält absolute Pfade zum Modellkatalog, zur k-lani-link-Binärdatei und
zur privaten Client-Konfiguration. Es enthält keinen API-Schlüssel. Codex ruft
`codex-token --config …` selbst als Credential-Helfer auf; dessen Ausgabe ist
eine Pipe, keine Logdatei. Direkte Ausgabe an ein interaktives Terminal wird
abgelehnt. Kein OpenAI-API-Key und kein ChatGPT-Login sind für diesen Provider nötig.

Websuche, Analytics, Feedback, Apps, Plugins, Unteragenten, Memories und
Shell-Snapshots sind im Profil deaktiviert. Normale Modellanfragen und Streams
werden nicht automatisch wiederholt. Der native Codex-Werkzeugsandbox bleibt auf
`workspace-write` mit deaktiviertem Netzwerkzugriff eingestellt.

## Lokale Desktop-App

Die Desktop-App liest ihre Einstellungen aus der Benutzer-Konfiguration, unter
Linux/macOS `~/.codex/config.toml`, unter Windows
`%USERPROFILE%\.codex\config.toml`. Der CLI-Aufruf `--profile link` schaltet
eine bereits laufende Desktop-App nicht um.

Für die Desktop-Aktivierung die Einstellungen aus dem erzeugten Profil in diese
Konfiguration übernehmen, vorhandene gleichnamige Schlüssel ersetzen und die App
neu starten. Dabei `model`, `model_provider`, `model_catalog_json` und die übrigen
obersten Felder außerhalb aller TOML-Tabellen halten. Andere bestehende
Einstellungen erhalten. Das k-lani-link-Programm und die Client-Datei müssen auf
diesem Rechner unter den eingetragenen Pfaden liegen; ein WSL-Pfad ist kein
nativer Windows-Pfad.

Das Modell erscheint über den Katalog als `<Modell-ID> · k-lani-link`. Im neuen
lokalen Chat Provider und Modell prüfen. Der Code für eine neue Geräte-Kopplung
ist getrennt vom lokalen API-Schlüssel für Codex. Nach endgültigem Tunnelverlust
einen neuen Kopplungscode auf beiden Geräten einrichten.

Das Modellprofil steuert Modellanfragen. Es sperrt nicht pauschal den gesamten
Desktop-Prozess, bereits konfigurierte MCP-Server oder sonstige App-Verbindungen.
Der unten geprüfte Containeraufbau hat eine zusätzliche, tatsächliche Netzsperre.
[OpenAI Docs: Provider einrichten](https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway),
[Grenzen und Voraussetzungen der Gateway-Anbindung](https://learn.chatgpt.com/docs/enterprise/gateway-compatibility).

## Codex ohne Internetroute

Für einen isolierten API-Verbraucher kann der k-lani-link-Client ausschließlich auf
einem Unix-Socket lauschen:

```sh
mkdir -m 700 /absolute/private/link-api
target/release/k-lani-link client --config pair/client.toml \
  --api-socket /absolute/private/link-api/api.sock
```

Der Ordner muss dem Benutzer gehören und privat sein. Der Socket erhält 0600;
es entsteht kein TCP-Listener auf dem Host. Ein bereits vorhandener Pfad wird
nicht überschrieben. Nach einem Prozessabsturz einen verbliebenen Socket erst
entfernen, wenn der zugehörige Prozess sicher beendet ist.

`bridge.py` läuft im Codex-Container mit `network=none` und stellt darin
`127.0.0.1:8787` bereit. Es reicht Bytes zum eingehängten Unix-Socket durch.
Der Bridge-Prozess verweigert den Start, wenn ein Netzwerkinterface außer
Loopback vorhanden ist. `LINK_API_SOCKET` und `LINK_API_PORT` passen den
Socketpfad bzw. Loopback-Port an. Die Bridge startet den als Argument angegebenen
Codex-Befehl und schreibt keine Verkehrslogs. Der Codex-Container benötigt nur
seinen Arbeitsordner, Profil, Katalog, Credential-Helfer und Client-Konfiguration;
keinen Docker-Socket und keine ChatGPT-Anmeldedaten.

Dieser Aufbau ist im folgenden Test vollständig ausgeführt. Er schützt auch dann
vor Internetzugriffen des Codex-Prozesses, wenn eine Anwendungseinstellung übersehen
wurde. Die Desktop-Oberfläche selbst ist nicht Bestandteil dieses Containers.

## Reproduzierbare Agententests

Voraussetzung: die Images aus [der Docker-Anleitung](../README.md), das gebaute
k-lani-link-Programm und eine native Linux-Codex-Binärdatei. Der npm-Startwrapper
`codex.js` genügt hier nicht; die Binärdatei liegt bei npm unter dem zum Betriebssystem
passenden `@openai/codex-…/vendor/…/bin/codex`. Die Beispielpfade sind zu ersetzen.

```sh
# Responses, echte Codex-Werkzeuge und SSE mit vorgegebenen Modellausgaben:
python3 -B deploy/codex/smoke.py \
  --codex-binary /absolute/path/to/codex --model-file /absolute/path/to/model.basalt --mode app-server

# Derselbe Agentenlauf mit echter GPU-Inferenz:
python3 -B deploy/codex/smoke.py \
  --codex-binary /absolute/path/to/codex --model-file /absolute/path/to/model.basalt --mode app-server --real
```

`--mode cli` prüft `codex exec`. `--model-file` wählt die lokale Basalt-Datei;
`--context` und `--timeout` ändern Testkontext bzw. Zeitlimit. Alle verwendeten
Prompts und Dateien sind synthetisch. Die Tests erzeugen eigene Codes und
Container, trennen den Vermittler vor dem Agentenlauf und entfernen anschließend
ihre eigenen Container und temporären Dateien. Basalt bleibt ohne Netzwerk,
Anwendungslogs oder Request-Historie. Der echte GPU-Test begrenzt Basalt auf
16 GiB Host-RAM ohne zusätzlichen Container-Swap.

Der Agent liest `seed.txt`, erzeugt `result.txt` mit `apply_patch`, liest die Datei
zur Kontrolle und liefert eine Schlussantwort. Der App-Server-Test prüft zusätzlich
den Modellkatalog und gestreamte Thinking-Events. Der Test verwendet ausschließlich
synthetische Dateien in einem Container mit schreibgeschütztem Root-Dateisystem
und ohne Netzwerk. Nur dort wird Codex' innere Sandbox für den Test abgeschaltet;
das native Providerprofil behält seine Werkzeug-Sandbox.

Geprüft: CLI 0.147.0 mit Basalts Testmodell; Desktop-Kern/App-Server 0.159.0 sowohl
mit Testmodell als auch mit `Qwen3.8-Flash-Next IQ3_XXS` auf einer RTX 5070 Ti
bei 300 W Powerlimit. Das ist ein Funktionstest, kein Durchsatzbenchmark.
Der echte Lauf lieferte 14 abgeschlossene Elemente und 579 Thinking-Delta-Events.
Die normale Desktop-Oberfläche und eine Verbindung zwischen Firmen- und Heimnetz
sind noch separat zu testen. Lange Sitzungen mit automatischer Kontextkompaktierung
sind nicht durch diesen kurzen Agententest abgedeckt.
