# OpenCode in MXC: one project, private session data, one local API endpoint.
[CmdletBinding()]
param(
    [string]$Project,
    [string]$Model,
    [string]$Prompt,
    [string]$ToolsFrom,
    [string]$PanelDirectory = (Join-Path $env:LOCALAPPDATA 'k-lani-link\control-panel'),
    [switch]$CheckIsolation
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$utf8 = New-Object System.Text.UTF8Encoding($false)
$app = Join-Path $env:LOCALAPPDATA 'k-lani-link\mxc'
$tools = Join-Path $app 'tools'
$run = Join-Path $app ('sessions\' + [guid]::NewGuid().ToString('N'))
$sandboxHome = Join-Path $run 'home'
$requestPath = Join-Path $run 'request.json'
$probeListener = $null
$probeListener6 = $null

function Stop-WithMessage([string]$Message) { throw $Message }

function Install-Pinned([string]$Name) {
    $entry = $pinned.$Name
    $valid = $true
    foreach ($file in $entry.files) {
        $leaf = Split-Path $file -Leaf
        $path = Join-Path $tools $leaf
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { $valid = $false; break }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() -ne $entry.file_sha256.$leaf) { $valid = $false; break }
    }
    if ($valid) { return }
    if ($ToolsFrom) {
        foreach ($file in $entry.files) {
            $leaf = Split-Path $file -Leaf
            $source = Join-Path $ToolsFrom $leaf
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { Stop-WithMessage "Missing offline tool: $source" }
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant() -ne $entry.file_sha256.$leaf) { Stop-WithMessage "Wrong SHA-256 for offline tool $leaf." }
            Copy-Item -LiteralPath $source -Destination (Join-Path $tools $leaf) -Force
        }
        return
    }
    $zip = Join-Path $run "$Name.zip"
    Write-Host "Downloading $Name $($entry.version) once, outside the sandbox ..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -UseBasicParsing -Uri $entry.url -OutFile $zip
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant() -ne $entry.sha256) { Stop-WithMessage "Wrong SHA-256 for the $Name download." }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    try {
        foreach ($file in $entry.files) {
            $item = $archive.GetEntry($file)
            if ($null -eq $item) { Stop-WithMessage "Missing archive entry: $file" }
            $leaf = Split-Path $file -Leaf
            $destination = Join-Path $tools $leaf
            [IO.Compression.ZipFileExtensions]::ExtractToFile($item, $destination, $true)
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToLowerInvariant() -ne $entry.file_sha256.$leaf) { Stop-WithMessage "Wrong SHA-256 for extracted tool $leaf." }
        }
    } finally { $archive.Dispose() }
    Remove-Item -LiteralPath $zip -Force
}

function Read-JsonResponse([string]$Uri, [hashtable]$Headers) {
    $response = Invoke-WebRequest -UseBasicParsing -Uri $Uri -Headers $Headers -TimeoutSec 10
    $body = if ($response.Content -is [byte[]]) { [Text.Encoding]::UTF8.GetString($response.Content) } else { $response.Content }
    return ($body | ConvertFrom-Json)
}

try {
    if (-not [Environment]::Is64BitProcess -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { Stop-WithMessage 'Use 64-bit PowerShell on Windows x64.' }
    $cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    $build = [int]$cv.CurrentBuild; $ubr = [int]$cv.UBR
    $supported = (($build -in 26100, 26200, 26300) -and $ubr -ge 9550) -or ($build -eq 28000 -and $ubr -ge 3086) -or $build -gt 28000
    if (-not $supported) { Stop-WithMessage "Windows $build.$ubr is too old. Install Windows updates (at least 26100.9550 / 26200.9550)." }
    if (-not $Project) {
        Add-Type -AssemblyName System.Windows.Forms
        $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
        $dialog.Description = 'Projektordner / Project folder for OpenCode'
        $dialog.ShowNewFolderButton = $true
        if ($dialog.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) { Stop-WithMessage 'No project folder chosen.' }
        $Project = $dialog.SelectedPath
    }
    if (-not (Test-Path -LiteralPath $Project -PathType Container)) { Stop-WithMessage "Not a folder: $Project" }
    $Project = (Resolve-Path -LiteralPath $Project).ProviderPath.TrimEnd('\')
    if ($Project -notmatch '^[A-Za-z]:\\' -or $Project.Length -le 3) { Stop-WithMessage 'Choose one local project folder, not a drive or network share.' }
    # Reject system folders and parents which would expose private launcher state.
    foreach ($protected in @($env:SystemRoot, $env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramData, $env:USERPROFILE, $env:LOCALAPPDATA, $env:APPDATA, $app, $PanelDirectory)) {
        if (-not $protected) { continue }
        $protected = [IO.Path]::GetFullPath($protected).TrimEnd('\')
        if ($Project.Equals($protected, [StringComparison]::OrdinalIgnoreCase) -or $protected.StartsWith($Project + '\', [StringComparison]::OrdinalIgnoreCase)) { Stop-WithMessage 'The project folder would expose your profile, system files or private launcher state.' }
        if (($protected -in @($env:SystemRoot, $env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramData, $app, $PanelDirectory)) -and $Project.StartsWith($protected + '\', [StringComparison]::OrdinalIgnoreCase)) { Stop-WithMessage 'Choose a project folder outside system and private launcher folders.' }
    }
    $item = Get-Item -LiteralPath $Project -Force
    while ($null -ne $item) {
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { Stop-WithMessage 'Junctions and symbolic links are not supported in the project path.' }
        $item = $item.Parent
    }
    New-Item -ItemType Directory -Force -Path $app | Out-Null
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    & icacls.exe $app /inheritance:r /grant:r "*$($sid):(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { Stop-WithMessage 'Cannot protect the private sandbox directory.' }
    foreach ($dir in @($tools, $run, $sandboxHome)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    $pinned = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'release.json') | ConvertFrom-Json
    if (-not $ToolsFrom -and (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'tools'))) { $ToolsFrom = Join-Path $PSScriptRoot 'tools' }
    Install-Pinned 'mxc'
    Install-Pinned 'opencode'
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Verify-Sandbox.js') -Destination $tools -Force
    Get-ChildItem -LiteralPath $tools -File | Unblock-File
    $wxc = Join-Path $tools 'wxc-exec.exe'
    $opencode = Join-Path $tools 'opencode.exe'
    $listen = '127.0.0.1:8787'
    $pointer = Join-Path $PanelDirectory 'https-pairing.txt'
    if (Test-Path -LiteralPath $pointer) {
        $id = (Get-Content -Raw -LiteralPath $pointer).Trim()
        if ($id -notmatch '^https-[a-f0-9]{32}$') { Stop-WithMessage 'Invalid control-panel pairing pointer.' }
        $client = Join-Path $PanelDirectory "$id\client.toml"
        if (Test-Path -LiteralPath $client) {
            $toml = Get-Content -Raw -LiteralPath $client
            if ($toml -match '(?m)^\s*listen\s*=\s*"([^"]+)"') { $listen = $Matches[1] }
        }
    }
    $keyFile = Join-Path $PanelDirectory 'api-key'
    if (-not (Test-Path -LiteralPath $keyFile -PathType Leaf)) { Stop-WithMessage 'Open the k-lani-link control panel and pair it first.' }
    $key = (Get-Content -Raw -LiteralPath $keyFile).Trim()
    if (-not $key -or $key -match '[\r\n]') { Stop-WithMessage 'Invalid control-panel API key.' }
    if ($listen -notmatch '^127\.0\.0\.1:([0-9]+)$') { Stop-WithMessage 'The API must listen on 127.0.0.1 with a valid port.' }
    $apiPort = [int]$Matches[1]
    if ($apiPort -lt 1 -or $apiPort -gt 65535) { Stop-WithMessage 'Invalid API port.' }
    $base = "http://$listen/v1"
    Write-Host 'Reading the models from the local k-lani-link API ...'
    $models = @((Read-JsonResponse "$base/models" @{ Authorization = "Bearer $key" }).data | Where-Object { $_ -and $_.id })
    if (-not $models.Count) { Stop-WithMessage 'No models available. Check that the control panel shows Connected.' }
    if ($Model -and -not ($models | Where-Object { $_.id -eq $Model })) { Stop-WithMessage "Model not offered: $Model" }
    if (-not $Model) { $Model = $models[0].id }
    $entries = [ordered]@{}
    foreach ($m in $models) {
        $modelKey = $m.id -replace '[^A-Za-z0-9._-]', '-'
        if ($entries.Contains($modelKey)) { Stop-WithMessage 'Two model IDs map to the same OpenCode ID.' }
        $context = 131072
        if ($m.PSObject.Properties['context_length'] -and $m.context_length) { $context = [int]$m.context_length }
        $entries[$modelKey] = [ordered]@{ id = $m.id; name = "$($m.id) via k-lani-link"
            limit = @{ context = $context; output = $context }; reasoning = $true; tool_call = $true
            interleaved = @{ field = 'reasoning_content' }; modalities = @{ input = @('text'); output = @('text') }
            cost = @{ input = 0; output = 0; cache_read = 0; cache_write = 0 } }
    }
    $config = [ordered]@{
        model = 'link/' + ($Model -replace '[^A-Za-z0-9._-]', '-'); default_agent = 'build'
        autoupdate = $false; share = 'disabled'; snapshot = $false; formatter = $false; lsp = $false
        enabled_providers = @('link'); plugin = @()
        provider = @{ link = @{ name = 'k-lani-link'; env = @('LINK_API_KEY'); npm = '@ai-sdk/openai-compatible'
            options = @{ baseURL = $base; apiKey = '{env:LINK_API_KEY}' }; models = $entries } }
        permission = @{ read = 'allow'; edit = 'allow'; glob = 'allow'; grep = 'allow'; list = 'allow'
            bash = 'deny'; websearch = 'deny'; webfetch = 'deny'; external_directory = 'deny'; task = 'deny' }
    }
    $configDir = Join-Path $sandboxHome 'config\opencode'
    New-Item -ItemType Directory -Force -Path $configDir | Out-Null
    [IO.File]::WriteAllText((Join-Path $configDir 'opencode.json'), ($config | ConvertTo-Json -Depth 12), $utf8)
    foreach ($dir in @('data', 'cache', 'state', 'tmp', 'roaming', 'local')) { New-Item -ItemType Directory -Force -Path (Join-Path $sandboxHome $dir) | Out-Null }
    # A clean environment avoids passing host credentials and configuration into the sandbox.
    $envList = @("SystemRoot=$env:SystemRoot", "WINDIR=$env:SystemRoot", "SystemDrive=$env:SystemDrive",
        "COMSPEC=$env:SystemRoot\System32\cmd.exe", "PATH=$tools;$env:SystemRoot\System32;$env:SystemRoot",
        "HOME=$sandboxHome", "USERPROFILE=$sandboxHome", "APPDATA=$sandboxHome\roaming", "LOCALAPPDATA=$sandboxHome\local",
        "XDG_CONFIG_HOME=$sandboxHome\config", "XDG_DATA_HOME=$sandboxHome\data", "XDG_CACHE_HOME=$sandboxHome\cache", "XDG_STATE_HOME=$sandboxHome\state",
        "TEMP=$sandboxHome\tmp", "TMP=$sandboxHome\tmp", "LINK_API_KEY=$key", 'NO_PROXY=127.0.0.1,localhost,::1',
        'OPENCODE_DISABLE_MODELS_FETCH=1', 'OPENCODE_DISABLE_AUTOUPDATE=1', 'OPENCODE_DISABLE_PROJECT_CONFIG=1',
        'OPENCODE_DISABLE_CLAUDE_CODE=1', 'OPENCODE_DISABLE_FFF=1', 'OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER=1', 'BUN_RUNTIME_TRANSPILER_CACHE_PATH=0')
    # Bun resolves paths by querying their ancestors. Permit metadata, never parent file contents.
    $enumerate = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($path in @($Project, $tools, $sandboxHome)) {
        $ancestor = (Get-Item -LiteralPath $path -Force).Parent
        while ($null -ne $ancestor) { [void]$enumerate.Add($ancestor.FullName); $ancestor = $ancestor.Parent }
    }
    $request = @{ version = '1.0.0'; containment = 'processcontainer'
        network = @{ egress = @{ default = 'deny' }; ingress = @{ default = 'allow'; hostLoopback = 'allow' } }
        runtimeConfig = @{ networkProxy = "http://$listen" }
        filesystem = @{ readwritePaths = @($Project, $sandboxHome); readonlyPaths = @($tools) }
        fallback = @{ allowDaclMutation = $false }
        processContainer = @{ filesystem = @{ enumeratePaths = @($enumerate) } }
        process = @{ commandLine = "`"$opencode`" --mini --log-level ERROR"; cwd = $Project; env = $envList; inheritDefaultEnv = $false } }
    Write-Host "Project: $Project"
    Write-Host "Model: $Model"
    Write-Host "Only external endpoint: 127.0.0.1:$apiPort (no internet or LAN)." -ForegroundColor Green
    if ($CheckIsolation) {
        # A known-good live listener prevents a false pass from testing an unused port.
        $probeListener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
        $probeListener.Start()
        $probePort = $probeListener.LocalEndpoint.Port
        $probeListener6 = [Net.Sockets.TcpListener]::new([Net.IPAddress]::IPv6Loopback, $probePort)
        $probeListener6.Start()
        $outside = Join-Path $run 'outside.txt'
        [IO.File]::WriteAllText($outside, 'not-readable-in-sandbox', $utf8)
        $request.process.env += @('BUN_BE_BUN=1', "LINK_TEST_PROJECT=$Project", "LINK_TEST_TOOLS=$tools", "LINK_TEST_OUTSIDE=$outside", "LINK_TEST_API=$base", "LINK_TEST_OTHER_PORT=$probePort", ('LINK_TEST_SCRIPT=' + [Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $PSScriptRoot 'Verify-Sandbox.js')))))
        $request.process.commandLine = "`"$opencode`" -e `"await eval(Buffer.from(process.env.LINK_TEST_SCRIPT,'base64').toString())`""
    }
    [IO.File]::WriteAllText($requestPath, ($request | ConvertTo-Json -Depth 12), $utf8)
    if ($Prompt -and -not $CheckIsolation) {
        # User text is an argument, never part of a PowerShell/cmd expression.
        & $wxc $requestPath -- $opencode run --auto --log-level ERROR $Prompt
    } else { & $wxc $requestPath }
    $code = $LASTEXITCODE
    if ($code -ne 0) { Stop-WithMessage "The sandbox exited with code $code." }
} catch {
    $message = $_.Exception.Message
    if (Get-Variable key -ErrorAction SilentlyContinue) { if ($key) { $message = $message.Replace($key, '[redacted]') } }
    Write-Host $message -ForegroundColor Red
    exit 1
} finally {
    if ($null -ne $probeListener) { $probeListener.Stop() }
    if ($null -ne $probeListener6) { $probeListener6.Stop() }
    Remove-Item -LiteralPath $requestPath -Force -ErrorAction SilentlyContinue
    # Session data is temporary. Project files stay in the selected project directory.
    if (Test-Path -LiteralPath $run) { Remove-Item -LiteralPath $run -Recurse -Force -ErrorAction SilentlyContinue }
}
