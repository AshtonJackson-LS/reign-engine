@echo off
rem Rebuilds Reign from this folder and installs it on this computer, the
rem same way ReignSetup.exe installs it for users. Double-click to run.
rem Close Reign Hub and any editors first.
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-setup.ps1" -Install
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
