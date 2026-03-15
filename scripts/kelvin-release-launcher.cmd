@echo off
setlocal
powershell -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0kelvin.ps1" %*
exit /b %ERRORLEVEL%
