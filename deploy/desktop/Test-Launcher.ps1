# Run on Windows without Docker. Test the filesystem preflight without starting services.
$ErrorActionPreference = 'Stop'
$tokens = $null
$problems = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'Manage.ps1'), [ref]$tokens, [ref]$problems)
if ($problems.Count -ne 0) { throw 'Windows launcher syntax check failed.' }
$check = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
    $node.Name -eq 'Assert-NoReparsePoints'
}, $true)
if ($null -eq $check) { throw 'Filesystem preflight function is missing.' }
. ([ScriptBlock]::Create($check.Extent.Text))
function Fail([string]$Message) { throw [InvalidOperationException]::new($Message) }
$root = Join-Path ([IO.Path]::GetTempPath()) ('link-launcher-' + [guid]::NewGuid().ToString('N'))
$work = Join-Path $root 'work with spaces'
$outside = Join-Path $root 'outside'
$junction = Join-Path $work 'escape'
try {
    $null = New-Item -ItemType Directory -Path $work, $outside
    $file = Join-Path $work 'ordinary.txt'
    [IO.File]::WriteAllText($file, 'synthetic test')
    Assert-NoReparsePoints $work -Recurse
    Assert-NoReparsePoints $file
    $null = New-Item -ItemType Junction -Path $junction -Target $outside
    $rejected = $false
    try { Assert-NoReparsePoints $work -Recurse }
    catch [InvalidOperationException] { $rejected = $true }
    if (-not $rejected) { throw 'Workspace junction was not rejected.' }
    $rejected = $false
    try { Assert-NoReparsePoints $junction }
    catch [InvalidOperationException] { $rejected = $true }
    if (-not $rejected) { throw 'Linked configuration parent was not rejected.' }
    Write-Host 'PASS: Windows launcher syntax, ordinary paths and junction rejection.'
}
finally {
    if (Test-Path -LiteralPath $junction) { [IO.Directory]::Delete($junction) }
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
