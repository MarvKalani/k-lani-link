# OpenCode mit MXC unter Windows / OpenCode with MXC on Windows

## Deutsch

Die [VM-Abnahme vom 2. November 2026](TESTED.md) dokumentiert die bestandenen
Prüfungen und den echten Modelllauf.

Dieser Starter führt das unveränderte OpenCode 1.19.5 im Windows-Prozesscontainer
von Microsoft MXC 1.0.1 aus. Docker, WSL, Hyper-V und Adminrechte werden zum
Starten nicht benötigt. Das ist ein Prozesscontainer mit demselben Windows-Kernel,
keine zusätzliche VM.

1. Das [k-lani-link-Kontrollfeld](../../README.de.md) starten, koppeln und auf
   **Verbunden** warten.
2. `Start-OpenCode.cmd` doppelklicken und **einen Projektordner** auswählen.
3. In OpenCodes Terminaloberfläche arbeiten. Mit `Ctrl+C` beenden.

Alternativ in Windows PowerShell 5.1 oder neuer:

```powershell
.\Start-OpenCode.ps1 -Project 'C:\Projekte\Kunde'
.\Start-OpenCode.ps1 -Project 'C:\Projekte\Kunde' -Model 'Modell-ID aus dem Kontrollfeld'
.\Start-OpenCode.ps1 -Project 'C:\Projekte\Kunde' -Prompt 'Lies README.md und erstelle eine lokale Demo.'
.\Start-OpenCode.ps1 -Project 'C:\Projekte\Kunde' -CheckIsolation
```

Der Ordner muss bereits existieren. Laufwerke, Netzwerkfreigaben, Systemordner,
das gesamte Benutzerprofil sowie Junctions im gewählten Pfad werden abgewiesen.
Das Kontrollfeld liefert Adresse, API-Schlüssel und Modelle automatisch. Bei einem
abweichenden privaten Kontrollfeldordner gibt es `-PanelDirectory`.

Der Starter leitet die gemeldete Kontextgröße als mögliche Ausgabeobergrenze
weiter und setzt keinen zusätzlichen 16k-/64k-Deckel. Denken und finale Antwort
teilen sich den nach der Eingabe verbleibenden Kontext. Für den verwalteten
Qwen-Backendbetrieb die aktuellen Beispielprofile mit `fit_max_tokens` verwenden.

### Was freigegeben ist

| Bereich | Zugriff aus der Sandbox |
| --- | --- |
| Gewählter Projektordner | Lesen und schreiben |
| Eigenes, temporäres OpenCode-Verzeichnis | Konfiguration, Verlauf, Cache, temporäre Dateien |
| Gepinnte Werkzeuge | Dateiinhalte nur lesen und Programme direkt starten |
| Andere Benutzerdateien und der private API-Schlüssel des Kontrollfelds | Nicht als Dateien freigegeben |
| Lokale k-lani-link-API | Genau `127.0.0.1:<API-Port>`, normalerweise `8787` |
| Andere lokale Ports, LAN und Internet | Gesperrt |

Übergeordnete Verzeichnisse dürfen für die Pfadauflösung nur abgefragt bzw.
aufgelistet werden; ihre Dateiinhalte werden dadurch nicht freigegeben.
Windows stellt zusätzlich die von MXC vorgesehenen Betriebssystemressourcen
bereit; dies ist kein Container mit einem komplett eigenen Root-Dateisystem.
OpenCode erhält seinen benötigten API-Schlüssel im Prozessumfeld. Zugangsdaten
anderer Anbieter und die übrige Host-Umgebung werden nicht übernommen. Die
Windows-Dateirechte der Projektdateien werden nicht aufgeweicht: Ein MXC-Fallback,
der Host-DACLs verändert, ist ausdrücklich abgeschaltet.

OpenCode kann die Projektdateien an dein eigenes Modell über die lokale API
senden. Der k-lani-link-Prozess außerhalb der Sandbox braucht dafür seinen
normalen Netzwerkzugang zum Modellrechner/Relay. Er bleibt von der Netzsperre des
Agenten getrennt. Der API-Endpunkt ist kein allgemeiner HTTP-Proxy.

Bei regulärem Ende entfernt der Starter sein temporäres Sitzungsverzeichnis.
Ergebnisdateien im Projekt bleiben bestehen. Nach einem Absturz oder Stromausfall
können Reste unter `%LOCALAPPDATA%\k-lani-link\mxc\sessions` liegen. Das ist keine
Zusicherung, dass Speicherinhalte oder gelöschte Dateien sicher überschrieben werden.

### Installation ohne Internet beim Kunden

Beim ersten Start ohne vorhandene Werkzeuge lädt **der Starter außerhalb der
Sandbox** die beiden offiziellen GitHub-Archive herunter: MXC etwa 517 MiB,
OpenCode etwa 78 MiB. Archiv und ausgepackte Programme werden gegen die
SHA-256-Werte in `release.json` geprüft; auch der Werkzeugcache wird bei jedem
Start geprüft. Während des Agentenlaufs bleibt das Internet gesperrt.

Für eine vollständig vorbereitete Übergabe diese Dateien aus den geprüften
Archiven in einen Ordner `tools` neben dem Starter legen:

```text
tools\wxc-exec.exe
tools\mxc_ffi.dll
tools\opencode.exe
```

Dieser Ordner wird automatisch erkannt. Alternativ `-ToolsFrom 'D:\MXC-Tools'`
angeben. Dabei ist kein Download erlaubt; fehlende oder veränderte Dateien
führen zum Abbruch. Die Lizenztexte aus `licenses` mitgeben. Der erste
Werkzeugimport kopiert die Dateien in das private Benutzerverzeichnis.

### Voraussetzungen und Grenzen

- Windows 11 x64, mindestens Build `26100.9550`/`26200.9550`, alternativ
  `26300.9550` oder `28000.3086`. Der tatsächliche MXC-Start muss die angeforderte
  Richtlinie unterstützen; es gibt keinen automatischen Wechsel auf eine
  schwächere Netzregel.
- Die Freigabe verwendet MXCs **Proxy-Modus**. Eine direkte Egress-Regel für
  `8787` sperrte im Test einen anderen lokalen Port nicht. Deshalb gibt es hier
  keine Browseroberfläche: Host-Verbindungen zu einem Server in der Sandbox
  funktionieren im Proxy-Modus auf der geprüften Windows-Version nicht.
- Microsoft bezeichnet die Variante mit einem nicht paketierten lokalen Prozess
  als *development/testing compatibility*. Die Portgrenze wird vom Betriebssystem
  gesetzt; eine MSIX-/AppContainer-Identität des API-Prozesses wird dabei nicht
  geprüft. Das ist kein fertig gehärteter MSIX-Installer und keine Zertifizierung
  für einen Kundeneinsatz. [Microsofts Netzwerkdokumentation](https://github.com/microsoft/mxc/blob/v1.0.1/docs/backends/process-container/networking.md)
- OpenCode 2.2.0 benötigt selbst im Terminal einen internen TCP-Server. Der
  strenge Proxy-Modus blockiert auch diese interne Verbindung. Dieses Profil
  verwendet deshalb die offizielle Version **1.19.5 (28. Oktober 2026)** mit
  `--mini` bzw. `run`, die ihren Server im Prozess aufrufen. Das Docker-Profil
  verwendet weiterhin 2.2.0.
- `cmd.exe` konnte im bisherigen VM-Test keine weiteren Programme starten;
  PowerShell innerhalb der Sandbox schlug ebenfalls fehl. Shell-Aufrufe sind
  deshalb im Profil deaktiviert. OpenCodes eigene
  Dateilese- und Schreibwerkzeuge sind davon getrennt. Compiler, Paketmanager und
  beliebige Shell-Werkzeuge sind mit diesem Profil nicht pauschal zugesagt.
- Ein lokaler Administrator und Software außerhalb der Sandbox bleiben außerhalb
  dieser Schutzgrenze. Die Freigabe eines Ordners schützt dessen Inhalt nicht vor
  Änderungen durch den Agenten; vorher ein Backup oder eine Versionsverwaltung
  verwenden.

`-CheckIsolation` führt echte Datei- und TCP-Versuche mit derselben Richtlinie aus:
Schreiben im Projekt, schreibgeschützte Werkzeuge, gesperrte Datei außerhalb,
authentifizierte Modellabfrage, ein tatsächlich lauschender anderer lokaler Port
für IPv4/IPv6 und direkte externe Verbindungen. Eine nicht bestandene Prüfung
endet mit einem Fehlercode. Die Prüfung löscht ihre eigenen Projektmarker wieder.

## English

Run `Start-OpenCode.cmd`, select an existing project directory and use OpenCode in
the terminal. First start and pair the k-lani-link control panel and wait for
**Connected**. No Docker, WSL, Hyper-V or elevation is needed to launch this
Windows process sandbox. It shares the Windows kernel.

Use `-Project`, optionally `-Model` or `-Prompt`, or `-CheckIsolation` as shown
above. `-PanelDirectory` selects a non-default private control-panel directory.
The launcher reads the local address/key and discovers the models automatically.
The launcher passes the advertised context as the possible output ceiling,
without an additional 16k/64k cap. Thinking and the final answer share the context
left after the input. Use the current managed Qwen examples with `fit_max_tokens`
on the backend; an older backend may reject a request that exceeds its remaining context.
Only the selected project and private temporary session data are writable.
Tools are read-only, other user files are not granted, and external network
access is limited to the exact local API endpoint, normally `127.0.0.1:8787`.
The separate k-lani-link transport retains the network access it needs to reach
your own model/relay. The model can receive the selected project data.

This profile pins **OpenCode 1.19.5 (28 October 2026)**: its `--mini` and `run`
modes use an in-process server. OpenCode 2.2.0 requires internal TCP even for the
terminal, which this strict MXC proxy policy also blocks. The Docker profile
continues to use 2.2.0.

Use Windows 11 x64 build `26100.9550` / `26200.9550` or the other supported builds
listed above. MXC must enforce the requested policy; the launcher does not relax
it if enforcement is unavailable. Host DACL mutation fallback is disabled.

First use downloads pinned official MXC/OpenCode archives **outside** the sandbox.
For offline installation, supply `wxc-exec.exe`, `mxc_ffi.dll` and `opencode.exe`
in a sibling `tools` directory or use `-ToolsFrom`. The launcher verifies both
archive and extracted-file hashes, and checks cached tools on every launch.
Include the supplied `licenses` directory when redistributing the binaries.

This profile deliberately uses MXC proxy mode and a terminal UI. In the tested
Windows build, direct egress rules did not close other host loopback ports;
proxy mode did, but prevented host browsers from reaching a contained server.
Microsoft labels the unpackaged local proxy arrangement development/testing
compatibility; it does not authenticate the API process by package identity.
There is no production MSIX installer or compliance certification here.

Shell child processes and PowerShell failed in the earlier VM tests, so shell
tools are disabled in this profile. Direct
OpenCode file tools and direct executable starts are separate; this profile does
not promise arbitrary compiler/package-manager support. Ancestor directories permit path metadata/enumeration, not file-content reads.
Windows system resources remain available as defined by MXC. Host administrators are outside this boundary.
Temporary session state is removed on normal exit; crashes may leave private
files under `%LOCALAPPDATA%\k-lani-link\mxc\sessions`. This is not secure erasure.

`-CheckIsolation` verifies real file access, an authenticated model query, live
IPv4/IPv6 loopback listeners and external TCP connections using the same policy.
A failed check returns a non-zero exit code.
