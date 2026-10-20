# SPDX-License-Identifier: Apache-2.0
# Builds Toktail for Windows (x64, or ARM64 with -Arm64).
#
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1            # build dist\Toktail-<version>-windows-x64.zip
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Install   # also copy to %LOCALAPPDATA%\Programs\Toktail and start it
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Demo      # build and run the demo
#
# Needs Rust (https://rustup.rs) with the MSVC toolchain, which the
# rustup installer sets up together with the Visual Studio Build Tools.
param([switch]$Install, [switch]$Demo, [switch]$Arm64)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Write-Host "Rust isn't installed. Get it from https://rustup.rs (choose the default MSVC setup), then run this again."
  exit 1
}
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
$target = if ($Arm64) { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
$arch = if ($Arm64) { "arm64" } else { "x64" }
rustup target add $target | Out-Null

if ($Demo) {
  Write-Host "==> Building and starting the demo (Ctrl-C to stop). The portal opens at http://127.0.0.1:8799/"
  cargo run --release --target $target -- --demo
  exit $LASTEXITCODE
}

Write-Host "==> Building Toktail $version for Windows $arch"
cargo build --release --target $target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$out = "dist\Toktail-$version-windows-$arch"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "target\$target\release\Toktail.exe" $out
Copy-Item README.md, LICENSE, NOTICE $out
Compress-Archive -Force -Path "$out\*" -DestinationPath "$out.zip"
Write-Host "==> Built $out.zip"

if ($Install) {
  Get-Process Toktail -ErrorAction SilentlyContinue | Stop-Process -Force
  $dest = Join-Path $env:LOCALAPPDATA "Programs\Toktail"
  New-Item -ItemType Directory -Force $dest | Out-Null
  Copy-Item "$out\*" $dest -Force
  # A Start menu shortcut, so Toktail is one search away.
  $lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Toktail.lnk"
  $ws = New-Object -ComObject WScript.Shell
  $s = $ws.CreateShortcut($lnk); $s.TargetPath = Join-Path $dest "Toktail.exe"; $s.Description = "Every token leaves a trail."; $s.Save()
  Write-Host "==> Installed to $dest. Pip is in your system tray (click ^ if it's hidden); the portal is at http://127.0.0.1:8765/"
  Start-Process (Join-Path $dest "Toktail.exe")
}

