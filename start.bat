@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1"
if errorlevel 1 (
  echo [buchcraft] start.ps1 exited with error - window held open
  pause
  exit /b 1
)
pause
