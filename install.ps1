<#
.SYNOPSIS
    Installs Rexo Code and adds `rexo` to the user PATH.

.DESCRIPTION
    Supports both:

      1. Release archive
         A folder containing:
           install.ps1
           rexo.exe
           rexo.ico

         The prebuilt executable is installed directly.

      2. Source checkout
         A folder containing:
           install.ps1
           Cargo.toml

         The script builds Rexo Code using:
           cargo build --release

         The resulting target\release\rexo.exe is installed.

    The executable is installed to:

        %LOCALAPPDATA%\RexoCode\bin

    The installation directory is added to the current user's PATH.

    The script modifies the user PATH through the Windows environment
    API instead of using `setx`, avoiding PATH truncation problems.

    IMPORTANT:
        Open a NEW Command Prompt or PowerShell window after installation
        for the PATH change to become available.

.PARAMETER SkipBuild
    Source checkout only.

    Reuses target\release\rexo.exe if it already exists instead of
    running cargo build.

.EXAMPLE
    PS> .\install.ps1

.EXAMPLE
    PS> .\install.ps1 -SkipBuild
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
$InstallDir = Join-Path $env:LOCALAPPDATA "RexoCode\bin"

$FlatExe = Join-Path $RepoRoot "rexo.exe"
$BuiltExe = Join-Path $RepoRoot "target\release\rexo.exe"

# ------------------------------------------------------------
# Header
# ------------------------------------------------------------

Write-Host ""
Write-Host "========================================" -ForegroundColor DarkCyan
Write-Host "          Rexo Code Installer" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor DarkCyan
Write-Host ""

# ------------------------------------------------------------
# Locate executable
# ------------------------------------------------------------

if (Test-Path -LiteralPath $FlatExe -PathType Leaf) {

    # Release archive
    Write-Host "Release archive detected." -ForegroundColor Cyan
    Write-Host "Using bundled rexo.exe..." -ForegroundColor Gray

    $ReleaseExe = $FlatExe
}
else {

    # Source checkout
    $ReleaseExe = $BuiltExe

    Write-Host "Source checkout detected." -ForegroundColor Cyan

    if ($SkipBuild -and (Test-Path -LiteralPath $ReleaseExe -PathType Leaf)) {

        Write-Host "Skipping build (-SkipBuild)." -ForegroundColor Yellow
        Write-Host "Using existing release executable." -ForegroundColor Gray
    }
    else {

        # Check Cargo
        $Cargo = Get-Command cargo -ErrorAction SilentlyContinue

        if (-not $Cargo) {
            Write-Host ""
            Write-Host "ERROR: Cargo was not found on PATH." -ForegroundColor Red
            Write-Host ""
            Write-Host "Install Rust from:" -ForegroundColor Yellow
            Write-Host "https://rustup.rs" -ForegroundColor White
            Write-Host ""
            Write-Host "Then open a NEW terminal and run this installer again." -ForegroundColor Yellow
            exit 1
        }

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

        Write-Host ""
        Write-Host "Build completed successfully." -ForegroundColor Green
    }
}

# ------------------------------------------------------------
# Verify executable
# ------------------------------------------------------------

if (-not (Test-Path -LiteralPath $ReleaseExe -PathType Leaf)) {

    Write-Host ""
    Write-Host "ERROR: Rexo executable was not found." -ForegroundColor Red
    Write-Host ""
    Write-Host "Expected:" -ForegroundColor Yellow
    Write-Host "  $ReleaseExe" -ForegroundColor White
    Write-Host ""

    if (-not (Test-Path -LiteralPath $FlatExe)) {
        Write-Host "If this is a release archive, make sure rexo.exe is" -ForegroundColor Gray
        Write-Host "located next to install.ps1." -ForegroundColor Gray
    }

    exit 1
}

# ------------------------------------------------------------
# Create installation directory
# ------------------------------------------------------------

Write-Host ""
Write-Host "Installing Rexo Code..." -ForegroundColor Cyan

New-Item `
    -ItemType Directory `
    -Force `
    -Path $InstallDir |
    Out-Null

# ------------------------------------------------------------
# Install executable
# ------------------------------------------------------------

$TargetExe = Join-Path $InstallDir "rexo.exe"

Copy-Item `
    -LiteralPath $ReleaseExe `
    -Destination $TargetExe `
    -Force

# ------------------------------------------------------------
# Install icon
# ------------------------------------------------------------

$FlatIcon = Join-Path $RepoRoot "rexo.ico"
$SourceIcon = Join-Path $RepoRoot "assets\rexo.ico"
$TargetIcon = Join-Path $InstallDir "rexo.ico"

if (Test-Path -LiteralPath $FlatIcon -PathType Leaf) {

    Copy-Item `
        -LiteralPath $FlatIcon `
        -Destination $TargetIcon `
        -Force

    Write-Host "Installed icon: $TargetIcon" -ForegroundColor Gray
}
elseif (Test-Path -LiteralPath $SourceIcon -PathType Leaf) {

    Copy-Item `
        -LiteralPath $SourceIcon `
        -Destination $TargetIcon `
        -Force

    Write-Host "Installed icon: $TargetIcon" -ForegroundColor Gray
}

Write-Host ""
Write-Host "Installed: $TargetExe" -ForegroundColor Green

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
    $PathEntries = @(
        $CurrentPath -split ";" |
        Where-Object {
            -not [string]::IsNullOrWhiteSpace($_)
        }
    )
}

$InstallDirNormalized = $InstallDir.TrimEnd("\").ToLowerInvariant()

$AlreadyOnPath = $false

foreach ($Entry in $PathEntries) {

    $EntryNormalized = $Entry.Trim().TrimEnd("\").ToLowerInvariant()

    if ($EntryNormalized -eq $InstallDirNormalized) {
        $AlreadyOnPath = $true
        break
    }
}

if ($AlreadyOnPath) {

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

    Write-Host "Added Rexo Code to your user PATH." -ForegroundColor Green
}

# ------------------------------------------------------------
# Verify installation
# ------------------------------------------------------------

Write-Host ""
Write-Host "========================================" -ForegroundColor DarkCyan
Write-Host "       Rexo Code Installation Done" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor DarkCyan
Write-Host ""

Write-Host "Installed to:" -ForegroundColor Gray
Write-Host "  $InstallDir" -ForegroundColor White
Write-Host ""

Write-Host "IMPORTANT:" -ForegroundColor Yellow
Write-Host "Open a NEW PowerShell or Command Prompt window." -ForegroundColor Yellow
Write-Host "The current terminal will not automatically receive" -ForegroundColor Gray
Write-Host "the updated PATH." -ForegroundColor Gray
Write-Host ""

Write-Host "Then run:" -ForegroundColor Cyan
Write-Host ""
Write-Host "    rexo" -ForegroundColor White
Write-Host ""

Write-Host "To remove Rexo Code later, run:" -ForegroundColor Gray
Write-Host ""
Write-Host "    .\uninstall.ps1" -ForegroundColor White
Write-Host ""

Write-Host "Installation complete." -ForegroundColor Green
Write-Host ""