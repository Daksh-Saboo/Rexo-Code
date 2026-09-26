<#
.SYNOPSIS
    Installs Rexo Code and adds `rexo` to the user's PATH.

.DESCRIPTION
    Supports both:

    1. Release archive
       - Expects rexo.exe next to this script.
       - Does not build anything.

    2. Source checkout
       - Expects Cargo.toml next to this script.
       - Builds with `cargo build --release`.
       - Use -SkipBuild to reuse an existing target\release\rexo.exe.

    The executable is installed to:

        %LOCALAPPDATA%\RexoCode\bin

    The directory is added to the user's PATH using the Windows
    Environment API instead of `setx`.

    A new terminal window is required after installation.

.PARAMETER SkipBuild
    Source checkout only. Reuses an existing release executable instead
    of running `cargo build --release`.

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
# Paths
# ------------------------------------------------------------

$RepoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

$FlatExe      = Join-Path $RepoRoot "rexo.exe"
$SourceExe    = Join-Path $RepoRoot "target\release\rexo.exe"
$CargoToml    = Join-Path $RepoRoot "Cargo.toml"

$InstallDir   = Join-Path $env:LOCALAPPDATA "RexoCode\bin"
$TargetExe    = Join-Path $InstallDir "rexo.exe"
$TargetIcon   = Join-Path $InstallDir "rexo.ico"

# ------------------------------------------------------------
# Detect release archive or source checkout
# ------------------------------------------------------------

if (Test-Path -LiteralPath $FlatExe) {

    # Release archive
    $ReleaseExe = $FlatExe

    Write-Host "Rexo Code release archive detected." -ForegroundColor Cyan
}
else {

    # Source checkout
    $ReleaseExe = $SourceExe

    if (-not (Test-Path -LiteralPath $CargoToml)) {
        throw @"
Could not find Rexo Code.

Expected either:

  $FlatExe

or:

  $CargoToml

Make sure install.ps1 is inside a Rexo Code release archive
or the root of the Rexo Code source repository.
"@
    }

    # Build unless SkipBuild is requested and the executable already exists.
    if (-not $SkipBuild -or -not (Test-Path -LiteralPath $ReleaseExe)) {

        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            throw @"
Cargo was not found on PATH.

Install Rust from:
https://rustup.rs

Then open a NEW terminal and run this installer again.
"@
        }

        Write-Host ""
        Write-Host "Building Rexo Code (release)..." -ForegroundColor Cyan
        Write-Host ""

        Push-Location $RepoRoot

        try {
            & cargo build --release

            if ($LASTEXITCODE -ne 0) {
                throw "cargo build failed with exit code $LASTEXITCODE."
            }
        }
        finally {
            Pop-Location
        }
    }
    else {
        Write-Host "Using existing release build." -ForegroundColor Yellow
    }
}

# ------------------------------------------------------------
# Verify executable
# ------------------------------------------------------------

if (-not (Test-Path -LiteralPath $ReleaseExe)) {
    throw @"
Rexo Code executable was not found:

  $ReleaseExe

Check the build output or make sure the release archive contains
rexo.exe next to install.ps1.
"@
}

# ------------------------------------------------------------
# Create installation directory
# ------------------------------------------------------------

New-Item `
    -ItemType Directory `
    -Force `
    -Path $InstallDir `
    | Out-Null

# ------------------------------------------------------------
# Install executable
# ------------------------------------------------------------

Copy-Item `
    -LiteralPath $ReleaseExe `
    -Destination $TargetExe `
    -Force

if (-not (Test-Path -LiteralPath $TargetExe)) {
    throw "Rexo Code installation failed. rexo.exe was not copied to $InstallDir."
}

# ------------------------------------------------------------
# Install icon
# ------------------------------------------------------------

$FlatIcon  = Join-Path $RepoRoot "rexo.ico"
$SourceIcon = Join-Path $RepoRoot "assets\rexo.ico"

$IconSource = $null

if (Test-Path -LiteralPath $FlatIcon) {
    $IconSource = $FlatIcon
}
elseif (Test-Path -LiteralPath $SourceIcon) {
    $IconSource = $SourceIcon
}

if ($IconSource) {
    Copy-Item `
        -LiteralPath $IconSource `
        -Destination $TargetIcon `
        -Force

    Write-Host "Installed icon: $TargetIcon" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "Installed Rexo Code:" -ForegroundColor Green
Write-Host "  $TargetExe" -ForegroundColor White

# ------------------------------------------------------------
# Add installation directory to USER PATH
# ------------------------------------------------------------

$CurrentPath = [Environment]::GetEnvironmentVariable(
    "Path",
    "User"
)

$PathEntries = @()

if (-not [string]::IsNullOrWhiteSpace($CurrentPath)) {

    $PathEntries = $CurrentPath `
        -split ";" `
        | ForEach-Object {
            $_.Trim().Trim('"')
        } `
        | Where-Object {
            -not [string]::IsNullOrWhiteSpace($_)
        }
}

$NormalizedInstallDir = $InstallDir.TrimEnd('\')

$AlreadyOnPath = $PathEntries | Where-Object {
    $_.TrimEnd('\') -ieq $NormalizedInstallDir
}

if ($AlreadyOnPath) {

    Write-Host ""
    Write-Host "Rexo Code is already on your user PATH." -ForegroundColor Yellow
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

    Write-Host ""
    Write-Host "Added Rexo Code to your user PATH:" -ForegroundColor Green
    Write-Host "  $InstallDir" -ForegroundColor White
}

# ------------------------------------------------------------
# Finished
# ------------------------------------------------------------

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Rexo Code installation complete!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "Open a NEW PowerShell or cmd.exe window." -ForegroundColor Yellow
Write-Host ""

Write-Host "Then run:" -ForegroundColor Cyan
Write-Host ""
Write-Host "    rexo" -ForegroundColor White
Write-Host ""

Write-Host "Installed version:" -ForegroundColor Cyan

try {
    & $TargetExe --version
}
catch {
    Write-Host "Unable to read the installed version." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "To remove Rexo Code later, run uninstall.ps1." -ForegroundColor DarkGray
Write-Host ""