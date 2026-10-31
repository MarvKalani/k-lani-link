param(
    [ValidateSet('Build', 'Pair', 'Start', 'Check', 'Stop')]
    [string]$Action = 'Start',
    [string]$ClientConfig,
    # Pair only: the relay the home computer offers the code on, e.g. https://relay.example.com.
    [string]$RelayUrl,
    # Pair only, for a relay whose certificate no public CA issued.
    [string]$CaCertFile,
    # HTTPS only: this customer's explicit proxy, e.g. http://proxy.example:3128. Never displayed.
    [string]$ProxyUrl,
    [switch]$NoBrowser
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$saved = @{}
foreach ($name in 'LINK_CLIENT_DIR', 'LINK_CLIENT_FILE', 'LINK_PROXY_URL', 'LINK_WORKSPACE') {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

function Fail([string]$Message) {
    [Console]::Error.WriteLine($Message)
    exit 1
}

function Invoke-Docker {
    & docker @args
    if ($LASTEXITCODE -ne 0) { Fail 'Docker command failed. No credentials were printed by this script.' }
}

function Assert-NoReparsePoints([string]$Path, [switch]$Recurse) {
    $item = Get-Item -LiteralPath $Path -Force
    $ancestor = $item
    while ($null -ne $ancestor) {
        if (($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Fail 'Junctions, symlinks and other reparse points are not allowed in this profile.'
        }
        if ($ancestor -is [IO.FileInfo]) { $ancestor = $ancestor.Directory }
        else { $ancestor = $ancestor.Parent }
    }
    if ($Recurse) {
        $pending = New-Object 'System.Collections.Generic.Stack[string]'
        $pending.Push($item.FullName)
        while ($pending.Count -gt 0) {
            foreach ($child in (Get-ChildItem -LiteralPath ($pending.Pop()) -Force)) {
                if (($child.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    Fail 'Remove workspace junctions, symlinks and other reparse points before starting.'
                }
                if ($child.PSIsContainer) { $pending.Push($child.FullName) }
            }
        }
    }
}

function Check-Desktop {
    $id = Invoke-Docker compose -f $compose ps -q opencode
    if (-not $id) { Fail 'The OpenCode container is not running.' }
    $inspection = @(Invoke-Docker inspect $id | ConvertFrom-Json)[0]
    $binds = @($inspection.Mounts | Where-Object { $_.Type -eq 'bind' })
    if ($binds.Count -ne 1 -or $binds[0].Destination -ne '/work' -or -not $binds[0].RW) {
        Fail 'Unexpected host mounts. Stop this stack.'
    }
    $source = $binds[0].Source.Replace('\', '/').TrimEnd('/').ToLowerInvariant()
    if ($source -notin @('d:/opencode', '/run/desktop/mnt/host/d/opencode', '/host_mnt/d/opencode')) {
        Fail 'The mounted host directory is not the approved D:\OpenCode directory.'
    }
    if ($inspection.HostConfig.NetworkMode -ne 'none' -or -not $inspection.HostConfig.ReadonlyRootfs -or
        $inspection.HostConfig.Privileged -or $inspection.HostConfig.LogConfig.Type -ne 'none') {
        Fail 'Unexpected container isolation settings. Stop this stack.'
    }
    Invoke-Docker compose -f $compose exec -T opencode python -B /opt/desktop/verify.py
    $response = Invoke-WebRequest -UseBasicParsing -Uri 'http://127.0.0.1:24244/' -TimeoutSec 10
    if ($response.StatusCode -ne 200) { Fail 'The local browser interface is not ready.' }
    Write-Host 'Browser interface ready at http://127.0.0.1:24244/ .'
}

function Assert-Workspace {
    if (-not (Test-Path -LiteralPath 'D:\OpenCode' -PathType Container)) {
        Fail 'Create D:\OpenCode and put only the intended project there first.'
    }
    Assert-NoReparsePoints 'D:\OpenCode' -Recurse
}

function Assert-Stopped {
    $running = @(Invoke-Docker compose -f $compose ps --status running -q)
    if ($running.Count -gt 0) { Fail 'The stack is already running. Use -Action Check, or stop it before preparing a new pair.' }
}

# 0: the path exists in that service's volume, 1: it does not. Only the exit code is used.
function Test-InVolume([string]$Service, [string]$Flag, [string]$Path) {
    & docker compose -f $compose run --rm -T --no-deps --pull never --entrypoint test $Service $Flag $Path | Out-Null
    if ($LASTEXITCODE -gt 1) { Fail 'Docker command failed. No credentials were printed by this script.' }
    return $LASTEXITCODE -eq 0
}

function Start-Stack {
    Invoke-Docker compose -f $compose run --rm --no-deps --pull never link check --config /run/secrets/client.toml
    Write-Host 'Start the matching remote host now. Waiting for the tunnel and model (up to 150 seconds).'
    Invoke-Docker compose -f $compose up -d --pull never --wait --wait-timeout 150 link opencode guard ui
    Check-Desktop
    if (-not $NoBrowser) { Start-Process 'http://127.0.0.1:24244/' }
}

try {
    if ($env:OS -ne 'Windows_NT') { Fail 'Run this launcher in Windows PowerShell with Docker Desktop Linux containers.' }
    if ($env:DOCKER_HOST) { Fail 'Unset DOCKER_HOST and select your local Docker Desktop context first.' }
    $context = @(Invoke-Docker context inspect | ConvertFrom-Json)[0]
    if (-not $context.Endpoints.docker.Host.StartsWith('npipe://')) {
        Fail 'A local Docker Desktop named-pipe context is required.'
    }
    $engine = Invoke-Docker info --format '{{.OSType}}/{{.Architecture}}'
    if ($engine -notin @('linux/x86_64', 'linux/amd64')) {
        Fail 'This profile requires Docker Desktop Linux containers on an x64 Windows computer.'
    }
    $compose = Join-Path $PSScriptRoot 'compose.yaml'
    # Stop/Check do not recreate containers; these placeholders only satisfy Compose interpolation.
    $env:LINK_CLIENT_DIR = 'C:/unused'
    $env:LINK_CLIENT_FILE = 'client.toml'
    $env:LINK_PROXY_URL = ''
    $env:LINK_WORKSPACE = 'D:/OpenCode'
    if ($Action -eq 'Build') {
        Invoke-Docker compose -f $compose build link opencode guard ui
        Write-Host 'Images built. Runtime startup uses only these local images.'
    }
    elseif ($Action -eq 'Stop') {
        Invoke-Docker compose -f $compose stop
        Write-Host 'Stopped. Private transport volumes were retained. WebRTC: a new tunnel needs a fresh pair. HTTPS: the imported pairing continues on the next start.'
    }
    elseif ($Action -eq 'Check') { Check-Desktop }
    elseif ($Action -eq 'Pair') {
        if (-not $RelayUrl) { Fail 'Pair requires -RelayUrl, the relay address the home computer uses.' }
        if ($ClientConfig) { Fail 'Pair receives the client folder by code; -ClientConfig is for Start.' }
        Assert-Workspace
        Assert-Stopped
        $joined = 'k-lani-link-desktop_joined'
        $env:LINK_PROXY_URL = $ProxyUrl
        if (Test-InVolume link '-e' /var/lib/link/https) {
            Fail 'This computer is already paired: use -Action Start. To pair anew, stop the stack and remove its volumes deliberately.'
        }
        # A pairing received earlier whose import did not finish is imported now, without a new code.
        if (-not (Test-InVolume join '-f' /var/lib/link/client/client.toml)) {
            $run = @('compose', '-f', $compose, 'run', '--rm', '-T', '--no-deps', '--pull', 'never')
            $relayArgs = @('--relay-url', $RelayUrl)
            if ($CaCertFile) {
                Assert-NoReparsePoints $CaCertFile
                $ca = (Get-Item -LiteralPath $CaCertFile -Force).FullName
                $run += @('-v', ($ca + ':/run/pair-ca/ca.pem:ro'))
                $relayArgs += @('--ca-cert-file', '/run/pair-ca/ca.pem')
            }
            # The code goes in on standard input: never into the command line or history.
            $code = Read-Host 'Pairing code shown on the home computer'
            $code | & docker @run join @relayArgs
            if ($LASTEXITCODE -ne 0) { Fail 'Pairing failed; nothing was imported. Ask the home computer for a new code.' }
        }
        $prepared = @(Invoke-Docker compose -f $compose run --rm -T --no-deps --pull never prepare-joined)
        $prepared | ForEach-Object { Write-Host $_ }
        if ($prepared -notcontains 'HTTPS pairing state: IMPORTED') { Fail 'The received pairing could not be imported.' }
        # The volume copy is now the only live pairing; the received one must not stay behind.
        Invoke-Docker volume rm $joined | Out-Null
        Start-Stack
    }
    elseif (-not $ClientConfig) {
        Assert-Workspace
        Assert-Stopped
        if (-not (Test-InVolume link '-e' /var/lib/link/https)) {
            Fail 'Start requires -ClientConfig pointing to the private client.toml in its own folder, or pair first with -Action Pair.'
        }
        Start-Stack
    }
    else {
        Assert-Workspace
        Assert-NoReparsePoints $ClientConfig
        $file = Get-Item -LiteralPath $ClientConfig -Force
        if ($file.PSIsContainer -or $file.Length -gt 65536) { Fail 'Select a regular private client configuration file.' }
        if ($file.FullName.StartsWith('D:\OpenCode\', [StringComparison]::OrdinalIgnoreCase)) {
            Fail 'Keep the private client configuration outside the agent workspace.'
        }
        # The preparation container reads the whole folder, so it must be a dedicated one.
        $folder = $file.Directory
        if ($null -eq $folder.Parent) { Fail 'Keep client.toml in its own private folder, not in a drive root.' }
        $https = (Get-Content -LiteralPath $file.FullName -Raw) -match '(?m)^\s*transport\s*=\s*"https"\s*$'
        if ($https) { Assert-NoReparsePoints $folder.FullName -Recurse }
        elseif ($ProxyUrl) { Fail '-ProxyUrl applies only to an HTTPS client folder.' }
        Assert-Stopped
        $env:LINK_CLIENT_DIR = $folder.FullName.Replace('\', '/')
        $env:LINK_CLIENT_FILE = $file.Name
        $env:LINK_PROXY_URL = $ProxyUrl
        $prepared = @(Invoke-Docker compose -f $compose run --rm -T --no-deps --pull never prepare)
        # Preparation prints only fixed, secret-free lines.
        $prepared | ForEach-Object { Write-Host $_ }
        if ($prepared -contains 'HTTPS pairing state: IMPORTED') {
            # The volume now holds the only live pairing. Retire the source copy instead of deleting it.
            $source = Join-Path $folder.FullName 'state'
            $retired = 'state.imported-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
            try {
                if (-not (Test-Path -LiteralPath $source -PathType Container)) { throw 'custom state folder' }
                Rename-Item -LiteralPath $source -NewName $retired
                Write-Host "The pairing state now lives in the Docker volume; the source copy was renamed to $retired. Do not use it again."
            }
            catch {
                Write-Warning 'The pairing state now lives in the Docker volume. Rename or remove the source state folder yourself; it must not be used again.'
            }
        }
        Start-Stack
    }
}
catch {
    # PowerShell/Docker failures can contain supplied paths. Do not dump exception objects or configurations.
    Write-Host 'Desktop operation failed. Check Docker Desktop, the approved workspace, fresh configuration and remote host. See deploy/desktop/README.md.'
    exit 1
}
finally {
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
