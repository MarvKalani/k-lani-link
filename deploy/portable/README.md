# Portable native builds

k-lani-link runs without an installer, administrator installation, bundled LLM or
additional WASM runtime. Starting the executable without arguments opens a local
browser control panel. On Windows, double-click the EXE. Explicit `client`, `host`,
pairing and operator commands remain available in PowerShell or a terminal.
An icon in the Windows taskbar or the macOS menu bar opens, pauses and quits it.

Get versioned downloads from [GitHub Releases](https://github.com/MarvKalani/k-lani-link/releases).
Use the processor architecture of your operating system:

| System | Variant | Package contents |
| --- | --- | --- |
| Windows x64 | `windows-x64` | One `k-lani-link.exe`, instructions and license notices |
| Windows ARM64 | `windows-arm64` | Native ARM64 EXE, instructions and license notices |
| macOS Apple Silicon | `macos-arm64` | Native executable, instructions and license notices |
| macOS Intel | `macos-x64` | Native executable, instructions and license notices |
| Linux x64 | `linux-x64` | Native executable, instructions and license notices |
| Linux ARM64 | `linux-arm64` | Native executable, instructions and license notices |

ZIP packages preserve the executable bit for macOS/Linux. Windows also has a
standalone `.exe` download; retain the accompanying license notices from the
release if redistributing it. Configuration and one-time pairing state are
separate private files created on your machine. No Rust, Python, Node.js or Docker
installation is needed to run the native transport itself.
The standalone Windows download has its version and architecture in the filename;
rename your chosen file to `k-lani-link.exe` to follow the commands below. ZIPs
already contain that short executable name.

## Support and validation boundaries

The Windows build statically links the MSVC runtime. macOS builds target 11.0 or
later; Linux builds use Ubuntu 22.04 (glibc 2.35). The release's build records name
the exact Git revision, Rust compiler, architecture and native runner. CI runs
the Rust tests on each native platform, including actual loopback WebRTC,
encrypted streaming, signaling shutdown, cancellation and concurrent requests
against synthetic backends. This does not prove connectivity through a particular
corporate network or test a real GPU model on every operating system.

Windows exposes its local API on authenticated loopback HTTP. Unix-socket API
listeners and the host-side model supervisor require Unix. Socket paths must be
short, especially on macOS (less than 104 bytes including the directory and
filename). Use a private directory with mode 0700. The supplied managed
Basalt Docker deployment is Linux/NVIDIA-specific. A Windows client can operate
that remote Linux model supervisor through the encrypted tunnel and its separate
operator credential. A native Windows host can use a loopback TCP backend with
the explicit [generic backend configuration](../BACKENDS.md#connect-another-local-server).
The same generic configuration is available on macOS and Linux.

**A native executable does not isolate your AI application or backend from the
Internet.** The [Docker quickstart](../../QUICKSTART.md) provides the separate
network-isolated application profile. The native program still authenticates
and encrypts the peer tunnel, binds the client API to loopback, and leaves TURN
disabled unless explicitly configured. Network access and required administrator
approval remain necessary. Do not change managed firewall rules without approval.

## Windows: connect to a remote model

For isolated OpenCode in your Windows browser, see the separate
[Docker Desktop setup](../desktop/README.md). The instructions below run the
transport natively and do not isolate the AI application's network access.

### Browser control panel

1. Download the current x64 EXE for Intel/AMD Windows, or ARM64 for Windows on ARM.
2. Double-click it. The local control panel opens in your default browser; no
   command-line arguments are needed. It listens on a random loopback-only port and
   follows the browser language (German or English) until you pick **DE** or **EN** in
   its header; that choice is kept in the panel's data directory.
3. On the model computer, `k-lani-link offer-https` prints a **relay address** and a
   **pairing code**. Type both into the panel and click **Connect**. The panel waits
   while the model computer confirms, then connects by itself; `k-lani-link host`
   must be running there. If the model computer shows a **QR code** instead
   (`offer-https --qr`), click **Scan QR code** and hold this computer's camera to it;
   without a camera, **Use a picture instead** reads a photo or screenshot of the code.
   The browser asks for the camera each time k-lani-link starts, because the panel's
   local address changes with every start.
4. When the panel shows **Connected**, it lists the three values for your AI app,
   each with a **Copy** button: base URL, local API key and model ID. Add an
   OpenAI-compatible provider in the app and paste them. The model ID comes from a
   model-list request through the real encrypted tunnel; no prompt is sent.

### Several model computers at once

Below the three values, **Model computers** lists every computer the panel is paired
with and its state. **+ Add another model computer** pairs one more by code or QR code
while the others stay connected (up to eight). The AI app keeps one base URL and one
API key: the models of all computers appear in one list, and each request goes only
to the computer that offers its model, never to another one, even when that one is
down. A request without a model, or with one no computer lists, goes to the first
computer. When two computers offer a model of the same name, the second one's appears
as `<name>@2` (the computer's place in the list). **Remove** ends one computer's
connection and deletes its pairing; a computer that needs a new code is removed when
you pair again from its problem notice. For several requests at once, raise
`max_concurrent_requests` (at most 8) in each host's `host.toml` to what its model
server runs in parallel, e.g. Basalt's `--parallel`; the panel's side lets eight run.

### Chat with the model

Below the three values, **New chat** opens a small window inside the panel page.
Each window is its own conversation: drag it by its title bar, resize it at the
bottom-right corner and close it with **×**. Answers stream as they are written,
with the model's thinking in a collapsible block and, under each answer, the model,
thinking level, tokens per second, token count, time to the first token and the
context size so far. **Model** and **Thinking** (default, off, low, medium, high) can
change between messages of the same chat; **Retry** asks again for the last answer
and **Stop** ends one early. When the model computer is busy with another request, a
window waits its turn. The panel sends the requests through the same local API as
your AI app, so the API key never reaches the page. A chat is named after its first
message; **✎** in the title bar, or a double-click on the title, renames it (Enter
keeps the new title, Escape the old one), and a saved chat keeps the new title.

A chat is kept only while its window is open. **Save** writes it, and every later
answer, to a private file in the `chats` folder of the panel's data directory
(`%LOCALAPPDATA%\k-lani-link\control-panel\chats` on Windows,
`~/.config/k-lani-link/control-panel/chats` on macOS and Linux, or below
`$XDG_CONFIG_HOME` when set). **Saved chats**, below **New chat** once the panel is
connected, lists them; click a title to open that chat again and continue it. The
folder is shown below the list; **Delete** there, or clicking **Saved** twice in the
window, removes the file. Nothing is saved without that click.

Everything else sits under **More options and help**: pair again with a new code,
disconnect, quit, troubleshooting, and importing a `client.toml` file from the
operator instead of a code. An imported file is validated and saved privately under
`%LOCALAPPDATA%\k-lani-link\control-panel`; container paths are normalized,
consumed-code markers kept and operator keys never imported.

On Windows, k-lani-link shows a square icon in the notification area of the taskbar
(bottom right; Windows may first place it under the arrow); on macOS it sits in the
menu bar (top right) without a Dock icon. Its colour shows the state: cyan
connected, yellow connecting, magenta a problem, grey paused or not paired; the
panel's browser tab uses the same icon. On Windows a click opens the panel and a
right-click the menu; on macOS a click opens the menu. The menu has **Open control
panel**, **Pause connection** / **Resume connection** and **Quit k-lani-link**.
Pause is offered for a pairing by code only, because an ended direct (WebRTC)
connection needs a new code; a pause lasts until you resume or the application
starts again. Linux has no icon yet; use the panel there. On macOS the menu-bar item
appears when the panel starts with its browser page, not with `ui --no-open`.

### Updates

The panel looks for a newer version twice a day and downloads it in the background.
It is installed at the next start of k-lani-link, or at once with **Update now** in the
note at the top of the panel; the panel then restarts and opens a new tab. An update is
accepted only if it carries the k-lani-link release signature and is newer than the
running version; anything else is ignored. The check sends GitHub nothing but a user
agent with the running version number. Switch it off under **More options and help**
→ **Updates**. On a host, or without the panel, run `k-lani-link update` (or
`k-lani-link update --check`); it replaces the program and the running host keeps the
old version until it is restarted. When the program's folder is read-only, the panel
says so and points to the download page instead.

Closing the browser tab leaves the connection running, and the panel reconnects by
itself when the application starts again. Double-clicking the EXE again reopens the
same panel. **Quit** stops the transport, the panel and the icon; **Disconnect**
stops only the transport. No autostart or system service is installed.

The panel uses an unpredictable local access token, checks Host and Origin, has no
external scripts, and keeps no connection logs. It stores a conversation only when
you save a chat, as described above. The
bootstrap token is removed from the browser address bar after loading. Its private
address file is deleted on orderly exit. The browser, clipboard and local user
account remain trusted. Keep the data directory private; Windows uses its inherited
NTFS permissions. Do not put it inside the agent's workspace.

The panel controls the **native client only**, keeps TURN disabled for direct
connections, and does not start Docker or manage a host/model supervisor.
The OpenCode Docker profile has its own transport and status checks. Do not run
both modes using the same one-time code. Use the Docker instructions above when
application network/file isolation is required.

### Command-line setup

Extract the appropriate ZIP into a folder under your user account, open
PowerShell there, then inspect the available commands:

```powershell
.\k-lani-link.exe --version
.\k-lani-link.exe --help
```

Create a fresh pair in your local application-data directory, using your own
existing ice-exchange v3 signaling endpoint. The example selects `stun.marvins.app`;
replace it to choose another STUN provider:

```powershell
$linkRoot = Join-Path $env:LOCALAPPDATA 'k-lani-link'
New-Item -ItemType Directory -Force $linkRoot | Out-Null
.\k-lani-link.exe init --dir "$linkRoot\pair" --signaling-backend ice-exchange --signaling-url https://signal.example.invalid --stun stun:stun.marvins.app:3478
```

The [ICE example](../../examples/ice-servers.toml) lists optional Google and custom
servers as comments. Edit the existing `urls` arrays in your TOML files or repeat
`--stun` to select multiple servers. Commented URLs are never contacted, and the
binary adds no fallback servers. TURN remains disabled.

The command writes private files and is silent on success. Keep `client.toml` on
this computer and transfer `host.toml` to the model computer through a trusted
channel. Configure and start the remote host as in the quickstart. Then run:

```powershell
.\k-lani-link.exe check --config "$linkRoot\pair\client.toml"
.\k-lani-link.exe client --config "$linkRoot\pair\client.toml"
```

Keep that terminal running. Configure your AI application with
`http://127.0.0.1:8787/v1`, the `api_key` from its private `client.toml`, and the
model ID reported by `/v1/models`. Stop with Ctrl-C. Forced termination can leave
an unused reservation behind; create a fresh code in that case. A code consumed
by an established tunnel cannot be reused after restarting the peers.

Store these files in an NTFS directory accessible only to your account and trusted
administrators, not in a shared/public folder. Windows inherits directory ACLs;
the Unix `0600`/`0700` checks are not a Windows ACL audit. If adapting an existing
Docker config, replace container-only paths with appropriate local paths. TOML
literal strings such as `'C:\Users\YOU\AppData\Local\k-lani-link\state'` avoid
backslash escapes. Do not copy a host's `model_control_socket` into a Windows config.

## macOS and Linux

Extract the matching ZIP and use Terminal. If your archive tool did not preserve
the executable bit, restore it with `chmod +x ./k-lani-link`.

```sh
./k-lani-link --version
./k-lani-link --help
```

Create a private directory and a fresh pair, then keep the client configuration
and transfer the host configuration as above:

```sh
LINK_STATE_DIR="$HOME/.local/share/k-lani-link"
mkdir -p "$LINK_STATE_DIR"
chmod 700 "$LINK_STATE_DIR"
./k-lani-link init --dir "$LINK_STATE_DIR/pair" --signaling-backend ice-exchange --signaling-url https://signal.example.invalid --stun stun:stun.marvins.app:3478
./k-lani-link check --config "$LINK_STATE_DIR/pair/client.toml"
./k-lani-link client --config "$LINK_STATE_DIR/pair/client.toml"
```

Do not delete or reset the pairing-state directory to reuse a code. For a new
session create a new code/pair; changing the model inside an existing managed
tunnel does not require a new session.

## Downloads, signatures and security software

Release checksums carry an Ed25519 signature for the k-lani-link updater.
Windows publisher signing and Apple Developer ID notarization are separate.
On Apple Silicon a linker-generated
ad-hoc signature may be present; it does not identify a verified publisher.
SmartScreen, Gatekeeper or company policy may warn or block these downloads.
Do not disable protection or bypass your organisation's policy to run them.

Check a downloaded file against the release's `SHA256SUMS`:

```powershell
Get-FileHash .\k-lani-link-*-windows-x64.exe -Algorithm SHA256
```

```sh
shasum -a 256 k-lani-link-*-macos-arm64.zip
```

Checksums detect changed files when compared with a trusted release; they do not
replace publisher signing or establish that software is harmless. Each package
includes `BUILD.json` with its binary hash and build provenance. The binaries are
not packed with an executable obfuscator, do not self-install and do not modify
firewall, antivirus or autostart settings.

For a signed distribution, the maintainer must provision Windows Authenticode
signing and Apple Developer ID credentials, sign the final binaries, notarize
the Mac distribution and then regenerate the checksums. Signing establishes
publisher identity and integrity; it does not guarantee that a scanner never
raises an alert. See [Microsoft's SmartScreen guidance](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation)
and [Apple's distribution guidance](https://developer.apple.com/developer-id/).

WASM is not an antivirus exemption. Its sandbox constrains the module through the
host runtime's permitted APIs; the networking executable still needs its normal
operating-system permissions. The existing optional protocol crate remains
WASM-buildable for future browser integrations, but the portable application
uses native Rust and does not embed a WASM runtime.
[WebAssembly security model](https://webassembly.org/docs/security/).

## Maintainer build and packaging

The `Portable builds` workflow builds and tests all six architectures using the
pinned Rust toolchain and `Cargo.lock`. It uploads only the staged executable and
public build metadata. A separate job generates redistribution notices with a
checksum-pinned cargo-about tool. No signing keys or runtime configuration files
are used by these jobs. Publishing a release is a separate maintainer action
after all required jobs pass; a failed platform must not be advertised as tested.

Use `deploy/portable/package.py` on the downloaded artifacts to verify each
binary's hash, version and source revision before making ZIPs and checksums.
The source bundle, project license, Rust dependency notices and commit-specific
workflow URL accompany the release. Sign the checksums for the updater with
`python3 -B deploy/portable/sign.py --key <private key> <output>/SHA256SUMS` and upload
`SHA256SUMS.sig` with them; a release without it is never installed by the updater.
The private key stays offline and outside the repository: losing it ends automatic
updates for every installed copy, and anyone holding it can ship code to them. Building from pinned source makes the build
auditable; byte-for-byte reproducibility across machines has not been established.

## Keep README downloads current

`Update download links` refreshes the marked blocks in both READMEs when a release
is published, edited, withdrawn or deleted. It reads all published releases,
including every published release channel, and chooses the most recently published complete Windows
package set with `SHA256SUMS`. Drafts and partial uploads are never advertised.
The separate Docker setup link always downloads `main`; it is a source/setup ZIP,
not a prebuilt Docker image or a replacement for the published executable.

Prepare a release as a draft, upload all packages and checksums, then publish it.
If assets are added after publication, explicitly run `Update download links`
from Actions after the upload completes: an asset upload is not a publication
of a new release. Automated publishers using `GITHUB_TOKEN` must also dispatch
that workflow explicitly, since their events do not trigger another workflow.
Branch protection must permit the bot's README-only fast-forward commit; otherwise
the job fails visibly. It never force-pushes, creates a branch or changes releases.

To refresh locally, run `python3 -B deploy/portable/update_downloads.py` and review
the README diff. Run its fixture tests with
`python3 -B -m unittest discover -s deploy/portable -p 'test_*.py'`.
GitHub's generic `/releases/latest` can omit published packages based on release metadata.
The updater therefore reads the release list and selects complete uploaded packages.
