$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/agent-bundle.ps1"
$targetRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path $PSScriptRoot -Parent) 'target'))
$scratch = Join-Path $targetRoot ('agent-bundle-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch -Force | Out-Null

function Assert-Rejected([scriptblock]$Action) {
    $rejected = $false
    try { & $Action } catch { $rejected = $true }
    if (-not $rejected) { throw 'An incomplete or invalid agent bundle was accepted' }
}

function Write-ElfFixture([string]$Name, [int]$Machine) {
    $bytes = New-Object byte[] 64
    $bytes[0] = 127; $bytes[1] = 69; $bytes[2] = 76; $bytes[3] = 70
    $bytes[4] = 2; $bytes[5] = 1; $bytes[18] = $Machine
    [System.IO.File]::WriteAllBytes((Join-Path $scratch $Name), $bytes)
}

try {
    Assert-Rejected { Assert-AgentBundle -Directory $scratch }
    $x64 = 'codeyard-agent-x86_64-unknown-linux-musl'
    $arm64 = 'codeyard-agent-aarch64-unknown-linux-musl'
    Write-ElfFixture $x64 62
    Assert-Rejected { Assert-AgentBundle -Directory $scratch }
    Write-ElfFixture $arm64 183
    Assert-AgentBundle -Directory $scratch
    # A Windows PE executable or a binary built for the wrong CPU must fail.
    [System.IO.File]::WriteAllBytes((Join-Path $scratch $x64), [byte[]](77, 90, 0, 0))
    Assert-Rejected { Assert-AgentBundle -Directory $scratch }
    Write-ElfFixture $x64 183
    Assert-Rejected { Assert-AgentBundle -Directory $scratch }
    Write-ElfFixture $x64 62
    Assert-AgentBundle -Directory $scratch
    Write-Output 'Agent bundle checks passed (missing files, valid ELF, wrong format, wrong CPU).'
} finally {
    $resolvedScratch = [System.IO.Path]::GetFullPath($scratch)
    if (-not $resolvedScratch.StartsWith($targetRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a test directory outside $targetRoot"
    }
    Remove-Item -LiteralPath $resolvedScratch -Recurse -Force
}
