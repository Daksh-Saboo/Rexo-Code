@echo off
rem Thin wrapper around install.ps1 for people running from cmd.exe (or
rem double-clicking this file in Explorer) instead of a PowerShell prompt.
rem See install.ps1 for what this actually does.
setlocal
set "SCRIPT_DIR=%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%install.ps1" %*
if %ERRORLEVEL% neq 0 (
    echo.
    echo install.ps1 exited with an error - see above.
    pause
)
