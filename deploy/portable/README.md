# Portable native builds

k-lani-link runs without an installer, administrator installation, bundled LLM or
additional WASM runtime. Starting the executable without arguments opens a local
browser control panel. On Windows, double-click the EXE. Explicit `client`, `host`,
pairing and operator commands remain available in PowerShell or a terminal.
The control panel was introduced in **1.2610.29-preview.3**; the earlier
**1.2610.29** download is command-line only. A tray icon is not included yet.

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

### Browser control panel (1.2610.29-preview.3 and later)

1. Download the current x64 EXE for Intel/AMD Windows, or ARM64 for Windows on ARM.
2. Double-click it. The local control panel opens in your default browser; no
   command-line arguments are needed. It listens on a random loopback-only port.
3. Choose your fresh private **client.toml** file. The panel validates it and saves
   a private copy under `%LOCALAPPDATA%\k-lani-link\control-panel`. It normalizes
   container paths, retains consumed-code markers and never imports operator keys.
4. Start the matching host on the model computer and click **Connect**. The live
   status distinguishes waiting, connected, recovering, rejected and used codes.
5. Click **Test backend** after connecting. It requests `/v1/models` through the
   real encrypted tunnel and reports model IDs and the full model-list round trip.
   It does not send prompts or benchmark token generation.
6. For a native AI app, use the displayed base URL and **Show local API key**.
   This is the inference key, not the one-time pairing code.

Closing the browser tab leaves the transport running. Double-clicking the EXE
again reopens the same panel. **Quit application** stops the transport and panel;
**Disconnect** stops only the transport. A fully ended tunnel needs a new pairing
code on both computers. Closing the tab is not a disconnect. No autostart or system
service is installed, and no tray icon is claimed for this preview.

The panel uses an unpredictable local access token, checks Host and Origin, has no
external scripts, and stores no conversation history or connection logs. The
bootstrap token is removed from the browser address bar after loading. Its private
address file is deleted on orderly exit. The browser, clipboard and local user
account remain trusted. Keep the data directory private; Windows uses its inherited
NTFS permissions. Do not put it inside the agent's workspace.

This first panel controls the **native client only**, supports direct connections
with TURN disabled, and does not start Docker or manage a host/model supervisor.
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

The first portable builds are **unsigned previews**, without Windows publisher
signing or Apple Developer ID notarization. On Apple Silicon a linker-generated
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
workflow URL accompany the release. Building from pinned source makes the build
auditable; byte-for-byte reproducibility across machines has not been established.

## Keep README downloads current

`Update download links` refreshes the marked blocks in both READMEs when a release
is published, edited, withdrawn or deleted. It reads all published releases,
including previews, and chooses the most recently published complete Windows
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
GitHub's generic `/releases/latest` excludes previews, so it cannot serve as the
sole download link while this project distributes preview builds.
