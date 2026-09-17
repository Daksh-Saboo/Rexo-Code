
<#
.SYNOPSIS
    Installs Rexo Code and adds `rexo` to the user's PATH.

.DESCRIPTION
    Supports two modes:

    1. Release archive:
       If rexo.exe exists next to this script, it is installed directly.

    2. Source checkout:
       If rexo.exe is not present, the script builds the project with:
           cargo build --release

       Use -SkipBuild to reuse an existing:
           target\release\rexo.exe

    The executable is installed to:
        %LOCALAPPDATA%\RexoCode\bin

    That directory is added to the user's PATH without using setx.

.PARAMETER SkipBuild
    Reuse an existing target\release\rexo.exe instead of rebuilding.

.EXAMPLE
    .\install.ps1

.EXAMPLE
    .\install.ps1 -SkipBuild
#>

[CmdletBinding()]
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

# ------------------------------------------------------------
# Locate repository / installer directory
# ------------------------------------------------------------

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

if (-not $RepoRoot) {
    throw "Could not determine the installer directory."
}

Write-Host ""
Write-Host "Rexo Code Installer" -ForegroundColor Cyan
Write-Host "-------------------" -ForegroundColor Cyan
Write-Host "Installer directory: $RepoRoot"
Write-Host ""

# ------------------------------------------------------------
# Find Rexo executable
# ------------------------------------------------------------

$FlatExe = Join-Path $RepoRoot "rexo.exe"
$ReleaseExe = Join-Path $RepoRoot "target\release\rexo.exe"

# Release archive:
# rexo.exe is expected next to install.ps1.
if (Test-Path -LiteralPath $FlatExe -PathType Leaf) {

    Write-Host "Release executable found." -ForegroundColor Green
    Write-Host "Using: $FlatExe"

    $SourceExe = $FlatExe
}
else {

    # Source checkout
    if ($SkipBuild) {

        if (-not (Test-Path -LiteralPath $ReleaseExe -PathType Leaf)) {
            throw @"
-SkipBuild was specified, but the release executable was not found:

$ReleaseExe

Run the installer without -SkipBuild so Cargo can build Rexo Code.
"@
        }

        Write-Host "Using existing release build." -ForegroundColor Yellow
        Write-Host "Using: $ReleaseExe"

        $SourceExe = $ReleaseExe
    }
    else {

        # Check for Cargo
        $Cargo = Get-Command cargo -ErrorAction SilentlyContinue

        if (-not $Cargo) {
            throw @"
Cargo was not found on PATH.

Install Rust from:
https://rustup.rs/

Then open a NEW terminal and run this installer again.
"@
        }

        # Make sure this actually looks like a Rust project.
        $CargoToml = Join-Path $RepoRoot "Cargo.toml"

        if (-not (Test-Path -LiteralPath $CargoToml -PathType Leaf)) {
            throw @"
Cargo.toml was not found.

Expected a Rexo Code source checkout at:

$RepoRoot

If you downloaded a release archive, make sure rexo.exe is
in the same folder as install.ps1.
"@
        }

        Write-Host "Building Rexo Code..." -ForegroundColor Cyan
        Write-Host ""

        Push-Location $RepoRoot

        try {
            & cargo build --release

            if ($LASTEXITCODE -ne 0) {
                throw "Cargo build failed with exit code $LASTEXITCODE."
            }
        }
        finally {
            Pop-Location
        }

        Write-Host ""
        Write-Host "Build completed successfully." -ForegroundColor Green

        if (-not (Test-Path -LiteralPath $ReleaseExe -PathType Leaf)) {
            throw @"
Cargo reported a successful build, but the executable was not found:

$ReleaseExe

Check your Cargo configuration and binary name.
"@
        }

        $SourceExe = $ReleaseExe
    }
}

# ------------------------------------------------------------
# Installation directory
# ------------------------------------------------------------

$InstallDir = Join-Path $env:LOCALAPPDATA "RexoCode\bin"

Write-Host ""
Write-Host "Installing Rexo Code..." -ForegroundColor Cyan
Write-Host "Target: $InstallDir"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

# ------------------------------------------------------------
# Copy executable
# ------------------------------------------------------------

$TargetExe = Join-Path $InstallDir "rexo.exe"

Copy-Item `
    -LiteralPath $SourceExe `
    -Destination $TargetExe `
    -Force

if (-not (Test-Path -LiteralPath $TargetExe -PathType Leaf)) {
    throw "Rexo Code could not be copied to $TargetExe"
}

Write-Host "Installed executable:" -ForegroundColor Green
Write-Host "  $TargetExe"

# ------------------------------------------------------------
# Copy icon if available
# ------------------------------------------------------------

$FlatIcon = Join-Path $RepoRoot "rexo.ico"
$SourceIcon = Join-Path $RepoRoot "assets\rexo.ico"
$TargetIcon = Join-Path $InstallDir "rexo.ico"

$IconSource = $null

if (Test-Path -LiteralPath $FlatIcon -PathType Leaf) {
    $IconSource = $FlatIcon
}
elseif (Test-Path -LiteralPath $SourceIcon -PathType Leaf) {
    $IconSource = $SourceIcon
}

if ($IconSource) {
    Copy-Item `
        -LiteralPath $IconSource `
        -Destination $TargetIcon `
        -Force

    Write-Host "Installed icon:" -ForegroundColor Green
    Write-Host "  $TargetIcon"
}

# ------------------------------------------------------------
# Add installation directory to USER PATH
# ------------------------------------------------------------

Write-Host ""
Write-Host "Checking user PATH..." -ForegroundColor Cyan

$CurrentPath = [Environment]::GetEnvironmentVariable(
    "Path",
    "User"
)

if ([string]::IsNullOrWhiteSpace($CurrentPath)) {
    $PathEntries = @()
}
else {
    $PathEntries = $CurrentPath -split ";" |
        Where-Object {
            -not [string]::IsNullOrWhiteSpace($_)
        }
}

$NormalizedInstallDir = $InstallDir.TrimEnd('\')

$AlreadyOnPath = $false

foreach ($Entry in $PathEntries) {

    $NormalizedEntry = $Entry.Trim().TrimEnd('\')

    if ($NormalizedEntry -ieq $NormalizedInstallDir) {
        $AlreadyOnPath = $true
        break
    }
}

if ($AlreadyOnPath) {

    Write-Host "Rexo Code is already on your user PATH." -ForegroundColor Yellow
    Write-Host "  $InstallDir"
}
else {

    if ([string]::IsNullOrWhiteSpace($CurrentPath)) {
        $NewPath = $InstallDir
    }
    else {
        $NewPath = "$CurrentPath;$InstallDir"
    }

    [Environment]::SetEnvironmentVariable(
        "Path",
        $NewPath,
        "User"
    )

    Write-Host "Added Rexo Code to your user PATH." -ForegroundColor Green
    Write-Host "  $InstallDir"
}

# ------------------------------------------------------------
# Verify installation
# ------------------------------------------------------------

Write-Host ""
Write-Host "Installation complete!" -ForegroundColor Green
Write-Host ""

Write-Host "Installed version:" -ForegroundColor Cyan

try {
    & $TargetExe --version
}
catch {
    Write-Host "Could not run the version check automatically." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "IMPORTANT:" -ForegroundColor Yellow
Write-Host "Open a NEW PowerShell or CMD window before running 'rexo'."
Write-Host ""
Write-Host "Then run:"
Write-Host ""
Write-Host "    rexo" -ForegroundColor White
Write-Host ""
Write-Host "To uninstall later:"
Write-Host ""
Write-Host "    .\uninstall.ps1" -ForegroundColor White
Write-Host ""

