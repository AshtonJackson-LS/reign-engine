@echo off
rem Makes a new Reign release. Double-click, answer the questions, done:
rem GitHub builds, signs and publishes it. See RELEASING.md.
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\release.ps1" %*
pause
