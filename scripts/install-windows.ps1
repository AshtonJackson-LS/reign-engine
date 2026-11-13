# Installs Reign Hub for the current user: copies the hub and the editor to
# %LOCALAPPDATA%\Programs\Reign Engine and adds a Start Menu shortcut (so
# "Reign Hub" shows up in Windows search) and a Desktop shortcut.
# Run after: cargo build --release -p reign_hub -p reign_engine
param([switch]$NoDesktop)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Reign Engine'
New-Item -ItemType Directory -Force $dest | Out-Null
foreach ($exe in 'reign-hub.exe', 'reign-engine.exe') {
    $src = Join-Path $root "target\release\$exe"
    $dst = Join-Path $dest $exe
    # A running .exe can't be overwritten, but it can be renamed away.
    if (Test-Path $dst) { Move-Item -Force $dst "$dst.old" -ErrorAction SilentlyContinue }
    Copy-Item -Force $src $dst
    Remove-Item "$dst.old" -ErrorAction SilentlyContinue
}
$shell = New-Object -ComObject WScript.Shell
$links = @(Join-Path ([Environment]::GetFolderPath('Programs')) 'Reign Hub.lnk')
if (-not $NoDesktop) { $links += Join-Path ([Environment]::GetFolderPath('Desktop')) 'Reign Hub.lnk' }
foreach ($link in $links) {
    $s = $shell.CreateShortcut($link)
    $s.TargetPath = Join-Path $dest 'reign-hub.exe'
    $s.WorkingDirectory = $dest
    $s.IconLocation = (Join-Path $dest 'reign-hub.exe') + ',0'
    $s.Description = 'Reign Hub: create and open Reign Engine projects'
    $s.Save()
    Write-Host "Shortcut: $link"
}
Write-Host "Installed to $dest"
