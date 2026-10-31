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

The optional supervisor in `deploy/managed/` replaces the Basalt process behind
its private inference socket. Basalt's generic administration API stays disabled.
The supervisor controls only preconfigured model IDs, requires the privacy health
profile after each load, and verifies the actual parallel slots and context.
The health response now includes the actual `parallel` value for this check.
The patch also keeps the answer-room calculation silent in privacy mode, and since
`5d28effb` it refuses Basalt's disk tier for conversations (`--conversation-cache-dir`,
`--conversation-cache-disk-mib`), which would write conversation state to disk.
No CUDA kernels or model weights are changed by these integration patches.
