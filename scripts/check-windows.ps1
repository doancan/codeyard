# Native Windows checks. Requires Rust MSVC, C++ Build Tools, Windows SDK (fxc) and Python 3.
# CI runs the standalone release in a parallel job; local invocation keeps every check.
param([switch]$SkipStandalone)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
Set-Location (Split-Path $PSScriptRoot -Parent)
$pythonVersion = & python --version 2>&1
if ($LASTEXITCODE -ne 0 -or $pythonVersion -notmatch '^Python 3\.') { throw 'Python 3 is required for the scripted Rust debugger regression.' }
& "$PSScriptRoot/test-agent-bundle.ps1"
if (-not $env:AGENTS_DIR) { & "$PSScriptRoot/build-agents.ps1" }
& rustc --edition=2024 --test crates/app/build.rs -o target/agent-build-tests.exe
if ($LASTEXITCODE -ne 0) { throw 'Build-script tests did not compile' }
& ./target/agent-build-tests.exe
if ($LASTEXITCODE -ne 0) { throw 'Build-script tests failed' }

function Invoke-Cargo {
    param([string[]]$CargoArgs)
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo $($CargoArgs -join ' ') failed" }
}

Invoke-Cargo -CargoArgs @('fmt', '--all', '--', '--check')
Invoke-Cargo -CargoArgs @('clippy', '--locked', '--workspace', '--lib', '--bins', '--', '-D', 'warnings')
# Every test of the workspace (libraries, binaries, integration tests), with no name filter. What Windows does not
# have is decided inside the test, never by leaving it out here: code that exists only on Unix is `#[cfg(unix)]`, and
# a test whose tool is missing on this machine (sshd, a POSIX shell, the exthost's OS sandbox) prints a SKIPPED line
# with the reason (docs/testing.md, "Windows"). Every test binary runs even after one fails, so one run shows every failure.
Invoke-Cargo -CargoArgs @('test', '--release', '--locked', '--workspace', '--no-fail-fast', '--', '--test-threads=1', '--nocapture')
if (-not $SkipStandalone) {
    Invoke-Cargo -CargoArgs @('build', '--release', '--locked', '-p', 'app')
    & "$PSScriptRoot/test-standalone-agents.ps1" -Executable 'target/release/codeyard.exe'
}
