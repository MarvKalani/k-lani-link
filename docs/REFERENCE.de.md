# k-lani-link — technische Referenz

> Deutsche Übersetzung der [technischen Referenz](../REFERENCE.md); bei Abweichungen gilt
> der englische Text. Produktüberblick und Kaufinformationen: [README](../README.de.md).

k-lani-link verbindet KI-Anwendungen auf Rechner A mit einem konfigurierten Backend
auf Rechner B. Die Standorte sind frei wählbar. Basalt ist die getestete
Referenzintegration; für andere lokale OpenAI-kompatible Server gibt es das
explizite Opt-in `allow_unverified_backend = true`. Die geprüfte Docker-Vorlage
bleibt standardmäßig bei aktivierter Datenschutzprüfung.

## Verbindliche Netzregeln

- `relay = "disabled"` ist der Standard. Die Binary ergänzt keine STUN- oder
  TURN-Server. Die Startbeispiele wählen `stun.marvins.app` ausdrücklich aus, einen
  kleinen STUN-Dienst des Autors zum Ausprobieren, ohne Verfügbarkeitszusage;
  Google-Alternativen bleiben bis zur Aktivierung durch den Betreiber Kommentare.
  Es gibt keinen automatischen Wechsel zu Google oder einem anderen Datenrelay.
- ICE/STUN-Server und Signaling-URL sind frei konfigurierbar. Der STUN-Dienst
  erfährt Netzwerkadressen. Der Vermittler sieht den öffentlichen Raumbezeichner,
  Verbindungsmetadaten und verschlüsselte Pakete. SDP, ICE-Adressen und
  Zertifikat-Fingerprints liegen innerhalb dieser Verschlüsselung. Beide Dienste
  erhalten keine KI-Anfragen oder Antworten.
- Nach dem Aufbau wird der Signaling-HTTP-Client verworfen. Keine dauerhafte
  Verbindung und keine Hintergrundabfragen beim Vermittler während der Sitzung.
  Erst bei einer neuen Kopplung mit neuem Code wird Signaling erneut benötigt.
  Ein bereits geöffneter Datenkanal bricht auch eine noch ausstehende Bestätigung
  des Vermittlers ab. Tests überwachen dafür die tatsächlichen TCP-Verbindungen
  bei absichtlich um 30 Sekunden verzögerten Antworten beider Signaling-Backends.
- Bei deaktiviertem TURN werden TURN-URLs in der Konfiguration und Relay-Kandidaten
  im empfangenen SDP abgelehnt. Scheitert P2P, wird der Fehler sichtbar; k-lani-link
  wechselt nicht heimlich auf einen Servertransport.
- TURN ist nur mit `relay = "allowed"` oder `"required"` und eigenen, ausdrücklich
  eingetragenen Zugangsdaten nutzbar. Der DataChannel bleibt auch dabei zwischen
  den Geräten verschlüsselt. Es wird kein TURN-Dienst mitgeliefert oder kostenlos
  zur Verfügung gestellt. Abrechnung/Zugangskontrolle liegt beim TURN-Betreiber;
  ein bezahlter TURN-Dienst ist nicht Teil des MVP. Der separate Kauf-Link
  betrifft ausschließlich die Softwarelizenz.
- Lokale HTTP-Strecken sind auf Loopback beschränkt. Der Gateway benötigt einen
  eigenen API-Schlüssel. Basalt-Zugangsdaten bleiben im Host. Umgebungs-Proxys und
  HTTP-Weiterleitungen werden nicht verwendet.
- k-lani-link erzeugt keine Anwendungs-, Verbindungs- oder Fehlerlogs und keine
  Telemetrie. Transport-/HTTP-Logging ist auch in Debug-Builds herauskompiliert;
  `RUST_LOG` kann es nicht einschalten. Erfolgreiche Starts bleiben still.
  Bedienfehler, eine ungültige Konfiguration, ein bereits verbrauchter Kopplungscode
  und eine abgelehnte Gegenstelle erzeugen genau eine feste Zeile auf stderr, die
  weder Schlüssel noch Konfigurationswerte enthält (Konfigurationsfehler nennen
  höchstens die Zeilennummer; Bedienfehler nennen ein Argument, wiederholen aber nie
  einen übergebenen Wert). Verbindungs-, Transport- und E/A-Fehler bleiben still
  und werden als Exitcode bzw. HTTP-Antwort gemeldet. `check` meldet `configuration OK`,
  `--help`/`--version` liefern die angeforderte Ausgabe. Der ausdrücklich
  aufgerufene Credential-Helfer `codex-token` liefert nur den API-Schlüssel über
  eine Pipe an Codex; er verweigert die Ausgabe an ein interaktives Terminal.
- Vor jeder Upstream-Anfrage muss `/health` das private Inferenzprofil bestätigen.
  Fehlt es, erhält der Backend-Server keinen Prompt. Das verhindert versehentlichen
  Betrieb ohne Datenschutzmodus; eine Selbstangabe ersetzt keine Prüfung des Codes.
  Nur mit `allow_unverified_backend = true` in der `host.toml` entfällt diese Prüfung;
  `check` und `host` warnen dann. Das ist für andere lokale Server gedacht, deren
  Logging- und Historienverhalten der Betreiber selbst geprüft hat.

## Bauen

Rust 1.96.0. Das Repository lässt sich ohne private Nachbarprojekte bauen.

Der native WebRTC-Adapter liegt im aktuellen MVP in `src/peer.rs` von k-lani-link;
das HTTP-/Streaming-Protokoll liegt in `k-lani-link-protocol`. Der native MVP ist
eigenständig; die Chat-/Dateitransport-Schnittstellen des Connectors werden nicht
verwendet. Eine gemeinsame Transportintegration bleibt ein separater Schritt.

```sh
cargo build --release --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo audit
```

Die ausführbare Datei liegt unter `target/release/k-lani-link` (unter Windows
mit `.exe`). Für die folgenden Beispiele kann stattdessen
`cargo run -- <Befehl>` verwendet werden.

## Ein Geräte-Paar anlegen

```sh
k-lani-link init --dir pair --signaling-backend ice-exchange \
  --signaling-url https://signal.example.invalid --stun stun:stun.marvins.app:3478
```

Der Befehl erstellt ein **neues** Verzeichnis mit `host.toml`, `client.toml`,
`pair-code.txt` und `signaling-token.txt`, überschreibt keine bestehenden Dateien und zeigt keine
Schlüssel im Terminal an. Zufallswerte stammen vom Betriebssystem. Unter Unix
werden Verzeichnis/Dateien mit 0700/0600 angelegt; unter Windows gelten die ACLs
des Benutzerverzeichnisses. Die Dateien gehören nicht in ein Git-Repository.

`host.toml` bleibt auf Rechner B, `client.toml` wird über einen bereits
vertrauenswürdigen Weg auf Rechner A übertragen. Der zufällige `pair_key` hat **43 URL-sichere
Base64-Zeichen** aus 32 Zufallsbytes. Die **ersten 12 Zeichen bestimmen automatisch
den Raum** und sind für den Vermittler sichtbar. Damit bleiben 184 geheime Bits;
ein kurzer, selbstgewählter PIN wird nicht unterstützt. Das gespeicherte Feld
`room` ist nur abgeleitete Information und wird beim Laden neu berechnet.

Alternativ können beide Geräte unabhängig mit demselben Code eingerichtet werden:

```sh
k-lani-link new-code --out code.txt
# code.txt über einen vertrauenswürdigen Weg auf das zweite Gerät übertragen.
# Auf jedem Gerät mit einem eigenen neuen Konfigurationspfad ausführen:
k-lani-link pair --config node.toml --code-file code.txt \
  --signaling-url https://signal.example.invalid --stun stun:stun.marvins.app:3478
```

Ohne `--code-file` erfolgt die Eingabe verdeckt im Terminal. Ein gesonderter
Raumname ist nicht nötig. Der lokale `api_key` für OpenCode/Cline ist ein anderer,
länger gültiger Schlüssel als der einmalige Kopplungscode.

### Authentifizierung und einmalige Kopplung

Signaling-Nachrichten werden mit XChaCha20-Poly1305 verschlüsselt und authentifiziert.
Raum, Rollen, Sitzung, Zeitfenster, SDP und Fingerprints werden geprüft. Falsche
oder manipulierte Pakete beenden den Versuch ohne automatische Wiederholung.
Beide Geräte leiten über HKDF-SHA256 getrennte Sitzungsschlüssel pro Richtung ab.
Auch jedes Paket im DataChannel wird mit XChaCha20-Poly1305 geschützt;
Sequenznummern verhindern Wiedereinspielen, Vertauschen und Spiegeln von Paketen.

**Erst nach gegenseitiger verschlüsselter Schlüsselbestätigung im geöffneten
Tunnel wird der Code verbraucht.** Fehlgeschlagene Kopplungen verbrauchen ihn nicht.
Die Sitzung läuft danach mit ihren Sitzungsschlüsseln weiter. Ein neuer vollständiger
Tunnel benötigt einen neuen Code; der alte Code wird dabei nicht reaktiviert.
Wer den geheimen Code besitzt, kann sich als Gegenstelle ausgeben. Der sichere
Austausch des Codes gehört deshalb zur Authentifizierung.

Verbrauchte Codes werden durch leere Dateien mit gehashtem Namen unter
`.link-key-state` neben der Konfiguration gesperrt, alternativ unter
`pairing_state_dir`. Das ist notwendiger Authentifizierungszustand, kein Chatlog;
es enthält keine Prompts, Antworten oder Klartextcodes. Dieses Verzeichnis muss
privat und dauerhaft erhalten bleiben. Nach einem Prozessabsturz kann eine
Reservierung bestehen bleiben; dann einen neuen Code verwenden. Die zusätzliche
Inhaltsverschlüsselung besitzt keinen eigenen ephemeren DH-Schlüsselaustausch;
ihre Sicherheit wird nicht als unabhängige Forward Secrecy zugesichert.

### Vorhandenes ice-exchange verwenden

Die Vermittlungsadresse durch den vorhandenen Dienst ersetzen. Die STUN-Anbieter
wählt der Betreiber selbst; in **beiden** Konfigurationen sind diese Werte frei änderbar:

```toml
signaling_backend = "ice_exchange"
signaling_url = "https://signal.example.invalid"
relay = "disabled"

[[ice_servers]]
urls = [
  "stun:stun.marvins.app:3478",
  # "stun:stun.l.google.com:19302",
  # "stun:stun1.l.google.com:19302",
  # "stun:your-stun-host.example:3478", # Vor dem Aktivieren ersetzen.
]
```

Nur bei gewünschten Anbietern die Kommentarzeichen entfernen. Für einen anderen
Anbieter die aktive URL ändern oder entfernen. Das [ICE-Beispiel](../examples/ice-servers.toml) liegt auch
separat vor. Die Vermittlungsadresse und der eigene alternative STUN-Hostname sind
Platzhalter; die Binary enthält keine eingebauten Standardserver. Der erzeugte
`signaling_token` wird nur vom optionalen Mailbox-Backend verwendet.

Der Adapter implementiert den dortigen v3-Ablauf: `POST /{room}` zur Rollenvergabe,
`POST /{room}/offer` mit Long-Poll auf die Antwort, `GET /{room}/offer` und
`POST /{room}/answer`. Das vorhandene `sdp`-Feld enthält eine feste SDP-Hülle mit
dem verschlüsselten Kopplungspaket als eigener Zeile; `ice` bleibt leer. Der
Vermittler muss dieses Feld nur unverändert weiterreichen. Das echte SDP samt
gesammelten ICE-Kandidaten steckt im verschlüsselten Paket. Ungesicherte zusätzliche
ICE-Arrays vom Vermittler werden nicht übernommen. Der Server benötigt keine Änderung.

Der Vertrag wird gegen einen lokalen Nachbau dieser Endpunkte mit echten
WebRTC-Peers getestet. Auch ein Verbindungstest über einen ausdrücklich ausgewählten bestehenden
ice-exchange-Vermittler mit zwei lokalen Peers und synthetischen Daten war erfolgreich. Der Server wurde
dabei nicht verändert; ein Test zwischen Firmen- und Heimnetz steht noch aus.

### Optionaler lokaler Test-Vermittler

Für einen vollständig lokalen Versuch:

```sh
k-lani-link init --dir local-pair
k-lani-link signal --token-file local-pair/signaling-token.txt
```

Die so erzeugte Konfiguration verwendet `signaling_backend = "mailbox"` und
`http://127.0.0.1:8788`. Der Dienst vermittelt nur zeitlich begrenzte SDP-Nachrichten
in verschlüsselten Hüllen und hat keine Inferenzroute. Für entfernten Zugriff muss er hinter HTTPS stehen.
Dieser MVP-Dienst ist für private Tests ausgelegt, nicht für ein öffentliches
Mehrkundenangebot.

Wenn beide Geräte für einen lokalen Test auf demselben Rechner laufen, müssen
ihre Konfigurationen verschiedene `pairing_state_dir`-Verzeichnisse verwenden.
Im Docker-Aufbau sind Host-Volume und Client-Verzeichnis bereits getrennt.

## Host und Client starten

Basalt wird separat mit dem mitgelieferten `--privacy`-Modus gestartet und muss auf
`http://127.0.0.1:8080` oder dem konfigurierten Unix-Socket erreichbar sein.
Die Docker-Konfiguration erzwingt Datenschutzmodus, deaktivierte Bilderkennung
und den privaten Unix-Socket.
Der Modus unterbindet `engine.log`, Logpuffer, Request-Historie, Hardware-Telemetrie
und Diagnoseoberfläche. Diagnose-/Dump-Flags werden abgelehnt. Details und
Update-Patch: [Basalt-Integration](../integrations/basalt/README.md).
Bei einem anderen Port `upstream` in `host.toml` ändern. Falls Basalt einen
API-Schlüssel verlangt, nur im Host `upstream_api_key` setzen.

```sh
# Backend-Rechner B
k-lani-link check --config pair/host.toml
k-lani-link host --config pair/host.toml

# Client-Rechner A
k-lani-link check --config pair/client.toml
k-lani-link client --config pair/client.toml
```

Client-Endpunkt: `http://127.0.0.1:8787/v1`.
API-Schlüssel: Wert von `api_key` aus `client.toml`.
Modell-ID: tatsächlicher Basalt-Modellname aus `GET /v1/models`.
Der zusätzlich vorhandene `GET /health` benötigt ebenfalls den API-Schlüssel
und meldet `connected`, `recovering`, `pairing_rejected`, `new_code_required`
sowie die konfigurierte Relay-Regel.

**Startreihenfolge.** Die Reihenfolge spielt keine Rolle. Wer zuerst startet, wartet
und versucht es alle zwei Sekunden erneut, bis der andere da ist; Kopplungsnachrichten
gelten 120 Sekunden, bei jedem Versuch entsteht eine neue. Mit der eingebauten Mailbox
und echten Prozessen geprüft: Host zuerst und Client 20 Sekunden später sowie Client
zuerst und Host 75 Sekunden später (länger als die 60 Sekunden Antwortwartezeit des
Clients). Bei einem externen ice-exchange-Dienst gelten dessen eigene Wartefenster.
Während des Wartens wird nichts ausgegeben.

### Prüfen, Beenden und weitere Optionen

`k-lani-link check --config …` validiert die Datei ohne Netzwerkzugriff und meldet
`configuration OK` oder den Grund. Ist die Datei für Gruppe oder Andere zugänglich,
erscheint eine Warnung (`chmod 600`).

`host` und `client` beenden sich bei SIGINT **und SIGTERM** (`docker stop`, systemd)
ordentlich: Die Kopplungsreservierung wird freigegeben und ein privater API-Socket
entfernt. Laufende API-Anfragen haben nach der Stoppanforderung drei Sekunden Zeit zum
Beenden; eine lange Generierung hält das Herunterfahren nicht auf. Nach SIGKILL oder Absturz kann die Reservierung bestehen bleiben; der Start
meldet das und es wird ein neuer Code benötigt.

Zwei Schalter in der `host.toml`/`client.toml`, standardmäßig aus und von `init` nicht
geschrieben:

- `allow_unverified_backend = true` (nur Host): Datenschutzprofil des Backends nicht prüfen.
- `include_loopback_candidate = true`: nur `127.0.0.1` als ICE-Kandidat anbieten; beide
  Endpunkte müssen dann auf demselben Rechner laufen. Nur für Tests gedacht.
- `resume_seconds = N` (nur HTTPS, auf beiden Endpunkten gleich setzen, höchstens
  `request_timeout_seconds`): Nach einer verlorenen Sitzung hält der Host jede laufende
  Antwort N Sekunden, und das Gateway setzt sie in der nächsten Sitzung lückenlos und
  ohne Doppelungen fort. Die Anfrage wird nie zweimal ausgeführt. Solange niemand liest,
  liest der Host nach einem Antwortfenster (64 KiB) nichts mehr vom Backend; das Modell
  wird also gebremst statt vorauszurechnen, ob es dabei pausiert, entscheidet das
  Backend. Kommt in N Sekunden keine Fortsetzung, wird die Backend-Anfrage abgebrochen.
  `0` (Standard) lässt laufende Anfragen sofort scheitern. Ein noch laufender Upload
  wird nie fortgesetzt.

### Mehrere Backends über einen Tunnel

`host.toml` kann neben `upstream` bis zu acht weitere OpenAI-kompatible Backends nennen:

```toml
upstream = "http://127.0.0.1:8080"          # Basalt auf dem Host-Rechner

[[backends]]
upstream = "http://192.168.1.20:1234"       # LM Studio auf dem Mac mini
allow_unverified_backend = true             # LM Studio hat kein Datenschutzprofil
allow_network_upstream = true               # anderer Rechner im LAN
```

Der Host fragt jedes Backend nach `/v1/models` (30 Sekunden zwischengespeichert, nach
einem Modellwechsel neu) und schickt jede Chat- oder Responses-Anfrage an das erste
Backend, das ihr `model` listet; bei gleicher ID gewinnt `upstream`. Ein Modell, das kein
Backend listet, geht wie bisher an `upstream`. Ein gelistetes Modell weicht nie auf ein
anderes Backend aus: Besteht sein Backend die Datenschutzprüfung nicht oder ist es nicht
erreichbar, scheitert die Anfrage. `GET /v1/models` liefert die zusammengeführte Liste
aller Backends, die die Prüfung bestanden haben, `upstream` zuerst; der Verbindungstest im
Bedienfeld zeigt also alle Modelle.

Ein weiteres Backend muss auf Loopback liegen, außer mit `allow_network_upstream = true`:
dann ist eine private IP-Adresse (RFC 1918, Link-Local, IPv6 Unique-Local oder
`100.64.0.0/10`, das Tailscale nutzt) über `http` oder jeder Host über `https` erlaubt.
Hostnamen über einfaches `http` werden abgelehnt, weil sonst DNS entscheidet, wohin
Prompts gehen. Über einfaches `http` laufen Prompts **unverschlüsselt** durch dieses Netz;
`check` warnt davor und vor ungeprüften Backends. `max_concurrent_requests` gilt für alle
Backends gemeinsam, und Modellverwaltung samt Drain betrifft nur `upstream`; ein
Modellwechsel wartet also auch auf Anfragen an anderen Backends.

## Unterstützte APIs

Der Tunnel trägt eine feste Routentabelle (`ROUTES` im Protokoll-Crate, mit ihm
versioniert). Request- und Response-Bodies laufen unverändert durch; ändert ein Backend
sein eigenes API-Format, muss hier nichts angepasst werden. Ein Host ohne eine neuere Zeile
antwortet mit 404.

| API | Routen | Typische Clients |
| --- | --- | --- |
| OpenAI | `GET /v1/models`, `POST /v1/chat/completions`, `/v1/responses`, `/v1/embeddings` | OpenCode, Cline, Continue, Codex mit eigenem Provider |
| Anthropic Messages | `POST /v1/messages`, `/v1/messages/count_tokens` | Claude Code über `ANTHROPIC_BASE_URL` |

Ob ein Backend eine Route anbietet, entscheidet das Backend; prüfe deine Version von
llama.cpp, LM Studio oder Basalt. `/v1/embeddings` schickt Text (etwa Code zur
Indizierung) genauso ans Backend wie ein Chat.

## Claude Code mit eigenem Modell

Das unveränderte Claude Code auf das lokale Gateway zeigen lassen und ihm den
k-lani-link-API-Key als Gateway-Zugang geben:

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
export ANTHROPIC_AUTH_TOKEN=<k-lani-link-API-Key>
export ANTHROPIC_MODEL=<Modell-ID deines Backends>
export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
export CLAUDE_CODE_MAX_CONTEXT_TOKENS=<Kontextfenster deines Modells, z. B. 131072>
```

`ANTHROPIC_AUTH_TOKEN` (oder `ANTHROPIC_API_KEY`) unbedingt setzen: Nur dann schickt Claude
Code diesen Key statt eines gespeicherten claude.ai-Logins. Das Gateway nimmt
`Authorization: Bearer` und `x-api-key` an, leitet beide nie weiter und lehnt alles andere
ab. k-lani-link kontaktiert Anthropic nie, nutzt oder übermittelt keine
Claude-Abo-Zugangsdaten und ist kein Weg, ein Claude-Abo außerhalb von Anthropics eigenen
Apps zu nutzen. Anfragen erreichen nur dein eigenes Backend.

Das Gateway akzeptiert Claude Codes `?beta=true` auf diesen Routen und verwirft es; der
Host setzt `anthropic-version: 2023-06-01` selbst und leitet Claude Codes
`anthropic-beta`-Liste nicht weiter, die lokale Backends nicht brauchen. Die
Modellerkennung (`GET /v1/models?limit=1000`) wird nicht bedient; das Modell also
ausdrücklich setzen. Anthropics Dokumentation beschreibt Gateways über
`ANTHROPIC_BASE_URL`, unterstützt aber nicht, Claude Code an Nicht-Claude-Modelle zu
leiten; Funktionen, die Claude-Modelle voraussetzen, können fehlen.

## OpenCode und Cline

**OpenCode 1:** `examples/opencode.json` als Ausgangspunkt verwenden und die
Modell-ID ersetzen. Über `/connect` → `Other` den Provider `link` anlegen
und den lokalen k-lani-link-API-Schlüssel hinterlegen. Danach das Modell unter
`/models` auswählen. `@ai-sdk/openai-compatible` nutzt Chat Completions;
`@ai-sdk/openai` ist für einen ausdrücklich gewünschten Responses-Client gedacht.
[Offizielle Provider-Dokumentation](https://opencode.ai/docs/providers/).
Die Vorlage erlaubt nur `link`, deaktiviert Sharing und verlangt eine
bewusste Entscheidung für Web- und Shell-Werkzeuge. Lokale Sitzungsverläufe sind zulässig. Keine dieser Einstellungen ist
für sich ein Nachweis, dass sämtliche Plugins/Telemetrie ohne ausgehende Zugriffe laufen.

**OpenCode 2:** `examples/opencode-v2.json` verwendet das V2-Schema mit
`providers`, Provider-Policies und `reasoning_content`. Die isolierte
[Docker-Variante](../deploy/opencode/README.md) wurde mit 2.2.0, Basalt und echten
Werkzeugaufrufen getestet. Die Modell-ID ermittelt deren Starter über k-lani-link.

**Cline:** Provider `OpenAI Compatible`, Base URL
`http://127.0.0.1:8787/v1`, k-lani-link-API-Schlüssel und die Basalt-Modell-ID
eintragen. Kontextfenster und Modellfähigkeiten entsprechend dem tatsächlich
geladenen Modell einstellen.
[Offizielle Konfiguration](https://docs.cline.bot/provider-config/openai-compatible).
In Cline die Telemetrie deaktivieren und im Editor zusätzlich
`telemetry.telemetryLevel = "off"` setzen; eine separate Editor-Vorlage liegt in
`examples/vscode-privacy-settings.json`. Der geprüfte Cline-Code unterscheidet
normale und erforderliche Telemetrieereignisse: Letztere hängen am Editor-Schalter,
weshalb der Cline-Schalter allein nicht alle Ereignisse sperrt.
[Cline-Telemetrieimplementierung](https://github.com/cline/cline/blob/775d0398039977ca5447ed39c1a7f178df69aa87/apps/vscode/src/sdk/sdk-telemetry.ts).
Andere Erweiterungen, MCP-Server, Updates und bewusst ausgeführte Netzwerkwerkzeuge
werden dadurch nicht pauschal kontrolliert. Eine ausgehende Netzsperre für den
Agentenclient mit erlaubtem lokalem k-lani-link-Zugriff ist die stärkere Kontrolle.

Das Gateway reicht Modellparameter, Tool-Definitionen, Tool-Ergebnisse und
Streaming-Deltas unverändert durch. Welche Funktionen funktionieren, entscheidet
Basalt mit dem geladenen Modell. OpenCode 2.2.0 wurde mit einer realen Basalt-Instanz und Werkzeugaufrufen getestet;
ein eigener Cline-Agentenlauf ist hier noch nicht geprüft.

**Thinking:** `reasoning_content` bei Chat Completions und benannte
`response.reasoning_text.delta`-Events bei Responses bleiben im laufenden Stream
erhalten. Die Tests prüfen, dass erste Reasoning-Daten vor dem Ende der Antwort
ankommen. Ob der Client sie sichtbar ausklappt, hängt von dessen Oberfläche ab.
k-lani-link erzeugt keine erfundenen Denktexte.

**Codex:** Die Anbindung über die Responses API ist mit der CLI 0.147.0 und dem
Desktop-Kern 0.159.0 getestet. Der Desktop-Kern hat auch mit dem echten lokalen
Qwen-Modell Dateien gelesen, per `apply_patch` bearbeitet, geprüft und den Lauf
abgeschlossen; Thinking wurde gestreamt. Basalt und Codex liefen dabei in getrennten
Containern ohne Internetroute, der Vermittler war nach der Kopplung abgeschaltet.
`codex-profile` erstellt ein Providerprofil samt Modellkatalog, `codex-token`
liefert den lokalen API-Schlüssel ausschließlich über eine Pipe an Codex.
Ein privater Unix-Listener ist über `client --api-socket` verfügbar.
[Einrichtung und reproduzierbarer Agententest](../deploy/codex/README.md).
Die Desktop-Oberfläche selbst wurde nicht umgeschaltet; andere App-Verbindungen
werden durch ein Modellprofil allein nicht gesperrt.
[OpenAI Docs: eigene Anbieter](https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway).

## API und Grenzen des MVP

Basalt bleibt ein separat aktualisierbarer Prozess. k-lani-link enthält keinen
Basalt-Fork und keinen Modellcode. Ein anderer OpenAI-kompatibler Inferenzserver
kann über `upstream` und seine Modell-ID eingebunden werden: mit einem geprüften
Datenschutzadapter oder ausdrücklich mit `allow_unverified_backend = true`.
Diese Freigabe überspringt die Datenschutzprüfung des Backends und lässt sich
nicht mit unserer Basalt-Modellverwaltung kombinieren.
[Konfiguration und Grenzen](../deploy/BACKENDS.md#connect-another-local-server).
Neue Serverversionen
sollten zuerst Modellliste, Streaming, Tool-Calling und Abbruch gegen diese
Schnittstelle bestehen, bevor sie die funktionierende Version ersetzen. Der MVP
lädt selbst keine Inferenzserver-Updates herunter.

Unterstützte Routen: `GET /v1/models`, `POST /v1/chat/completions` und
`POST /v1/responses`. JSON- und SSE-Antworten sowie HTTP-Statuscodes werden
weitergegeben. Es gibt keine Übersetzung zwischen diesen API-Formaten.
Basalts Responses-Implementierung ist zustandslos; dies ist keine vollständige
Implementierung aller OpenAI-Dienste.

Standardmäßig eine aktive Anfrage pro Geräte-Paar. `max_concurrent_requests`
(1–8) und `max_queued_requests` (0–16) begrenzen Streams und Warteschlange;
Überlast erhält HTTP 429. Die [Modellverwaltung](../deploy/managed/README.md)
braucht einen getrennten Bedienerschlüssel. Maximal 8 MiB
Anfragegröße. Antworten werden in 8-KiB-Stücken und mit begrenztem Sendefenster
übertragen. Standardmäßig gilt ein Gesamtlimit von 600 Sekunden, einstellbar über
`request_timeout_seconds` (10–3600). Abbruch des lokalen HTTP-Clients stoppt den
Upstream-Aufruf.

Bei kurzem Paketverlust bleiben Stream, Sitzungsschlüssel und Sequenznummern
erhalten. Nach etwa 8 Sekunden ohne Peer-Aktivität meldet `/health` `recovering`;
ICE hat danach weitere 15 Sekunden zur Wiederherstellung derselben Verbindung.
Kehrt sie rechtzeitig zurück, werden ausstehende Daten geordnet weitergeliefert,
ohne neue Generierung oder Vermittlerkontakt. Puffer bleiben begrenzt und liegen
im RAM. Das gesamte Anfragezeitlimit und Zeitlimits des API-Clients gelten weiter.

Nach etwa 23 Sekunden ohne Peer-Aktivität, einem geschlossenen DataChannel oder
einem sonstigen endgültigen Transportfehler endet die laufende Antwort mit einem
Fehler; die Upstream-Anfrage wird abgebrochen. Es gibt keine automatische Wiederholung
der Generierung. Ein neuer Tunnel braucht einen neuen Code auf beiden Geräten.
Netz-/IP-Wechsel oder Prozessneustarts benötigen häufig einen solchen Neuaufbau;
eine Wiederaufnahme über einen neu ausgehandelten Tunnel ist noch nicht implementiert.

Der native UDP-Transport verwendet aktuell IPv4. IPv6, komfortables Pairing,
Dienstinstallation, signierte Windows-Releases
und eine Browser-Erweiterung sind nachfolgende Ausbauschritte. TURN-Einstellungen
sind implementiert; ein echter TURN-Server/Firmen-NAT wurde im lokalen Test nicht
verwendet.

## WASM und Browser-Erweiterungen

```sh
cargo build -p k-lani-link-protocol --target wasm32-unknown-unknown --release
```

Der eigenständige Protokollkern baut als WASM und exportiert über wasm-bindgen
Validierung, Verschlüsseln/Öffnen der Signaling-Nachrichten sowie eine zustandsbehaftete
`WasmSecureSession` für Inhaltsverschlüsselung und Wiedereinspielschutz. Das ist noch kein
fertiger Browser-Client. Ein Browser-Adapter kann `RTCPeerConnection` verwenden;
der native Prozess stellt den lokalen HTTP-Server bereit.

Eine Chrome-/Firefox-Erweiterung kann Kopplung, Status und Browser-Chat anbieten.
Für einen HTTP-Endpunkt, den OpenCode/Cline aufrufen, braucht sie weiterhin einen
nativen Helfer, etwa über [Chrome Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)
oder [Firefox Native Messaging](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_messaging).
WASM ist keine Garantie gegen Virenscanner- oder Reputationswarnungen. Für die
native Verteilung sind nachvollziehbare Builds und Codesignierung sinnvoll;
auch signierte neue Programme können zunächst SmartScreen-Warnungen erhalten.
[Microsofts Hinweise](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation).

## Datenschutz

k-lani-link hält den Inferenzverkehr zwischen den beiden gewählten Geräten. Der
Client-Rechner sendet dabei Daten an den gewählten Backend-Rechner. Speicherorte,
Zugriffsrechte, Aufbewahrung, Zulässigkeit dieser Verarbeitung und Einstellungen
von Basalt/OpenCode/Cline bleiben zu klären. Verschlüsselung ist eine technische
Maßnahme und keine DSGVO-Zertifizierung.
[EU-Kommission: Grundsätze der DSGVO](https://commission.europa.eu/law/law-topic/data-protection/information-business-and-organisations/principles-gdpr_en).
