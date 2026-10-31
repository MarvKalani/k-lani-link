# Windows: isolated OpenCode in the browser

This profile packages **OpenCode 2.2.0 and k-lani-link** as one Compose application.
Open `http://127.0.0.1:24244/` in your Windows browser. Basalt runs on the other
computer. No local GPU, model weights, Python, Node.js or Rust installation is
needed on Windows; Docker builds the images.

**Status:** a Windows launcher and Linux-container profile. Local container checks
are described below. A real Windows Docker Desktop / company-network connection
still needs acceptance testing. This is not a preconfigured connection to the
maintainer's running lab. The transport is WebRTC with your STUN service, or the
[HTTPS mailbox](#https-relay-instead-of-webrtc) through your own relay when only
ordinary HTTPS (optionally via a proxy) is allowed.

```text
Windows browser -> loopback UI gateway -> private Unix socket -> OpenCode
                                                               network: none
                                                               D:\OpenCode = /work
                                                                    |
                                                           private API socket
                                                                    |
                                                             k-lani-link
                                                                    |
                                                    encrypted peer tunnel -> Basalt
```

## What is isolated

| Component | Access |
| --- | --- |
| OpenCode | Only loopback networking; the inference API is reached through a Unix socket. |
| Host files | Exactly one writable host directory: `D:\OpenCode`, mounted as `/work`. No drive root, user profile, SSH directory or Docker socket. |
| Credentials | OpenCode gets only the local inference API key and its UI password. Pairing/signaling keys stay in the transport service; no operator key is installed. |
| UI gateway | Host-loopback port 24244, checked Host/Origin and restrictive CSP. A separate guard blocks new outbound IPv4/IPv6 connections and DNS. |
| k-lani-link | Network access for selected signaling/STUN and direct peer traffic; no project-directory mount. TURN is disabled and TURN settings are rejected by this profile. |
| Local storage | OpenCode home, history, logs and cache are RAM-backed and disappear when its container stops. Workspace changes persist on D:. Private credentials and consumed-code state use separate Docker volumes. |

Four runtime containers are deliberate: OpenCode, transport, UI guard and UI
gateway. A short-lived offline preparation container copies the supplied config
into Linux volumes with private permissions. Windows bind mounts alone do not
provide Unix `0600` ownership. It also separates the inference key from pairing
credentials. The launcher starts all services together.

OpenCode runs as UID 1000, with a read-only root filesystem, no Linux capabilities,
no privilege escalation, no Docker access, and bounded CPU/RAM/process counts.
The Docker log driver is disabled. No packet or prompt logging is added.

## Before starting

1. Obtain your IT administrator's approval for Docker Desktop, this application,
   the selected backend and the required network access. Use a current, patched
   Docker Desktop with **Linux containers / WSL2 on an x64 Windows PC**. A Docker
   Desktop subscription may be required by your organisation. Do not disable
   endpoint protections or change managed firewall policies to run this profile.
2. Put the repository outside `D:\OpenCode`, for example `C:\Tools\k-lani-link`.
   Put only the project you want the agent to work on in `D:\OpenCode`. Back it up
   or commit it first. OpenCode can read, overwrite and delete anything in that
   directory. The starter rejects junctions, symlinks and other reparse points;
   do not add Windows reparse points while the stack is running either.
3. Obtain a **fresh** private `client.toml` in **its own folder** outside that
   directory. The short-lived, network-less preparation container reads that whole
   folder, so keep nothing else in it. Keep it outside that directory,
   for example `%LOCALAPPDATA%\k-lani-link\office\client.toml`. The matching host
   must use the same fresh pairing code, a reachable HTTPS signaling endpoint and
   your explicit STUN service. Existing local-lab `127.0.0.1` signaling configs
   are rejected. Do not put credentials in Git, screenshots or the agent project.
   The [ICE example](../../examples/ice-servers.toml) selects `stun.marvins.app`
   and leaves Google/custom alternatives commented out. You choose which entries
   to enable; this profile never adds another server automatically.
4. Prepare the remote [Basalt host](../../QUICKSTART.md). The model must be loaded
   and advertise at least 262,144 context tokens for the existing OpenCode runtime.
   This is capacity, not proof that a full 256K request fits your hardware.
5. This profile retains the lab's **no Linux-VM swap** requirement. If your IT policy
   permits it, merge `swap=0` into the `[wsl2]` section of your existing
   `%USERPROFILE%\.wslconfig`; do not overwrite other settings. Applying WSL changes
   requires shutting down WSL and restarting Docker Desktop, which interrupts other
   WSL workloads. The launcher does not change these global settings. Container
   memory+swap limits also equal the RAM limits. This does not disable Windows'
   page file or prevent Windows from paging VM memory; it is not a no-disk-write
   or secure-erasure guarantee.

Docker Desktop file sharing must allow D:. The launcher accepts only a local
Docker named-pipe context, not a remote Docker daemon. It never mounts the engine
socket in a container.

## Build once (Internet needed)

In PowerShell in the repository directory:

```powershell
.\deploy\desktop\Manage.ps1 -Action Build
```

OpenCode's upstream archive is pinned and verified by SHA-256 during the build.
Runtime startup uses `--pull never`; missing images cause failure rather than an
automatic download. Docker itself, builds and the Windows browser are outside
OpenCode's network namespace. If script execution is restricted, have IT review
and approve the script; no execution-policy bypass is part of the instructions.

## Start

```powershell
.\deploy\desktop\Manage.ps1 -Action Start -ClientConfig "$env:LOCALAPPDATA\k-lani-link\office\client.toml"
```

Start the matching remote host at about the same time. The launcher validates the
config without displaying it, starts the tunnel, waits up to 150 seconds for model
discovery and UI readiness, checks the isolation, then opens the local browser.
Use `-NoBrowser` to suppress opening the tab. The provider/model configuration is
generated inside OpenCode; you do not paste pairing keys into its web interface.
OpenCode uses the sole model returned by the backend. Make a first test using
fictional data, for example asking it to create and inspect `/work/hello.txt`.

The transport uses a normal Docker bridge, so it works without enabling Docker
Desktop host networking or disabling Enhanced Container Isolation. This adds VM /
Docker NAT to the path. **STUN cannot guarantee direct connectivity through that
NAT, a VPN or a corporate firewall.** Direct peer UDP must be allowed. A network
that permits only proxied HTTPS may not work. There is no automatic TURN or HTTPS
inference fallback; for such networks use the HTTPS relay below instead.

## HTTPS relay instead of WebRTC

### Pair by code

The simplest way: the home computer shows a code (`k-lani-link offer-https`, or the `offer`
service of [the Basalt setup](../README.md#über-das-https-relay-statt-webrtc-gekoppelt-per-code)),
and this PC receives the pairing with it. No client folder is copied:

```powershell
.\deploy\desktop\Manage.ps1 -Action Pair -RelayUrl https://relay.example.com
# Behind this customer's proxy:
.\deploy\desktop\Manage.ps1 -Action Pair -RelayUrl https://relay.example.com -ProxyUrl "http://proxy.customer.example:3128"
```

The launcher asks for the code and hands it to the transport container on standard input,
so it never appears in a command line or history. That container receives the client side
over the relay into a private Linux volume; the network-less preparation imports it like a
copied folder, and the launcher then removes the received copy, so exactly one live
pairing remains. It starts the stack right away. Later starts need no folder:

```powershell
.\deploy\desktop\Manage.ps1 -Action Start
```

- A computer that is already paired refuses `Pair`. To pair anew, stop the stack and remove
  its volumes deliberately.
- A wrong or expired code imports nothing; ask the home computer for a new one. If the
  import itself was interrupted, the next `Pair` imports the received pairing without a
  new code.
- The relay needs a certificate from a public CA (Let's Encrypt). For a private CA add
  `-CaCertFile` with its PEM file.
- `-ProxyUrl` reaches the container as an environment variable, never as an argument.

### Copy the client folder

On the home side, issue a pair with `k-lani-link init-https` (see
[the HTTPS guide](../https/README.md)) and copy only the **client** folder to this PC, for
example `%LOCALAPPDATA%\k-lani-link\customer\client\`. It contains `client.toml`,
`relay-token`, an optional `ca.pem` and `state\`. Start with that `client.toml`:

```powershell
.\deploy\desktop\Manage.ps1 -Action Start -ClientConfig "$env:LOCALAPPDATA\k-lani-link\customer\client\client.toml"
# Behind this customer's proxy (no authentication or Basic only):
.\deploy\desktop\Manage.ps1 -Action Start -ClientConfig "...\client\client.toml" -ProxyUrl "http://proxy.customer.example:3128"
```

- **The first start imports the pairing.** The folder's `state` is copied into a private
  Docker volume, because Windows bind mounts cannot keep the `0600` permissions the
  pairing store requires. That volume copy is then the **only live pairing**: it advances
  with every session. The launcher renames the source to `state.imported-<time>` (it never
  deletes it); do not use that copy again. If renaming fails, for example on read-only
  media, it warns and you rename it yourself.
- **Later starts keep the volume state** and never overwrite it from the folder. A folder
  of a **different** pair is refused. To switch pairs deliberately, stop the stack and
  remove its volumes yourself, then import the new folder.
- **Resetting or purging Docker Desktop deletes the pairing.** A new pair must then be
  issued on the home side. Exporting the live state back to a folder is not built yet.
- **Proxies:** only an explicit proxy without authentication or with Basic authentication
  works; NTLM, Kerberos and PAC files do not, and the Windows proxy settings are ignored.
  `-ProxyUrl` is never displayed; it is stored only in the private transport volume. A
  TLS-inspecting proxy breaks the relay's certificate check unless the relay host is
  exempted; the end-to-end layer still protects content either way.
- The relay needs a **server** certificate. A self-signed certificate made with a plain
  `openssl req -x509` is marked as a CA and is refused by the client; add
  `-addext basicConstraints=critical,CA:FALSE` or use a CA-issued certificate.
- OpenCode stays `network=none`; only the transport container reaches the relay.

With [several backends](../../REFERENCE.md#several-backends-through-one-tunnel) behind the
host, `/v1/models` lists all of them. This OpenCode runtime still expects exactly one
model with at least 256K context and then fails its health check; use one backend for
this profile for now.

## Check and stop

```powershell
.\deploy\desktop\Manage.ps1 -Action Check
.\deploy\desktop\Manage.ps1 -Action Stop
```

The check verifies the actual host mount and network mode, process privileges,
credential separation, workspace writes, socket access, UI reachability and failed
direct Internet probes. These are boundary checks, not a guarantee against every
container escape or application vulnerability.

Do not run `docker compose down --volumes` on your normal installation. Consumed
pairing-code markers must survive restarts. An established tunnel's code cannot
be reused: after stopping the transport, get a new pair for both computers and
start with the new client file. The starter refuses to rewrite credentials while
the stack is running. If startup fails after pairing, the code may already be
consumed. Use Check to inspect the running setup before stopping it; do not erase
state to force reuse. Lost streams are not silently replayed.

## Privacy and practical limits

- The allowed model channel intentionally sends prompts, code excerpts and tool
  results to the selected backend computer. That computer and its operator must
  be trusted. OpenCode cannot independently reach an Internet provider, a package
  registry, GitHub, telemetry endpoints, host services or Docker DNS.
- Consequently `git fetch`, package downloads, web search and online MCP services
  do not work from the agent. Supply dependencies/files deliberately before a run,
  or build reviewed tools into the image. No general Internet proxy is included.
- The Windows browser displays the conversation and is outside the container.
  Browser extensions, host administrators, Docker/WSL, backups, security products
  and compromised endpoints remain outside this isolation boundary. Browser/UI
  traffic is an intentional output, not proof that data can never leave the PC.
- OpenCode can read its own container runtime files and write temporary files;
  “only D:\OpenCode” describes access to **Windows project files**, not a claim
  that a functioning Linux application sees no other files.
- Home/history are ephemeral here. Source files remain on D:, and private
  transport credentials persist in Docker volumes. This is neither automatic GDPR
  compliance nor a substitute for reviewing generated code or backing up projects.
- No remote desktop, remote Docker-control tool, extra MCP service or model
  management credential is installed in this profile.

Docker references: [network isolation](https://docs.docker.com/engine/network/drivers/none/),
[Windows bind mounts](https://docs.docker.com/engine/storage/bind-mounts/),
[Docker Desktop networking limits](https://docs.docker.com/desktop/features/networking/networking-how-tos/).
Microsoft: [WSL configuration](https://learn.microsoft.com/windows/wsl/wsl-config).

## Reproduce the container checks

On a Linux x64 test host with UID 1000, Docker Compose and swap disabled:

```sh
LINK_CLIENT_DIR=/unused LINK_WORKSPACE=/unused/work \
  docker compose -f deploy/desktop/compose.yaml build link opencode guard ui
python3 -B -m unittest discover -s deploy/desktop -p 'test_*.py'
python3 -B deploy/desktop/smoke.py
python3 -B deploy/desktop/https_smoke.py
```

`https_smoke.py` runs a real relay with a disposable certificate, a real host with a
synthetic backend and this profile's real transport container. It checks the first
import, a model list and a streamed answer through the private socket from a
`network=none` container, that the pairing advances and is kept on the next start, and
that another pair is refused. It does not start OpenCode itself, needs no swap-free host
and does not test a proxy, a WAN or Windows.

The smoke test creates its own uniquely named stack and temporary project,
substitutes a **synthetic Unix-socket inference server**, and runs the real pinned
OpenCode server and agent. It checks one workspace bind, hidden sibling files,
restricted credentials, loopback-only networking, process restrictions, blocked
UI-network TCP/DNS, rejected foreign Host/Origin headers, and streamed reasoning
plus a shell tool that creates the expected file. It removes only its disposable
test containers/volumes. It does not contact Basalt, consume a real pairing code,
modify the running lab or benchmark a model.

The initial Linux acceptance run passed with two reasoning events and one tool
call. The PowerShell parser was checked locally; Windows CI separately exercises
Windows PowerShell 5.1 and junction rejection. These checks do not validate NTFS
mount behavior, Docker Desktop networking, WSL swap configuration or the office
firewall on your actual machine. Those remain the next acceptance test.
