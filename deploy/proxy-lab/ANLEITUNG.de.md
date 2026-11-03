# Firmenproxy-Labor für k-lani-link

Das Labor betreibt den echten Client, Host und Relay in getrennten Docker-Containern.
Dazwischen sitzt mitmproxy als Ausgangsproxy mit echter TLS-Inspektion. Nur das Modell
ist ein synthetischer Testserver, damit sich Antworten, Reihenfolge und doppelte
Ausführungen eindeutig prüfen lassen.

**[Veröffentlichte Ergebnisse und belegte Produktaussagen](RESULTS.md)**

## Starten

Voraussetzungen: Linux x64, Python ab 3.11, Docker Engine, OpenSSL sowie die Rust-Toolchain
des Repositories mit musl-Ziel und musl-C-Compiler. Es werden keine produktiven Modelle,
Zugangsdaten oder GPU-Geräte eingebunden.

```sh
rustup target add x86_64-unknown-linux-musl
python3 -B deploy/proxy-lab/run.py
```

Der Runner baut die aktuelle Software, erstellt das Testimage und führt alle Szenarien
aus. Die Vorbereitung darf Images und Build-Abhängigkeiten herunterladen. Während der
Tests sind alle Docker-Netze intern; es werden keine Ports am Rechner veröffentlicht.

Einzelne Szenarien lassen sich gezielt wiederholen:

```sh
python3 -B deploy/proxy-lab/run.py --cases inspected_https,json_only,blocked_quic
```

`--skip-build` verwendet bewusst die bereits gebaute musl-Binärdatei; ihr Hash steht im
Bericht. `--keep-private-state` behält die temporären Testschlüssel für lokale Fehlersuche.
Im Normalbetrieb werden diese gelöscht. Testcontainer und Testnetze werden auch bei
Fehlern oder einem abgefangenen Abbruch entfernt. Die Testimages bleiben als Build-Cache.

## Was geprüft wird

- Der Arbeitsplatz erreicht ausschließlich den Proxy, nicht Relay oder Modell direkt.
- Die Kopplung per Code und eine vollständige gestreamte Antwort funktionieren durch TLS-Inspektion.
- Unbekannte bzw. gesperrte Relay-Domain, fehlende Unternehmens-CA, falscher Zertifikatsname
  und abgelaufenes Relay-Zertifikat verhindern die Verbindung.
- Explizite Basic-Proxyanmeldung sowie HTTP/1.1 funktionieren.
- Abgewiesenes PUT führt automatisch zu POST; abgewiesene Binärdaten führen zu JSON-POST.
- Pufferung, verlorene Schreibbestätigung und Proxy-Neustart führen nicht zu einer doppelten Modellausführung.
- Entzug der Domainfreigabe oder des Relay-Tokens blockiert neue Arbeit.
- Manipulierte verschlüsselte Datensätze werden abgewiesen.
- Ein eigener QUIC-Test verwirft UDP-Pakete tatsächlich und prüft den automatischen TCP-Fallback.

Der QUIC-Test ist vom expliziten Proxy getrennt, weil die Anwendung `http3` und
`proxy_url` nicht gleichzeitig zulässt. Nur ein kurzlebiger Helfer darf dazu mit
`NET_ADMIN` die Regeln **im Netzwerk-Namensraum des Testclients** setzen. Docker verwaltet
seine temporären Bridge-Netze; die UDP-Sperre wird nicht in der Host-Firewall gesetzt.

Beim Token-Entzug wartet der Test höchstens 40 Sekunden auf die eindeutige `401`-Antwort
und den Status „Zugriff verweigert“. Er sendet bereits während des Wiederherstellungsfensters
eine Anfrage und prüft, dass sie das Modell nicht erreicht. Die beobachtete Zeit steht
im Bericht; ein bloßer Timeout zählt nicht als bestandener Token-Entzug.

## Bericht und Aussagekraft

Der Ausgabeordner wird beim Start angezeigt:

```text
.release/proxy-lab-<UTC>-<ID>/REPORT.md
.release/proxy-lab-<UTC>-<ID>/report.json
```

Der Bericht enthält jeden Test mit seinen Einzelbedingungen, Antwortzählern, Protokollen,
Versions- und Datei-Hashes. Ein Szenario gilt nur dann als bestanden, wenn die erwartete
Wirkung tatsächlich beobachtet wurde. Ein Image-Build allein zählt nicht als Test.

Die Auditdateien enthalten keine vollständigen URLs, Header, Tokens, Kopplungscodes oder
Inhalte. Der Inspektor sucht nach synthetischen Markierungen, auch nach dem Entpacken von
JSON/Base64. Deren Abwesenheit ist ein Regressionstest und kein unabhängiges Kryptografie-Audit.

Das Ergebnis belegt das Verhalten in diesem Docker-/mitmproxy-Labor. Es ist keine Abnahme
für ein bestimmtes Firmennetz, dessen URL-Reputationsdatenbank, PAC, NTLM/Kerberos oder
Windows-Sicherheitssoftware. Dafür folgt ein Pilotlauf mit dem echten Gateway und der
nativen Anwendung. [Aufbau und vollständige Testmatrix](README.md).

## Vorlage für eine freigegebene Nutzung

Die IT legt Relay-Domain, erlaubten Modellrechner, Nutzergruppe, Proxyadresse und
Zertifikatsbundle fest. Individuelle Tokens müssen entzogen werden können. Eine
Sperre der Domain muss für sämtliche HTTP-Darstellungen wirksam bleiben.

TLS-Inspektion sieht äußere HTTP-Header und Metadaten, einschließlich Relay-Zugangstokens;
Logs müssen entsprechend konfiguriert werden. Prompts und Antworten bleiben zusätzlich
verschlüsselt. Eine verlangte Inhaltsprüfung muss an einem freigegebenen Endpunkt vor
der Verschlüsselung oder nach der Entschlüsselung stattfinden.
