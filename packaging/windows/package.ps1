<#
.SYNOPSIS
  Build, sign and package FreeDAWm for Windows.

.DESCRIPTION
  Produces, in $env:DIST (default: dist/release):
    freedawm-<version>-windows-<arch>.msi            per-machine installer (WiX v5)
    freedawm-<version>-windows-<arch>-portable.zip   freedawm.exe + freedawm-cli.exe + freedawm-scan.exe + DLLs

  freedawm-scan.exe is the plugin scanner helper: the app scans plugins only through it, next to itself.

  -Asio builds the app with ASIO audio devices (`--features soundcraft/asio`). It needs Steinberg's
  ASIO SDK unzipped with CPAL_ASIO_DIR pointing at it (the folder holding common/asio.h). Steinberg
  licenses the SDK under GPLv3 or its own agreement; these builds use GPLv3, so the package then
  carries GPL-3.0.txt and ASIO-NOTICE.txt (see docs/release-playbook.md › ASIO).

  DirectML.dll and the Visual C++ runtime DLLs are staged next to the programs (app-local), so
  neither the MSI nor the portable zip needs the VC++ redistributable. Signing is delegated to sign.ps1 (skipped with a
  warning when no signing secrets are set).

  Needs: Rust (MSVC toolchain + the target), the Windows SDK (rc.exe, signtool.exe),
  and WiX v5: dotnet tool install --global wix --version 5.0.2

.EXAMPLE
  pwsh packaging/windows/package.ps1 -Arch x64
  pwsh packaging/windows/package.ps1 -Arch x64 -Asio   # with ASIO; needs CPAL_ASIO_DIR
  pwsh packaging/windows/package.ps1 -Arch x86 -SkipBuild
  pwsh packaging/windows/package.ps1 -Arch arm64     # cross-compiled; needs the MSVC ARM64 build tools
#>
param(
  [ValidateSet('x64', 'x86', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild,
  [switch] $Asio
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
# Each FREEDAWM_* variable also reads its older SOUNDCRAFT_* name.
foreach ($n in @('VERSION', 'BUILD_SHA', 'BUILD_DATE')) {
  if (-not [Environment]::GetEnvironmentVariable("FREEDAWM_$n")) { [Environment]::SetEnvironmentVariable("FREEDAWM_$n", [Environment]::GetEnvironmentVariable("SOUNDCRAFT_$n")) }
}
$Version = $env:FREEDAWM_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# MSI ProductVersion is numeric (major.minor.build); pre-release tags are dropped there.
$MsiVersion = ($Version -split '-')[0]

$Target = switch ($Arch) { 'x64' { 'x86_64-pc-windows-msvc' } 'x86' { 'i686-pc-windows-msvc' } 'arm64' { 'aarch64-pc-windows-msvc' } }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

if (-not $env:FREEDAWM_BUILD_SHA) { $env:FREEDAWM_BUILD_SHA = (git -C $Root rev-parse HEAD 2>$null) }
if (-not $env:FREEDAWM_BUILD_DATE) { $env:FREEDAWM_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }

Write-Output "FreeDAWm $Version for Windows $Arch ($Target)"

if (-not $SkipBuild) {
  # The dynamic C runtime (the MSVC default): ONNX Runtime's prebuilt static library (ort-sys)
  # is compiled against it, so +crt-static fails to link. The runtime DLLs ship app-locally below.
  # Fail the build (rather than warn) if the icon/VERSIONINFO can't be embedded.
  $env:FREEDAWM_REQUIRE_WINRES = '1'
  $features = @()
  if ($Asio) {
    if (-not $env:CPAL_ASIO_DIR -or -not (Test-Path (Join-Path $env:CPAL_ASIO_DIR 'common\asio.h'))) {
      throw '-Asio needs CPAL_ASIO_DIR set to an unzipped ASIO SDK (the folder holding common\asio.h)'
    }
    $features = @('--features', 'soundcraft/asio')
  }
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p soundcraft -p soundcraft-cli -p soundcraft-scan --target $Target @features }
}

$Bin = Join-Path $TargetDir "$Target\release"

# Check both binaries' PE headers before packaging. Machine (COFF header) must match -Arch, so an
# x64 build can never ship labelled arm64. Subsystem (optional header): 2 = Windows GUI, 3 = console.
# The app must be GUI (no console window opens with it); the CLI must stay console so its output
# reaches the terminal.
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}
$Machine = switch ($Arch) { 'x64' { 0x8664 } 'x86' { 0x14C } 'arm64' { 0xAA64 } }
foreach ($check in @(@('freedawm.exe', 2), @('freedawm-cli.exe', 3), @('freedawm-scan.exe', 3))) {
  $h = Get-PeHeader (Join-Path $Bin $check[0])
  if ($h.Machine -ne $Machine) { throw "$($check[0]) is for machine 0x$('{0:X}' -f $h.Machine), expected 0x$('{0:X}' -f $Machine) ($Arch)" }
  if ($h.Subsystem -ne $check[1]) { throw "$($check[0]) has PE subsystem $($h.Subsystem), expected $($check[1])" }
  Write-Output "ok $($check[0]): $Arch, PE subsystem $($h.Subsystem)"
}
$Stage = Join-Path $TargetDir "windows-package\$Arch"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item (Join-Path $Bin 'freedawm.exe'), (Join-Path $Bin 'freedawm-cli.exe'), (Join-Path $Bin 'freedawm-scan.exe') $Stage
if ($Asio) {
  # The ASIO SDK is used under GPLv3, so this build as a whole is distributed under GPLv3.
  Copy-Item (Join-Path $PSScriptRoot 'GPL-3.0.txt') $Stage
  @(
    "FreeDAWm $Version for Windows ($Arch), built with ASIO support.",
    '',
    "This build includes code from Steinberg's ASIO SDK, which Steinberg Media Technologies GmbH",
    'licenses under the GNU General Public License version 3 (or its own licence agreement). It is',
    'used here under GPLv3, so this Windows build as a whole is distributed under GPLv3: see',
    'GPL-3.0.txt. FreeDAWm''s own source code is MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE).',
    '',
    'Complete corresponding source: https://github.com/s0lidr0ck/FreeDAWm (the tag for this version),',
    'and the ASIO SDK from https://www.steinberg.net/developers/.',
    '',
    'ASIO is a trademark and software of Steinberg Media Technologies GmbH.'
  ) | Set-Content -Encoding utf8 (Join-Path $Stage 'ASIO-NOTICE.txt')
}

# DLLs the programs load, staged next to them (the MSI installs every staged *.dll):
# - DirectML.dll, ONNX Runtime's GPU backend, which ort's `copy-dylibs` puts next to the build;
# - the Visual C++ runtime (vcruntime140*.dll, msvcp140*.dll), app-local, so the app runs without
#   the VC++ redistributable installed. Microsoft allows redistributing these files from Visual
#   Studio's Redist folder.
$dlls = @(Get-ChildItem -Path $Bin -Filter '*.dll' -File -ErrorAction SilentlyContinue)
if (-not ($dlls | Where-Object Name -eq 'DirectML.dll')) { throw "DirectML.dll not found in $Bin (ort copy-dylibs)" }
$dlls | ForEach-Object { Copy-Item $_.FullName $Stage; Write-Output "staged $($_.Name)" }
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$crtDir = $null
if (Test-Path $vswhere) {
  $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if ($vs) {
    $crtDir = Get-ChildItem -Path (Join-Path $vs 'VC\Redist\MSVC') -Directory -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -match '^\d' } | Sort-Object { [version]($_.Name -replace '[^\d.].*$', '') } -Descending |
      ForEach-Object { Get-ChildItem -Path (Join-Path $_.FullName $Arch) -Directory -Filter 'Microsoft.VC*.CRT' -ErrorAction SilentlyContinue } |
      Select-Object -First 1
  }
}
if ($crtDir) {
  Get-ChildItem -Path $crtDir.FullName -Filter '*.dll' -File | ForEach-Object { Copy-Item $_.FullName $Stage; Write-Output "staged $($_.Name) (VC++ runtime)" }
} elseif ($env:GITHUB_ACTIONS) {
  throw 'the Visual C++ runtime (VC\Redist\MSVC\<version>\<arch>\Microsoft.VC*.CRT) was not found'
} else {
  Write-Warning 'Visual C++ runtime DLLs not found: the package will need the VC++ redistributable'
}

& (Join-Path $PSScriptRoot 'sign.ps1') (Join-Path $Stage 'freedawm.exe') (Join-Path $Stage 'freedawm-cli.exe') (Join-Path $Stage 'freedawm-scan.exe')

# ---- MSI ---------------------------------------------------------------------------------------
$Msi = Join-Path $Dist "freedawm-$Version-windows-$Arch.msi"
Invoke-Native 'wix build' {
  wix build (Join-Path $PSScriptRoot 'freedawm.wxs') -arch $Arch `
    -d "Version=$MsiVersion" -d "BinDir=$Stage" -d "IconPath=$(Join-Path $Root 'assets\app-icon\freedawm.ico')" `
    -o $Msi
}
# wix writes its debug symbols (.wixpdb) next to the MSI; keep them out of the release assets.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($Msi, '.wixpdb'))
& (Join-Path $PSScriptRoot 'sign.ps1') $Msi

# ---- portable zip ------------------------------------------------------------------------------
$Portable = Join-Path $TargetDir "windows-package\freedawm-$Version-windows-$Arch-portable"
Remove-Item -Recurse -Force $Portable -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Portable | Out-Null
Copy-Item (Join-Path $Stage '*.exe'), (Join-Path $Stage '*.dll') $Portable
Get-ChildItem -Path $Stage -Filter '*.txt' -File | ForEach-Object { Copy-Item $_.FullName $Portable }
foreach ($f in 'README.md', 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md') {
  $p = Join-Path $Root $f
  if (Test-Path $p) { Copy-Item $p $Portable }
}
$Zip = Join-Path $Dist "freedawm-$Version-windows-$Arch-portable.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $Portable -DestinationPath $Zip

# Smoke-test the CLI when this machine can run it. An ARM64 build made on an x64 runner can't run
# here; .github/workflows/windows-arm64.yml installs and runs it on ARM64 instead.
$HostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($Arch -ne 'arm64' -or $HostArch -eq 'arm64') {
  Invoke-Native 'freedawm-cli --version' { & (Join-Path $Stage 'freedawm-cli.exe') --version }
} else {
  Write-Output "skipping freedawm-cli --version: an $Arch build doesn't run on this $HostArch machine"
}
Get-Item $Msi, $Zip | Format-Table Name, Length
