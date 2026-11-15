# Builds Reign for Windows into dist\: the installer (ReignSetup.exe) and the
# zip the hub updates itself from. The release workflow runs this too, so
# what you build here is what users download.
#
#   scripts\build-setup.ps1               x64: ReignSetup.exe, reign-windows-x64.zip
#   scripts\build-setup.ps1 -Arch arm64   ReignSetup-arm64.exe, reign-windows-arm64.zip
#   scripts\build-setup.ps1 -Install      build x64, then install it on this computer
param(
    [ValidateSet('x64', 'arm64')][string]$Arch = 'x64',
    [switch]$Install
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$target = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Arch]
$built = "target\$target\release"
$setupName = if ($Arch -eq 'x64') { 'ReignSetup.exe' } else { "ReignSetup-$Arch.exe" }

function Invoke-Cargo {
    cargo build --release --target $target @args
    if ($LASTEXITCODE -ne 0) { throw "cargo build $args failed" }
}

Write-Host "Building the hub and the editor ($Arch)..."
Invoke-Cargo -p reign_hub -p reign_engine

$payload = "target\setup-payload-$Arch"
New-Item -ItemType Directory -Force $payload, dist | Out-Null
Copy-Item -Force "$built\reign-hub.exe", "$built\reign-engine.exe", LICENSE $payload

Write-Host 'Building the installer...'
$env:REIGN_SETUP_PAYLOAD = (Resolve-Path $payload).Path
try { Invoke-Cargo -p reign_setup } finally { Remove-Item Env:REIGN_SETUP_PAYLOAD }
Copy-Item -Force "$built\reign-setup.exe" "dist\$setupName"

# The update zip also carries the licences of the fonts built into the hub.
$zip = "dist\reign-windows-$Arch.zip"
Copy-Item -Force hub\assets\fonts\*-OFL.txt $payload
Compress-Archive -Force -Path "$payload\*" -DestinationPath $zip
Write-Host "Built dist\$setupName and $zip"

if ($Install) {
    Write-Host 'Installing...'
    $p = Start-Process -FilePath "dist\$setupName" -ArgumentList '--silent' -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "ReignSetup failed (exit $($p.ExitCode))" }
    Write-Host 'Installed: search "Reign Hub" in the Start menu.'
}
