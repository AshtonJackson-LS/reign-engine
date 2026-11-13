@echo off
rem Rebuilds Reign Hub and the editor from this folder and reinstalls them
rem (Start Menu + Desktop shortcuts). Double-click to run.
cd /d "%~dp0"
echo Building Reign (release)...
cargo build --release -p reign_hub -p reign_engine
if errorlevel 1 goto failed
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\install-windows.ps1"
if errorlevel 1 goto failed
echo.
echo Done. Reign Hub is up to date.
pause
exit /b 0
:failed
echo.
echo Something went wrong, see the messages above.
pause
exit /b 1
