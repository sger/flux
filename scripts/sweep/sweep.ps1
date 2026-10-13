#Requires -Version 7
# Compile every Flux file in the corpus, without running any of them, and
# record how each one ended. Two sweeps of the same corpus — say `main` and a
# branch — are compared with compare.ps1, which is how "no file newly
# rejected" is measured: a test or parity fixture only covers a file someone
# wrote one for.
#
# Usage:
#   ./scripts/sweep/sweep.ps1 -OutDir <dir> [-Mode cfg|core|core-opt] [-Files <list.txt>]
#                             [-Jobs <n>] [-TimeoutSec <s>] [-Flux <path>]
#
# Modes (each type-checks the file and lowers it, then exits):
#   cfg       --dump-cfg                 the VM path (default)
#   core      --dump-core                the Core path, as dumps and LLVM see it
#   core-opt  --optimize --dump-core     the same with AST optimizations
#
# Every run uses --no-cache, so no cached module hides a change.
#
# Writes to <dir>:
#   runs/<file>.txt   each file's output
#   results.tsv       file, status, exit code, diagnostic codes, milliseconds
#   summary.txt       how many files ended each way
#   errors.txt        the files that did not end `ok`
#
# Statuses: ok, error (diagnostics, none internal), ice (an internal compiler
# error), panic (the compiler panicked), timeout, crash (any other failure).

param(
    [Parameter(Mandatory = $true)]
    [string]$OutDir,
    [ValidateSet('cfg', 'core', 'core-opt')]
    [string]$Mode = 'cfg',
    [string]$Files,
    [int]$Jobs = [Math]::Max(1, [Environment]::ProcessorCount - 2),
    [int]$TimeoutSec = 120,
    [string]$Flux
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $RepoRoot

if (-not $Flux) {
    $exe = if ($IsWindows) { 'flux.exe' } else { 'flux' }
    $Flux = Join-Path $RepoRoot "target/debug/$exe"
}
if (-not (Test-Path $Flux)) {
    throw "no compiler at $Flux; run cargo build first"
}
$Flux = (Resolve-Path $Flux).Path

$ModeFlags = @{
    'cfg'      = @('--dump-cfg')
    'core'     = @('--dump-core')
    'core-opt' = @('--optimize', '--dump-core')
}[$Mode]

# The corpus: every .flx under examples/, tests/ and lib/, or the given list.
$corpus = if ($Files) {
    Get-Content $Files | Where-Object { $_.Trim() } | ForEach-Object { $_.Trim() -replace '\\', '/' }
} else {
    Get-ChildItem examples, tests, lib -Recurse -Filter *.flx -File |
        ForEach-Object { [IO.Path]::GetRelativePath($RepoRoot, $_.FullName) -replace '\\', '/' }
}
$corpus = @($corpus | Sort-Object -CaseSensitive -Unique)

if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
$Runs = Join-Path $OutDir 'runs'
New-Item -ItemType Directory -Force $Runs | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Runs = (Resolve-Path $Runs).Path

# Run a copy: a running compiler locks its file on Windows, so a build during
# the sweep would fail, and a rebuild would change the compiler halfway.
$copy = Join-Path $OutDir (Split-Path -Leaf $Flux)
Copy-Item $Flux $copy
$Flux = $copy

Write-Host ("sweep: {0} files, mode {1} ({2}), {3} jobs" -f $corpus.Count, $Mode, ($ModeFlags -join ' '), $Jobs)
$started = Get-Date

$results = $corpus | ForEach-Object -ThrottleLimit $Jobs -Parallel {
    $file = $_
    $flux = $using:Flux
    $flags = $using:ModeFlags
    $runs = $using:Runs
    $timeoutMs = $using:TimeoutSec * 1000

    $psi = [Diagnostics.ProcessStartInfo]::new($flux)
    foreach ($arg in @('--no-cache', '--quiet') + $flags + @($file)) { $psi.ArgumentList.Add($arg) }
    $psi.WorkingDirectory = $using:RepoRoot
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.Environment['NO_COLOR'] = '1'

    $clock = [Diagnostics.Stopwatch]::StartNew()
    $proc = [Diagnostics.Process]::Start($psi)
    $stdout = $proc.StandardOutput.ReadToEndAsync()
    $stderr = $proc.StandardError.ReadToEndAsync()
    $finished = $proc.WaitForExit($timeoutMs)
    if (-not $finished) {
        try { $proc.Kill($true) } catch {}
        $proc.WaitForExit()
    }
    $clock.Stop()
    # The dump goes to stdout and is long; diagnostics go to stderr. Keep the
    # dump's tail only, so a run's file stays readable.
    $err = $stderr.Result
    $out = $stdout.Result
    $text = ($err -replace "`e\[[0-9;]*m", '')
    $outTail = ($out -split "`n" | Select-Object -Last 40) -join "`n"

    $codes = @([regex]::Matches($text, '\[(E\d{3,4})\]') | ForEach-Object { $_.Groups[1].Value } |
        Sort-Object -Unique)
    $exit = if ($finished) { $proc.ExitCode } else { -1 }
    $status = if (-not $finished) { 'timeout' }
        elseif ($text -match '(?i)internal compiler error') { 'ice' }
        elseif ($text -match "panicked at|thread '.*' has overflowed its stack") { 'panic' }
        elseif ($exit -eq 0) { 'ok' }
        elseif ($codes.Count -gt 0) { 'error' }
        else { 'crash' }

    $name = ($file -replace '[\\/:]', '__')
    Set-Content -LiteralPath (Join-Path $runs "$name.txt") -Encoding utf8 -Value (
        "file: $file`nstatus: $status`nexit: $exit`ncodes: $($codes -join ',')`n`n--- stderr`n$text`n--- stdout (tail)`n$outTail")

    [pscustomobject]@{
        File   = $file
        Status = $status
        Exit   = $exit
        Codes  = ($codes -join ',')
        Ms     = [int]$clock.ElapsedMilliseconds
    }
}

$results = @($results | Sort-Object File -CaseSensitive)
$results | ForEach-Object { "{0}`t{1}`t{2}`t{3}`t{4}" -f $_.File, $_.Status, $_.Exit, $_.Codes, $_.Ms } |
    Set-Content -Encoding utf8 (Join-Path $OutDir 'results.tsv')

$summary = @(
    "mode: $Mode ($($ModeFlags -join ' '))"
    "files: $($results.Count)"
    "elapsed: $([int]((Get-Date) - $started).TotalSeconds)s"
    ''
) + @($results | Group-Object Status | Sort-Object Name | ForEach-Object { "{0,-8} {1}" -f $_.Name, $_.Count })
$summary | Set-Content -Encoding utf8 (Join-Path $OutDir 'summary.txt')

$results | Where-Object Status -ne 'ok' |
    ForEach-Object { "{0}`t{1}`t{2}" -f $_.Status, $_.Codes, $_.File } |
    Set-Content -Encoding utf8 (Join-Path $OutDir 'errors.txt')

$summary | ForEach-Object { Write-Host $_ }
