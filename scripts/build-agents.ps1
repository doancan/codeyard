# Build static Linux SSH agents on Windows without WSL, Docker or Zig.
# Rust's musl targets include their libc and rust-lld supplies the cross-linker.
# The app's development builds find target/agents; bundle-windows.ps1 uses it too.
param(
    [ValidateSet('x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl')]
    [string[]]$Targets = @('x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'),
    [string]$Destination = 'target/agents'
)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
. "$PSScriptRoot/agent-bundle.ps1"

function Find-RustTool([string]$Name) {
    $command = Get-Command "$Name.exe" -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $fallback = Join-Path $env:USERPROFILE ".cargo/bin/$Name.exe"
    if (Test-Path -LiteralPath $fallback) { return $fallback }
    throw "Cannot find $Name. Install Rust with rustup first."
}

Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    $cargo = Find-RustTool 'cargo'
    $rustc = Find-RustTool 'rustc'
    $rustup = Find-RustTool 'rustup'
    $metadata = & $cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $libdir = & $rustc --print target-libdir
    if ($LASTEXITCODE -ne 0) { throw 'rustc --print target-libdir failed' }
    $linker = Join-Path (Split-Path $libdir -Parent) 'bin/rust-lld.exe'
    if (-not (Test-Path -LiteralPath $linker)) { throw "Missing Rust cross-linker: $linker" }
    $destinationPath = if ([System.IO.Path]::IsPathRooted($Destination)) {
        [System.IO.Path]::GetFullPath($Destination)
    } else {
        [System.IO.Path]::GetFullPath((Join-Path $PWD $Destination))
    }
    New-Item -ItemType Directory -Force -Path $destinationPath | Out-Null
    foreach ($target in $Targets) {
        & $rustup target add $target
        if ($LASTEXITCODE -ne 0) { throw "Could not install Rust target $target" }
        $linkerVariable = 'CARGO_TARGET_' + $target.Replace('-', '_').ToUpperInvariant() + '_LINKER'
        $previousLinker = [System.Environment]::GetEnvironmentVariable($linkerVariable, 'Process')
        try {
            [System.Environment]::SetEnvironmentVariable($linkerVariable, $linker, 'Process')
            & $cargo build --locked --profile release-agent -p agent --target $target
            if ($LASTEXITCODE -ne 0) { throw "Building SSH agent for $target failed" }
        } finally {
            [System.Environment]::SetEnvironmentVariable($linkerVariable, $previousLinker, 'Process')
        }
        $binary = Join-Path $metadata.target_directory "$target/release-agent/codeyard-agent"
        Assert-LinuxAgent -Path $binary -Target $target
        $dest = Join-Path $destinationPath "codeyard-agent-$target"
        Copy-Item -LiteralPath $binary -Destination $dest -Force
        Write-Output $dest
    }
} finally {
    Pop-Location
}
