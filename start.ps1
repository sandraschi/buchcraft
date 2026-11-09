#Requires -Version 5.1
# buchcraft launcher: builds if needed, shows ledger status. The desktop app (egui) comes in Phase 2.
$ErrorActionPreference = 'Stop'
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host '[buchcraft] cargo not found. Install Rust from https://rustup.rs'
    exit 1
}
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = 'D:\Dev\cargo-target' }
Set-Location $PSScriptRoot
cargo build --release -p buchcraft-mcp
if ($LASTEXITCODE -ne 0) { exit 1 }
$exe = Join-Path $env:CARGO_TARGET_DIR 'release\buchcraft.exe'
& $exe status
if ($LASTEXITCODE -ne 0) { exit 1 }
Write-Host ''
Write-Host "[buchcraft] binary: $exe"
Write-Host '[buchcraft] try: buchcraft --help, buchcraft u30 2026-10, buchcraft mcp --allow-path <DIR>'
