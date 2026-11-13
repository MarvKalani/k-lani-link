# Basalt-Datenschutzprofil

`privacy.patch` ergänzt den in `BASE_COMMIT` genannten Basalt-Stand um `--privacy`
und einen optionalen Unix-Socket. Die Änderungen betreffen den Python-Server und
seine Tests, keine CUDA-Rechenkerne. Basalt bleibt ein separat austauschbarer
Prozess unter seiner ursprünglichen MIT-Lizenz; die Lizenz liegt daneben.

Der mitgelieferte Docker-Build lädt den festgelegten öffentlichen Quellstand und
wendet den Patch an. Der Entrypoint erzwingt `--privacy --vision off` und einen
privaten Unix-Socket, verwirft stdout/stderr und deaktiviert Core-Dumps sowie CUDA-Diskcache.

Der private Modus:

- öffnet keine `engine.log` und erstellt keine Logpuffer;
- behält keine abgeschlossenen Request-Historien/Diagnosewerte oder Antwortvorschauen;
- verweigert Dokumente aus URLs und lokalen Dateipfaden und umgeht den PDF-Cache
  für direkt mitgesendete Dokumente;
- startet keinen Hardware-Telemetrie-Sampler (dieser ist im normalen Basalt lokal,
  kein Beleg für eine externe Telemetrieübertragung);
- deaktiviert Weboberfläche, Metrics, Logs, Status und Verwaltungsrouten;
- verweigert Debug-/Dump-Schalter, Vision und zusätzliche Browserfreigaben;
- liefert über `/health` das Profil `link_privacy` für die k-lani-link-Zugangssperre.

Erlaubt sind Modellliste, Health und die Chat-/Responses-/Messages-Inferenzrouten.
Temporäre Rechenzustände und KV-Caches im RAM/VRAM bleiben Teil der Inferenz;
dies ist keine Zusage, dass Speicher physisch nach jeder Antwort überschrieben wird.
Tokenizerdateien aus dem Modell können lokal gecacht werden. In Docker liegen
diese Dateien auf tmpfs und der Server hat gar kein Netzwerkinterface.

## Updates

Der aktuelle Quellstand ist [Basalt v0.3.3](https://github.com/jesdga95/basalt/releases/tag/v0.3.3)
mit Commit `2729e2288f59e01836ddd7fcc630810ada98995a`. Das Image
vermerkt den Quellstand als `io.k-lani.basalt.revision`. Wer
`BASALT_SOURCE_CONTEXT` überschreibt, setzt `BASALT_REVISION` auf den dazugehörigen Commit.

Diese Version liest `.basalt`-Format 2, separate Expertenprofile und nur noch
Konfigurationsdateien im Format 2 (`"version": 2` mit einer `models`-Liste, siehe
[`deploy/basalt.json`](../../deploy/basalt.json)); ältere Dateien überführt Basalts
`tools/migrations/config_v1_to_v2.py`. Ältere Packs
müssen mit dem offiziellen Migrationstool in eine neue Datei überführt werden.
Bei eigenen oder beschnittenen Modellen muss ein Profil mit passender Geometrie
vorhanden sein; der [Supervisor](../../deploy/managed/README.md#custom-basalt-expert-profiles)
kann es ausdrücklich über `expert_profile` auswählen. Die Integration verändert
keine Modellgewichte und aktiviert keine Bildverarbeitung.

[Validierung vom 13. November 2026](validation-20261113.json) dokumentiert den
v0.3.3-Build und den GPU-Test über den verschlüsselten Link.
Die [Validierung vom 7. November](validation-20261107.json) dokumentiert
Modellwechsel mit Coder und Mellum am vorherigen Basalt-Stand.

Auf einem frischen Checkout des genannten Basalt-Commits:

```sh
git apply --check /path/to/k-lani-link/integrations/basalt/privacy.patch
git apply /path/to/k-lani-link/integrations/basalt/privacy.patch
```

Bei neueren Versionen erst die Anwendbarkeit und Tests prüfen. Der Starter fällt
bei fehlendem `--privacy` nicht auf normalen Betrieb zurück. k-lani-link prüft das
Profil vor jeder Anfrage, auch nach einem Backend-Neustart. Die Profilantwort
ist eine Schutzmaßnahme gegen Fehlkonfiguration, keine kryptografische Attestation.

```sh
.venv/bin/python -B -m unittest serve.tests.test_privacy -q
```

Die Tests verwenden synthetische Texte, prüfen Reasoning-/Antwort-Streaming,
Unix-Socket-Betrieb, leere Log-/Historienpuffer und lautlosen Prozessstart.
Die nativen Coder-/NVFP4-Skripte wurden zusätzlich im Lazy-Modus geprüft, ohne
eine GPU-Inferenz auszulösen. `basalt-probe` ist ein Hardware-/Benchmark-Werkzeug,
kein interaktiver Chat-Test.

### Basalt unabhängig vom Link aktualisieren

Der [Basalt-Workflow](../../.github/workflows/basalt.yml) prüft täglich den
neuesten stabilen Upstream-Tag. Manuell lässt sich auch ein bestimmter Tag
auswählen. Änderungen an der Integration werden gegen `BASE_COMMIT` geprüft.
Der Build bindet den Upstream-Commit fest, wendet beide Patches an und führt
die Basalt-Server-Suite aus. Für GPU-Tests gilt der separate lokale Test mit
einem vorhandenen Modell.

Der Workflow baut und prüft ein Linux-amd64-Image.
Auf dem Default-Branch veröffentlicht CI bei einem geplanten Lauf, nach
Änderungen am Backend oder mit der manuellen Option `publish` nach
`ghcr.io/marvkalani/k-lani-link-basalt`. Der Image-Tag enthält den
Basalt-Tag, dessen Commit und einen Hash der Build-Eingaben. Ein
Patch-Konflikt stoppt den Build vor der Veröffentlichung.

Solange Inferenz-API, Privacy-Profil und Supervisor-Schnittstelle
kompatibel bleiben, kann der Link mit der installierten Client-Version
weiterlaufen. Setze in der privaten Compose-Umgebung `BASALT_IMAGE` auf
das geprüfte Image, vorzugsweise per Digest. Für ein privates GHCR-Paket
benötigt der Backend-Rechner Leserechte und `docker login ghcr.io`.
Verwende alle Compose-Dateien
deiner Installation. Die folgenden Befehle ersetzen nur den Backend-Dienst.

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml pull basalt
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --no-deps --no-build basalt
```

Für einen lokalen Build setzt du `BASALT_SOURCE_CONTEXT` und
`BASALT_REVISION` auf denselben geprüften Commit und führst
`docker compose build basalt` mit denselben Compose-Dateien aus.
Laufende Anfragen können beim Austausch abbrechen. Das vorherige
Image bleibt über seinen Tag oder Digest als Rückfall verfügbar.

## Explicit opt-in numeric measurements

`numeric-metrics.patch` applies after `privacy.patch`. With `--privacy` and
`--privacy-metrics-file /absolute/tmpfs/path`, Basalt writes a private 0600
snapshot of numeric counters, model ID and context capacity every 0.5 seconds.
It contains no prompt, response, token strings, request IDs or conversation
history. The file is replaced atomically and removed on normal shutdown.
Live decode uses the engine's two-second rate window; the last request's prefill,
decode and first-token timings come from engine counters, not SSE event counts.
HTTP `/metrics`, `/v1/status` and management endpoints remain disabled.

In the production Compose file this is off by default; set
`LINK_NUMERIC_METRICS=1` explicitly to enable it. The measurement lab enables
it explicitly. Its host-side sampler reads only this bounded file through the
Docker archive API and discards snapshots older than five seconds or from a
previous container process. The agent never receives the Docker socket.
Recording CSV/JSON is a separate opt-in action. This is an explicit exception
for numeric measurements, not a claim that the measurement run stores no data.

```sh
git apply /path/to/k-lani-link/integrations/basalt/numeric-metrics.patch
.venv/bin/python -B -m unittest serve.tests.test_privacy serve.tests.test_numeric_metrics
```

## Managed model processes

The optional supervisor (`k-lani-link backend supervise`, set up in `deploy/managed/`) replaces the Basalt process behind
its private inference socket. Basalt's generic administration API stays disabled.
The supervisor controls only preconfigured model IDs, requires the privacy health
profile after each load, and verifies the actual parallel slots and context.
The health response now includes the actual `parallel` value for this check.
The catalog's Boolean `answer_room` setting maps to Basalt v0.3.3's
`--answer-room auto` or `--answer-room 0`. Earlier images use the Boolean flags.
The patch also keeps the answer-room calculation silent in privacy mode, and since
`5d28effb` it refuses Basalt's disk tier for conversations (`--conversation-cache-dir`,
`--conversation-cache-disk-mib`), which would write conversation state to disk.
The integration patches touch Python serving and tests. The suite uses temporary
executable mock engines inside the build stage. The production container uses
non-executable temporary filesystems.
