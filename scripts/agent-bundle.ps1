# Shared checks for remote Linux executables. Never ship a Windows .exe renamed as a Linux agent.
function Assert-LinuxAgent {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Target)
    $machines = @{ 'x86_64-unknown-linux-musl' = 62; 'aarch64-unknown-linux-musl' = 183 }
    if (-not $machines.ContainsKey($Target)) { throw "Unsupported Linux agent target: $Target" }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Missing SSH agent: $Path. Run scripts/build-agents.ps1 or set AGENTS_DIR to a complete agent bundle." }
    $bytes = [System.IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Path).Path)
    if ($bytes.Length -lt 64 -or $bytes[0] -ne 127 -or $bytes[1] -ne 69 -or $bytes[2] -ne 76 -or $bytes[3] -ne 70 -or $bytes[4] -ne 2 -or $bytes[5] -ne 1) {
        throw "SSH agent is not a 64-bit little-endian Linux ELF binary: $Path"
    }
    $machine = [int]$bytes[18] + 256 * [int]$bytes[19]
    if ($machine -ne $machines[$Target]) { throw "SSH agent architecture does not match ${Target}: $Path" }
}

function Assert-AgentBundle {
    param([Parameter(Mandatory)][string]$Directory)
    foreach ($target in @('x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl')) {
        Assert-LinuxAgent -Path (Join-Path $Directory "codeyard-agent-$target") -Target $target
    }
}
