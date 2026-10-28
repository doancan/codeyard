# Windows package: target\windows\Codeyard-<version>-windows-x64.zip (MSVC, on a Windows machine).
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1
# Needs Rust (msvc) and Visual Studio Build Tools with the Windows SDK (fxc.exe for GPUI's shaders).
# With Inno Setup 6 installed it also builds Codeyard-<version>-windows-x64-setup.exe
# (installer\windows\codeyard.iss).
# $env:AGENTS_DIR: optional prebuilt SSH agents. Otherwise built from source and embedded in the EXE.
# Licence texts go into the zip as licenses\ (same list as scripts/licenses.sh; that script checks both agree).
# The .exe is unsigned, so SmartScreen warns; a release needs a code-signing certificate.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
# Licence texts every artifact ships: source path -> name inside licenses\ (keep in sync with scripts/licenses.sh).
$licenses = [ordered]@{
  'LICENSE-MIT' = 'LICENSE-MIT'
  'LICENSE-APACHE' = 'LICENSE-APACHE'
  'THIRD_PARTY_NOTICES.md' = 'THIRD_PARTY_NOTICES.md'
  'THIRD_PARTY_CRATES.md' = 'THIRD_PARTY_CRATES.md'
  'crates\app\assets\icons\LICENSE' = 'icons-LICENSE'
  'crates\syntax\grammars\dockerfile\LICENSE' = 'dockerfile-grammar-LICENSE'
  'vendor\gpui\LICENSE-APACHE' = 'gpui-LICENSE-APACHE'
  'vendor\xattr\LICENSE-MIT' = 'xattr-LICENSE-MIT'
  'vendor\xattr\LICENSE-APACHE' = 'xattr-LICENSE-APACHE'
  'crates\spell\dicts\en\license' = 'spell-dict-en-license'
  'crates\syntax\LICENSES' = 'syntax-LICENSES'
}
foreach ($src in $licenses.Keys) { if (-not (Test-Path $src)) { throw "missing licence file: $src" } }
& "$PSScriptRoot/build-windows.ps1" -Profile Release
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
$version = ((& $cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json).packages | Where-Object name -eq 'app').version
if ($LASTEXITCODE -ne 0 -or -not $version) { throw 'cargo metadata failed' }
if ($version -notmatch '^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$') { throw "Invalid package version: $version" }
$name = "Codeyard-$version-windows-x64"
$out = [System.IO.Path]::GetFullPath((Join-Path $PWD 'target/windows'))
$packageDir = [System.IO.Path]::GetFullPath((Join-Path $out $name))
if (-not $packageDir.StartsWith($out + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace a package outside $out"
}
if (Test-Path -LiteralPath $packageDir) { Remove-Item -LiteralPath $packageDir -Recurse -Force }
New-Item -ItemType Directory -Force "$out\$name" | Out-Null
Copy-Item target\release\codeyard.exe "$out\$name\Codeyard.exe"
Copy-Item target\release\cyx.exe "$out\$name\cyx.exe"
New-Item -ItemType Directory -Force "$out\$name\licenses" | Out-Null
foreach ($src in $licenses.Keys) { Copy-Item -Recurse $src "$out\$name\licenses\$($licenses[$src])" }
Remove-Item "$out\$name.zip" -ErrorAction SilentlyContinue
Compress-Archive -Path "$out\$name" -DestinationPath "$out\$name.zip"
(Get-FileHash "$out\$name.zip" -Algorithm SHA256).Hash
Write-Output "$out\$name.zip"

# Installer (Setup.exe) when Inno Setup is available (winget install JRSoftware.InnoSetup).
$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1 }
if ($iscc) {
  $isccArgs = @("/DAppVersion=$version", 'installer\windows\codeyard.iss')
  & $iscc @isccArgs
  if ($LASTEXITCODE -ne 0) { throw 'Inno Setup compilation failed' }
  $setup = "$out\Codeyard-$version-windows-x64-setup.exe"
  (Get-FileHash $setup -Algorithm SHA256).Hash
  Write-Output $setup
} else {
  Write-Warning 'Inno Setup not found: built the zip only. To get the installer: winget install JRSoftware.InnoSetup'
}
