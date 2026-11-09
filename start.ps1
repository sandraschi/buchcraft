#Requires -Version 5.1
# buchcraft launcher: builds the CLI and the desktop app, then opens the window.
$ErrorActionPreference = 'Stop'
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host '[buchcraft] cargo not found. Install Rust from https://rustup.rs'
    exit 1
}
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = 'D:\Dev\cargo-target' }
Set-Location $PSScriptRoot
cargo build --release -p buchcraft-mcp -p buchcraft-app
if ($LASTEXITCODE -ne 0) { exit 1 }
$app = Join-Path $env:CARGO_TARGET_DIR 'release\buchcraft-app.exe'
Start-Process -FilePath $app
Write-Host "[buchcraft] window started: $app"
Write-Host '[buchcraft] empty ledger? click Demo-Firma laden on the overview.'
