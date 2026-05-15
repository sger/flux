#requires -Version 7.0
<#
.SYNOPSIS
  Rebuild flux-lsp, repackage the VS Code extension, and reinstall it.

.DESCRIPTION
  PowerShell sibling of scripts/rebuild-vsix.sh. Mirrors the Windows
  "Develop and rebuild" workflow in editors/vscode/README.md.

.PARAMETER NoInstall
  Build and package only; skip the uninstall/install steps.

.EXAMPLE
  ./scripts/rebuild-vsix.ps1
  ./scripts/rebuild-vsix.ps1 -NoInstall
#>

[CmdletBinding()]
param(
    [switch]$NoInstall
)

$ErrorActionPreference = 'Stop'

$RepoRoot  = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$VscodeDir = Join-Path $RepoRoot 'editors\vscode'

# Platform suffix: .exe on Windows, empty elsewhere (pwsh runs cross-platform).
$ExeSuffix = if ($IsWindows) { '.exe' } else { '' }

# ── step 1: build the server binary ───────────────────────────────────────────
Write-Host '==> Building flux-lsp (release)...'
cargo build --release -p flux-lsp --manifest-path (Join-Path $RepoRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }

# ── step 2: stage the binary ──────────────────────────────────────────────────
Write-Host '==> Staging binary...'
$ServerDir = Join-Path $VscodeDir 'server'
if (-not (Test-Path $ServerDir)) { New-Item -ItemType Directory -Path $ServerDir | Out-Null }
$Src = Join-Path $RepoRoot ("target\release\flux-lsp" + $ExeSuffix)
$Dst = Join-Path $ServerDir ("flux-lsp" + $ExeSuffix)
Copy-Item -Path $Src -Destination $Dst -Force

# ── step 3: install JS deps ───────────────────────────────────────────────────
Write-Host '==> Installing JS dependencies...'
npm install --prefix $VscodeDir --silent
if ($LASTEXITCODE -ne 0) { throw "npm install failed ($LASTEXITCODE)" }

# ── step 4: compile TypeScript ────────────────────────────────────────────────
Write-Host '==> Compiling TypeScript...'
npm run compile --prefix $VscodeDir
if ($LASTEXITCODE -ne 0) { throw "npm run compile failed ($LASTEXITCODE)" }

# ── step 5: package ───────────────────────────────────────────────────────────
Write-Host '==> Packaging extension...'
Push-Location $VscodeDir
try {
    $packageOutput = npx --yes '@vscode/vsce' package --no-git-tag-version 2>&1
    if ($LASTEXITCODE -ne 0) {
        $packageOutput | ForEach-Object { Write-Host $_ }
        throw "vsce package failed ($LASTEXITCODE)"
    }

    $VsixPath = $null
    foreach ($line in $packageOutput) {
        if ($line -match 'Packaged:\s*(.+?)(?:\s*\(.*\))?\s*$') {
            $VsixPath = $Matches[1].Trim()
            break
        }
    }

    # Fallback: pick the most recently created .vsix in editors/vscode/.
    if ([string]::IsNullOrEmpty($VsixPath)) {
        $latest = Get-ChildItem -Path $VscodeDir -Filter '*.vsix' -File `
            | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($null -ne $latest) { $VsixPath = $latest.FullName }
    }
}
finally {
    Pop-Location
}

if ([string]::IsNullOrEmpty($VsixPath)) {
    throw 'Could not locate the packaged .vsix file.'
}

Write-Host "==> Packaged: $VsixPath"

# ── step 6: install into VS Code ──────────────────────────────────────────────
if ($NoInstall) {
    Write-Host '==> Skipping install (-NoInstall passed).'
    Write-Host ''
    Write-Host 'To install manually:'
    Write-Host '  code --uninstall-extension flux.flux-language'
    Write-Host "  code --install-extension `"$VsixPath`""
    Write-Host 'Then fully restart VS Code (File -> Exit, not Reload Window).'
    return
}

if (-not (Get-Command code -ErrorAction SilentlyContinue)) {
    Write-Warning "'code' CLI not found on PATH — skipping install."
    Write-Host "Install manually: code --install-extension `"$VsixPath`""
    return
}

Write-Host '==> Uninstalling previous version (if any)...'
& code --uninstall-extension flux.flux-language 2>&1 | Out-Null

Write-Host "==> Installing $VsixPath..."
& code --install-extension $VsixPath
if ($LASTEXITCODE -ne 0) { throw "code --install-extension failed ($LASTEXITCODE)" }

Write-Host ''
Write-Host 'Done. Fully restart VS Code (File -> Exit) to activate the new version.'
