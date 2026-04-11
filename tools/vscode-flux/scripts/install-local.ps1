$ErrorActionPreference = "Stop"

$RootDir = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$ExtDir = Join-Path $RootDir "tools\vscode-flux"
$ExtId = "flux-local.flux-language-0.0.4"

function Install-To-Dir($BaseDir) {
    $TargetDir = Join-Path $BaseDir $ExtId
    if (Test-Path $TargetDir) {
        Remove-Item -Recurse -Force $TargetDir
    }
    Copy-Item -Recurse $ExtDir $TargetDir
    Write-Host "Installed to: $TargetDir"
}

# VS Code stable
$VsCodeDir = Join-Path $env:USERPROFILE ".vscode\extensions"
Install-To-Dir $VsCodeDir

# VS Code Insiders
$InsidersDir = Join-Path $env:USERPROFILE ".vscode-insiders\extensions"
if (Test-Path (Split-Path $InsidersDir)) {
    Install-To-Dir $InsidersDir
}

Write-Host "Done. Restart VS Code and open a .flx file."
