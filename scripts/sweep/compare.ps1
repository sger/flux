#Requires -Version 7
# Compare two sweeps from sweep.ps1 — a base (say `main`) and a head (the
# branch) — file by file.
#
# Usage:
#   ./scripts/sweep/compare.ps1 -Base <sweep dir> -Head <sweep dir>
#
# Reports:
#   newly rejected   ok in base, anything else in head
#   new internal     ice, panic, timeout or crash in head, not in base
#   newly accepted   anything else in base, ok in head
#   codes changed    rejected in both, with different diagnostic codes
#   only in base / only in head
#
# Exits 1 when a file is newly rejected or newly internal, so a script can
# stop on it.

param(
    [Parameter(Mandatory = $true)][string]$Base,
    [Parameter(Mandatory = $true)][string]$Head
)

$ErrorActionPreference = 'Stop'

function Read-Sweep([string]$Dir) {
    $tsv = Join-Path $Dir 'results.tsv'
    if (-not (Test-Path $tsv)) { throw "no results.tsv in $Dir; is it a sweep.ps1 output?" }
    $map = @{}
    foreach ($line in Get-Content $tsv) {
        $f = $line -split "`t"
        $map[$f[0]] = [pscustomobject]@{ Status = $f[1]; Codes = $f[3] }
    }
    $map
}

$baseRuns = Read-Sweep $Base
$headRuns = Read-Sweep $Head
$internal = 'ice', 'panic', 'timeout', 'crash'

$newlyRejected = @(); $newInternal = @(); $newlyAccepted = @(); $codesChanged = @()
foreach ($file in ($headRuns.Keys | Sort-Object -CaseSensitive)) {
    $h = $headRuns[$file]
    if (-not $baseRuns.ContainsKey($file)) { continue }
    $b = $baseRuns[$file]
    if ($h.Status -in $internal -and $b.Status -notin $internal) {
        $newInternal += "{0} -> {1}  {2}" -f $b.Status, $h.Status, $file
    } elseif ($b.Status -eq 'ok' -and $h.Status -ne 'ok') {
        $newlyRejected += "ok -> {0} [{1}]  {2}" -f $h.Status, $h.Codes, $file
    } elseif ($b.Status -ne 'ok' -and $h.Status -eq 'ok') {
        $newlyAccepted += "{0} [{1}] -> ok  {2}" -f $b.Status, $b.Codes, $file
    } elseif ($b.Status -ne 'ok' -and $h.Codes -ne $b.Codes) {
        $codesChanged += "[{0}] -> [{1}]  {2}" -f $b.Codes, $h.Codes, $file
    }
}
$onlyBase = @($baseRuns.Keys | Where-Object { -not $headRuns.ContainsKey($_) } | Sort-Object -CaseSensitive)
$onlyHead = @($headRuns.Keys | Where-Object { -not $baseRuns.ContainsKey($_) } | Sort-Object -CaseSensitive)

function Report([string]$Title, [string[]]$Lines) {
    Write-Host ("{0}: {1}" -f $Title, $Lines.Count)
    $Lines | ForEach-Object { Write-Host "  $_" }
}

Write-Host "base: $Base"
Write-Host "head: $Head"
Report 'newly rejected' $newlyRejected
Report 'new internal' $newInternal
Report 'newly accepted' $newlyAccepted
Report 'codes changed' $codesChanged
Report 'only in base' $onlyBase
Report 'only in head' $onlyHead

if ($newlyRejected.Count -gt 0 -or $newInternal.Count -gt 0) { exit 1 }
