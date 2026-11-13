# Builds dist\ReignSetup.exe: the one-file Windows installer, holding the
# release hub and editor. The release workflow runs this too, so what you
# build here is what users download.
#
#   scripts\build-setup.ps1            build dist\ReignSetup.exe
#   scripts\build-setup.ps1 -Install   ...and install it on this computer
param([switch]$Install)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host 'Building the hub and the editor (release)...'
cargo build --release -p reign_hub -p reign_engine
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

# What the setup carries inside it.
$payload = Join-Path $root 'target\setup-payload'
New-Item -ItemType Directory -Force $payload | Out-Null
Copy-Item -Force target\release\reign-hub.exe, target\release\reign-engine.exe, LICENSE $payload

Write-Host 'Building ReignSetup.exe...'
$env:REIGN_SETUP_PAYLOAD = $payload
cargo build --release -p reign_setup
$code = $LASTEXITCODE
Remove-Item Env:REIGN_SETUP_PAYLOAD
if ($code -ne 0) { throw 'cargo build of the setup failed' }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item -Force target\release\reign-setup.exe dist\ReignSetup.exe
Write-Host "Built $(Join-Path $root 'dist\ReignSetup.exe')"

if ($Install) {
    Write-Host 'Installing...'
    $p = Start-Process -FilePath (Join-Path $root 'dist\ReignSetup.exe') -ArgumentList '--silent' -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "ReignSetup failed (exit $($p.ExitCode))" }
    Write-Host 'Installed: search "Reign Hub" in the Start menu.'
}
