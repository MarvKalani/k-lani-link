@echo off
rem Double-click: OpenCode in the MXC sandbox, see README.md.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Start-OpenCode.ps1" %*
if errorlevel 1 pause
