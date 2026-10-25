# Move only the EXE to an empty directory and verify both agents without touching user state or SSH.
param([Parameter(Mandatory)][string]$Executable)
$ErrorActionPreference = 'Stop'
# A GUI app must not allocate a separate console, including in the Debug profile.
# Inspect the PE header rather than relying on how it behaves when launched from a terminal.
$stream = [System.IO.File]::OpenRead((Resolve-Path -LiteralPath $Executable).Path)
$reader = New-Object System.IO.BinaryReader($stream)
try {
    if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) { throw 'Not a Windows executable' }
    $stream.Position = 60
    $peOffset = $reader.ReadInt32()
    if ($peOffset -lt 64 -or $peOffset + 94 -gt $stream.Length) { throw 'Invalid PE header offset' }
    $stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) { throw 'Invalid PE signature' }
    $stream.Position = $peOffset + 92
    if ($reader.ReadUInt16() -ne 2) { throw 'Codeyard must be a Windows GUI executable; a console build opens an extra terminal window.' }
} finally {
    $reader.Dispose()
}
# GPUI and Explorer both use icon group ID 1 from the EXE's native resources.
if (-not ('CodeyardExecutableResources' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CodeyardExecutableResources {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr LoadLibraryExW(string file, IntPtr reserved, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr FindResourceW(IntPtr module, IntPtr name, IntPtr type);
    [DllImport("kernel32.dll")]
    private static extern bool FreeLibrary(IntPtr module);
    public static bool HasAppIcon(string file) {
        IntPtr module = LoadLibraryExW(file, IntPtr.Zero, 2);
        if (module == IntPtr.Zero) return false;
        try { return FindResourceW(module, new IntPtr(1), new IntPtr(14)) != IntPtr.Zero; }
        finally { FreeLibrary(module); }
    }
}
'@
}
if (-not [CodeyardExecutableResources]::HasAppIcon((Resolve-Path -LiteralPath $Executable).Path)) {
    throw 'Codeyard EXE is missing its application icon resource.'
}
$targetRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path $PSScriptRoot -Parent) 'target'))
$scratch = Join-Path $targetRoot ('standalone-agent-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch -Force | Out-Null
try {
    $copy = Join-Path $scratch 'Codeyard.exe'
    Copy-Item -LiteralPath $Executable -Destination $copy
    $process = Start-Process -FilePath $copy -ArgumentList '--check-agent-bundle' -WorkingDirectory $scratch -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(30000)) {
        $process.Kill()
        $process.WaitForExit(5000) | Out-Null
        throw 'Standalone agent check timed out'
    }
    if ($process.ExitCode -ne 0) { throw "The EXE is missing valid embedded Linux agents (exit $($process.ExitCode))" }
    Write-Output 'Standalone checks passed: GUI executable, application icon, embedded Linux agents.'
} finally {
    $resolvedScratch = [System.IO.Path]::GetFullPath($scratch)
    if (-not $resolvedScratch.StartsWith($targetRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a test directory outside $targetRoot"
    }
    Remove-Item -LiteralPath $resolvedScratch -Recurse -Force
}
