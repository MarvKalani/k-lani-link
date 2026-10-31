# Sicherheitsmodell

> Deutsche Fassung von [SECURITY.md](../SECURITY.md); bei Abweichungen gilt der englische Text.

Der Kopplungscode wird über einen bereits vertrauenswürdigen Weg auf beide Geräte
gebracht. Seine ersten zwölf Zeichen bilden den öffentlichen Raumbezeichner;
184 Bits bleiben geheim. Wer den vollständigen Code kennt, kann eine Gegenstelle
authentifizieren. Der Code wird erst nach verschlüsselter gegenseitiger
Schlüsselbestätigung verbraucht. Verbrauchszustand muss dauerhaft erhalten bleiben.

Signaling und Nutzdaten sind zusätzlich zu WebRTC/DTLS mit XChaCha20-Poly1305
geschützt. Sitzungsschlüssel sind richtungsgebunden; Paketnummern verhindern
Wiedereinspielen und Umordnen. Diese zusätzliche PSK-Schicht hat keinen eigenen
ephemeren DH-Austausch. Es gibt keinen Klartext-Fallback und keinen automatischen
TURN-Dienst.

Der Vermittler kann Verbindungsmetadaten sehen und Verbindungen blockieren.
Nach dem Tunnelaufbau wird seine Verbindung geschlossen. Netzwerkunterbrechungen
können innerhalb derselben Sitzung überbrückt werden; ein neuer Tunnel benötigt
einen neuen Code. Abgebrochene Generierungen werden nicht automatisch wiederholt.

k-lani-link bietet nur die ausdrücklich zugelassenen Inferenzrouten an und prüft
vor jeder Anfrage das Datenschutzprofil des Backends. Diese Selbstangabe ist
keine kryptografische Attestation. Basalt muss mit dem enthaltenen Privacy-Patch
und der vorgesehenen Netzisolation betrieben werden. Mit `allow_unverified_backend = true`
schaltet der Betreiber die Prüfung ausdrücklich ab; `check` und der Start warnen dann,
und das Logging-Verhalten des Backends liegt vollständig in seiner Verantwortung.

Die ThinkRail-Laborumgebung beschränkt IPv4, IPv6 und DNS auf die erforderlichen
lokalen Verbindungen. Der Browserzugang bindet nur an Loopback und prüft Host/
Origin; eine CSP begrenzt die Ressourcen der Seite auf lokale Quellen. Der
optionale Ressourcenrekorder läuft auf dem Host und liest nur ausgewählte
Docker-Statistiken und Systemzähler. Der Docker-Socket wird keinem Agentencontainer
bereitgestellt.

Ein kompromittiertes Endgerät, fremde lokale Programme, bewusst freigegebene
Werkzeuge und die Host-Administration liegen außerhalb der Tunnelverschlüsselung.
Dieses MVP hat noch keine unabhängige Sicherheitsprüfung oder DSGVO-Zertifizierung.
Für Berichte zu Fehlern synthetische Reproduktionen verwenden und Schlüssel,
Prompts sowie personenbezogene Inhalte entfernen.

## WebRTC advisory review — 28 October 2026

The native dependency is the Rust `webrtc` crate **0.21.1**, pinned in Cargo.lock.
This runtime does not link Neko, Signal RingRTC or Chromium's libwebrtc. Updating
a browser does not change that Rust transport; a future browser adapter would
need its own browser-version policy.

| Report | Affected component / fix | Relevance here |
| --- | --- | --- |
| Neko CVE-2026-116679 / GHSA-5cp7-fp4j-wwcv | Neko 3.0.0–3.1.4; fixed in 3.1.5. Legacy WebRTC input authorization bypass under the documented settings. | Neko is not used. This is an application authorization defect, not a general break of WebRTC encryption. |
| Signal RingRTC CVE-2026-113380 | RingRTC before 2.72.0. Saturating the FSM queue can prevent local hang-up. | RingRTC is not used. We added a real encrypted DataChannel regression for local close and Drop while the application receive queue is full. |

Primary sources: [Neko advisory](https://github.com/m1k1o/neko/security/advisories/GHSA-5cp7-fp4j-wwcv),
[RingRTC fix](https://github.com/signalapp/ringrtc/commit/4b1b6fa53c9f30c87c8ea78e44675d4a13e1e4d3),
[CVE record](https://github.com/CVEProject/cvelistV5/blob/main/cves/2026/113xxx/CVE-2026-113380.json).
The incomplete identifier `CVE-20217951` could not be confidently matched.

`cargo audit` with the RustSec database at commit
`99670cfc0bc574433034b75876a4b791ccaa2efb` (updated 27 October 2026) reported no
known vulnerable dependencies or warnings for this lockfile. This is not proof
that the code is free of vulnerabilities. The queue regression completed with
explicit close under two seconds and remote closure under three seconds; it
also checks Drop and blocked sender termination. Existing tests cover tampering,
replay/reflection, mismatched keys, consumed codes, relay rejection, stream
cancellation and signaling disconnection. Generic backend administration routes remain denied. The two `/v1/link/` model
control routes require explicit host opt-in, a separate client-side operator key
and a private Unix supervisor socket. Only locally allowlisted IDs are accepted;
AI applications are not given the operator key or Docker socket.

The two named advisories did not require a production CVE patch in k-lani-link.
We do not describe an unrelated dependency upgrade as fixing them. Recheck
the lockfile and relevant advisories before each release.

## Optional measurements and intentional output paths

The Basalt numeric exporter is explicitly opt-in and writes only an allowlisted
counter snapshot to tmpfs. The host sampler can record CPU, RAM, GPU and token
timings when asked; it does not capture inference content. This measurement
exception is separate from disabled application logs and request history.

Both AI application containers use `network_mode: none`; the k-lani-link transport
containers deliberately have network access to establish ICE and reach the peer.
They are not containers with all egress removed. The local browser, Unix sockets,
mounted workspaces and exported documents are intentional data paths. The setup
therefore makes no absolute assertion that data cannot leave two containers.
Purchase pages are separate browser visits and have no runtime activation path.
