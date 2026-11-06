# Builds the setup file: target/release/bundle/nsis/Keyscene_<version>_x64-setup.exe
# Usage: pwsh -File tools\build-setup.ps1   (needs: cargo install tauri-cli --version "^2" --locked)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
. (Join-Path $PSScriptRoot 'build-env.ps1')
Push-Location $root
try {
    cargo tauri build
    if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
} finally { Pop-Location }
