# The hardware check for codeyard-exthost's Windows sandbox (docs/plugin-architecture.md §3, §15 decision 1).
#   pwsh scripts/check-exthost-sandbox-windows.ps1
# Run by a maintainer on a physical Windows machine (not a VM, not CI). It starts the exthost in an AppContainer
# (LPAC) inside a Job Object, the way the app will, and runs the escape probes there (crates/ext-host/tests/sandbox.rs,
# `appcontainer_refuses_every_escape`): the extension's own files must work; reading the user's files, writing
# outside its data folder, connecting out and starting a process must all be refused.
# Prints a record to paste into issue #100. When it passes, set `VERIFIED` in crates/ext-proto/src/appcontainer.rs.
# Needs Rust (MSVC) and the C++ Build Tools, like scripts/check-windows.ps1.
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
Set-Location (Split-Path $PSScriptRoot -Parent)

$os = Get-CimInstance Win32_OperatingSystem
$cs = Get-CimInstance Win32_ComputerSystem
$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
$commit = (& git rev-parse --short HEAD 2>$null)
if (-not $commit) { $commit = 'unknown' }

$log = Join-Path ([System.IO.Path]::GetTempPath()) "exthost-sandbox-check-$PID.txt"
& cargo test --release --locked -p ext-host --test sandbox appcontainer_refuses_every_escape -- --ignored --exact --nocapture *>&1 |
    Tee-Object -FilePath $log
$passed = $LASTEXITCODE -eq 0
$probes = Select-String -Path $log -Pattern '^probe (.*)$' | ForEach-Object { $_.Matches.Groups[1].Value }

Write-Output ''
Write-Output '### codeyard-exthost AppContainer hardware check'
Write-Output ''
Write-Output "- Result: $(if ($passed) { 'PASS' } else { 'FAIL' })"
Write-Output "- Date: $(Get-Date -Format 'yyyy-MM-dd')"
Write-Output "- Commit: $commit"
Write-Output "- Windows: $($os.Caption) $($os.Version) (build $($os.BuildNumber)), $($os.OSArchitecture)"
Write-Output "- Machine: $($cs.Manufacturer) $($cs.Model); hypervisor present: $($cs.HypervisorPresent)"
Write-Output "- CPU: $cpu"
Write-Output '- Probes:'
foreach ($p in $probes) { Write-Output "  - $p" }
Write-Output ''
Write-Output "Full output: $log"
if ($cs.HypervisorPresent) {
    Write-Output ''
    Write-Output 'Note: a hypervisor is present. If this is a VM, the record does not count as the physical-hardware check.'
}
if (-not $passed) { exit 1 }
