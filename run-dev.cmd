@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\run-dev.ps1" %*
if errorlevel 1 (
  echo.
  echo Startup failed. If the terminal is restricted, double-click this file from Explorer,
  echo or open PowerShell from the Start menu and run scripts\run-dev.ps1.
  pause
)
endlocal
