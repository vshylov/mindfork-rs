<#
.SYNOPSIS
    Install the pinned Inno Setup compiler and report the absolute path to ISCC.exe.

.DESCRIPTION
    The Windows installer (packaging/windows/mindfork.iss) is compiled by two
    workflows — release.yml (the real artifact) and packaging.yml (the syntax
    gate) — and they must use the *same* compiler, or the gate stops meaning
    anything about the release. That is why the version and its hash live here
    and not inline in both YAMLs.

    Both used to run `choco install innosetup`, which had three problems
    (docs/research/inno-setup-7.md §4): it pinned nothing, it reinstalled what
    the runner image already carries, and the community package has been frozen
    at 6.7.1 since 2026-02-17 while upstream shipped 6.7.2, 6.7.3, 7.0.2 and
    7.1.0. There is no 7.x on chocolatey at all, and the script now needs 7 for
    SetupArchitecture, so the compiler comes from the official GitHub release
    asset, verified against a pinned SHA-256.

    The path is *returned*, never searched for. Chocolatey puts an ISCC.exe shim
    on PATH, so `Get-Command ISCC.exe` on a GitHub runner resolves to the
    preinstalled 6.7.1 — a resolution that would keep compiling with Inno 6 while
    the log claims 7 was installed. The version is asserted for the same reason:
    a silent fallback to the wrong compiler is the failure mode worth spending a
    line on.

    Idempotent: an install of the pinned version already present is left alone,
    which is what makes the script usable on a developer machine to get exactly
    the build the releases are compiled with.

.PARAMETER Quiet
    Suppress the progress lines; the ISCC path is still written to stdout.

.OUTPUTS
    The absolute path to ISCC.exe (stdout). Under GitHub Actions the same path is
    also exported as the ISCC environment variable for later steps.

.EXAMPLE
    $iscc = ./tools/install_inno.ps1
    & $iscc /DAppVersion=0.0.0 /DBinDir=$PWD\stage packaging\windows\mindfork.iss
#>
[CmdletBinding()]
param([switch]$Quiet)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# The pin. Bumping Inno Setup is a deliberate edit of these three lines: the
# version, the URL's tag, and the hash from the release's asset digest
# (https://github.com/jrsoftware/issrc/releases). The x64 edition is the one
# upstream recommends; either edition builds either installer architecture.
$Version = '7.1.0'
$Url     = 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe'
$Sha256  = '0362A383ED217D4C4239B5933866DD96D3EB2102737DA92F80F6057A4B40DF2F'

$iscc = Join-Path $env:ProgramFiles 'Inno Setup 7\ISCC.exe'

function Write-Step([string]$Message) {
    if (-not $Quiet) { Write-Host $Message }
}

# `--version` prints the compiler engine version alone ("7.1.0") and exists only
# on 7, so this doubles as the "is it even the right major" check.
function Get-IsccVersion([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    try { return (& $Path --version 2>$null | Select-Object -First 1).Trim() } catch { return $null }
}

if ((Get-IsccVersion $iscc) -eq $Version) {
    Write-Step "Inno Setup $Version is already installed."
} else {
    $installer = Join-Path ([IO.Path]::GetTempPath()) "innosetup-$Version-x64.exe"
    Write-Step "Downloading Inno Setup $Version..."
    # Invoke-WebRequest's progress rendering costs more than the download on a
    # runner; the variable is restored so the script leaves no trace in a shell.
    $previousProgress = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try { Invoke-WebRequest -Uri $Url -OutFile $installer -UseBasicParsing }
    finally { $ProgressPreference = $previousProgress }

    $actual = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    if ($actual -ne $Sha256) {
        throw "Inno Setup $Version SHA-256 mismatch: expected $Sha256, got $actual"
    }
    Write-Step 'SHA-256 verified. Installing...'

    # /SP- suppresses the "This will install..." prompt that /VERYSILENT alone
    # leaves in place; /NORESTART because nothing here needs one.
    $process = Start-Process -FilePath $installer -Wait -PassThru `
        -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-'
    if ($process.ExitCode -ne 0) {
        throw "The Inno Setup installer exited with code $($process.ExitCode)"
    }
    Remove-Item -LiteralPath $installer -Force -ErrorAction SilentlyContinue
}

$installed = Get-IsccVersion $iscc
if ($installed -ne $Version) {
    throw "Expected ISCC $Version at '$iscc', found '$installed'"
}
Write-Step "ISCC $installed at $iscc"

if ($env:GITHUB_ENV) { "ISCC=$iscc" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8 }
$iscc
