<#
.SYNOPSIS
    Removes rexo.exe and its PATH entry that install.ps1 added.

.DESCRIPTION
    Undoes exactly what install.ps1 did: deletes
    %LOCALAPPDATA%\RexoCode\bin\rexo.exe (and the folder, if it's now
    empty) and removes that folder from your user PATH. Doesn't touch
    this checkout, your rexo.toml, or anything else on PATH.

.EXAMPLE
    PS> .\uninstall.ps1
#>

$ErrorActionPreference = "Stop"

$InstallDir = Join-Path $env:LOCALAPPDATA "RexoCode\bin"
$Target = Join-Path $InstallDir "rexo.exe"

if (Test-Path $Target) {
    Remove-Item -Path $Target -Force
    Write-Host "Removed: $Target" -ForegroundColor Green
}
else {
    Write-Host "Nothing installed at $Target (already removed?)" -ForegroundColor Yellow
}

if ((Test-Path $InstallDir) -and ((Get-ChildItem -Path $InstallDir -Force | Measure-Object).Count -eq 0)) {
    Remove-Item -Path $InstallDir -Force
}

$CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($CurrentPath) {
    $PathEntries = $CurrentPath -split ";" | Where-Object { $_ -ne "" -and $_.TrimEnd('\') -ine $InstallDir.TrimEnd('\') }
    $NewPath = ($PathEntries -join ";")
    if ($NewPath -ne $CurrentPath) {
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host "Removed from your user PATH: $InstallDir" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "Done. Open a new terminal window for the PATH change to take effect." -ForegroundColor Cyan
