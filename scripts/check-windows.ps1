# Native Windows checks. Requires Rust MSVC, C++ Build Tools and Windows SDK (fxc).
# CI runs the standalone release in a parallel job; local invocation keeps every check.
param([switch]$SkipStandalone)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
Set-Location (Split-Path $PSScriptRoot -Parent)
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
Invoke-Cargo -CargoArgs @('test', '--release', '--locked', '--workspace', '--lib', '--', '--test-threads=1')
# Native UI/path/keyboard regressions. POSIX shell/server fixtures remain on macOS/Linux.
Invoke-Cargo -CargoArgs @(
    'test', '--release', '--locked', '-p', 'app', '--bin', 'codeyard', '--',
    'windows_', 'resize_', 'qa_sidebar_sash_drag', 'links::tests', 'terminal::tests', 'ui::path_tests',
    'sysopen::tests', 'workspace_file::tests', 'vscode::tests', 'trash::', 'terminal_web_links',
    'files_from_the_system', 'qa_workspace_file', 'markdown', 'docker',
    '--test-threads=1'
)
if (-not $SkipStandalone) {
    Invoke-Cargo -CargoArgs @('build', '--release', '--locked', '-p', 'app')
    & "$PSScriptRoot/test-standalone-agents.ps1" -Executable 'target/release/codeyard.exe'
}
