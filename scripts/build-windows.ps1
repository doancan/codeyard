# Build a standalone Windows app, including all Linux SSH agents, in one command.
param([ValidateSet('Debug', 'Release')][string]$Profile = 'Debug')
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
. "$PSScriptRoot/agent-bundle.ps1"
Push-Location (Split-Path $PSScriptRoot -Parent)
$previousAgentsDir = $env:AGENTS_DIR
try {
    if ($env:AGENTS_DIR) {
        $agentsDir = (Resolve-Path -LiteralPath $env:AGENTS_DIR).Path
    } else {
        & "$PSScriptRoot/build-agents.ps1" | Out-Host
        $agentsDir = (Resolve-Path -LiteralPath 'target/agents').Path
    }
    Assert-AgentBundle -Directory $agentsDir
    $env:AGENTS_DIR = $agentsDir
    $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
    if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
    $cargoArgs = @('build', '--locked', '-p', 'app')
    if ($Profile -eq 'Release') { $cargoArgs += '--release' }
    & $cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Windows application build failed' }
    $metadata = & $cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $binary = Join-Path $metadata.target_directory ($Profile.ToLowerInvariant() + '/codeyard.exe')
    & "$PSScriptRoot/test-standalone-agents.ps1" -Executable $binary
    Write-Output $binary
} finally {
    $env:AGENTS_DIR = $previousAgentsDir
    Pop-Location
}
