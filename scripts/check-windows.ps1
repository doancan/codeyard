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
Invoke-Cargo -CargoArgs @('test', '--release', '--locked', '--workspace', '--lib', '--', '--test-threads=1')
# Native UI/path/keyboard and Rust Cargo/debug regressions. POSIX server fixtures remain on macOS/Linux.
Invoke-Cargo -CargoArgs @(
    'test', '--release', '--locked', '-p', 'app', '--bin', 'codeyard', '--',
    'windows_', 'resize_', 'qa_sidebar_sash_drag', 'links::tests', 'terminal::tests', 'ui::path_tests',
    'sysopen::tests', 'workspace_file::tests', 'vscode::tests', 'trash::', 'terminal_web_links',
    'files_from_the_system', 'qa_workspace_file', 'markdown', 'docker', 'web_preview',
    'rust_hardening', 'product_', 'a11y', 'worktree', 'explorer_',
    '--test-threads=1', '--nocapture'
)
if (-not $SkipStandalone) {
    Invoke-Cargo -CargoArgs @('build', '--release', '--locked', '-p', 'app')
    & "$PSScriptRoot/test-standalone-agents.ps1" -Executable 'target/release/codeyard.exe'
}
