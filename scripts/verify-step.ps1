#Requires -Version 7
# Verify one step of a change: lint, the full suite, the corpus sweeps and
# backend parity. Each run's outputs are tagged, so a step's results compare
# with the previous step's.
#
# Usage (from anywhere):
#   ./scripts/verify-step.ps1 <tag>
#   ./scripts/verify-step.ps1 <tag> -Base <earlier tag>   # also diff the sweeps
#   ./scripts/verify-step.ps1 <tag> -SkipSuite            # skip the test suite
#   ./scripts/verify-step.ps1 <tag> -SkipSweeps           # skip the sweeps
#
# Outputs go to .git/parked/out/ (untracked, survives `cargo clean`):
#   test_<tag>.txt          the suite
#   sweep_<tag>/            every example compiled with --dump-cfg
#   dump_<tag>__dump_core/            ... with --dump-core
#   dump_<tag>__optimize___dump_core/ ... with --optimize --dump-core
#   parity_<tag>.txt        vm, vm_cached, vm_strict and llvm compared
#
# The sweeps are scripts/sweep/sweep.ps1; -Base runs scripts/sweep/compare.ps1
# on each against the same sweep of the earlier tag. A sweep's base must come
# from the same tools, so run one on main first: check out main, build, and
# `./scripts/verify-step.ps1 main -SkipSuite`.

param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Tag,
    [string]$Base,
    [switch]$SkipSuite,
    [switch]$SkipSweeps
)

$ErrorActionPreference = 'Continue'
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot
$Parked = Join-Path $RepoRoot '.git/parked'
$Out = Join-Path $Parked 'out'
New-Item -ItemType Directory -Force $Out | Out-Null

function Section([string]$Title) { Write-Host "== $Title" -ForegroundColor Cyan }
function Strip-Ansi([string]$Line) { $Line -replace "`e\[[0-9;]*m", '' }

# Lint and test with the flags CI uses, so the llvm-feature tests run too.
Section 'fmt, clippy'
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { Write-Warning 'FMT: run cargo fmt' }
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 |
    ForEach-Object { "$_" } |
    Select-String -Pattern '^(warning|error)' -Context 0, 6 |
    Select-Object -First 20 |
    ForEach-Object { $_.ToString() }

if (-not $SkipSuite) {
    Section 'suite'
    $TestLog = Join-Path $Out "test_$Tag.txt"
    $env:INSTA_FORCE_PASS = '1'
    cargo test --all --all-features --no-fail-fast 2>&1 |
        ForEach-Object { "$_" } |
        Tee-Object -FilePath $TestLog |
        Where-Object { $_ -match '^\s+Running |^test result' }
    $TestExit = $LASTEXITCODE
    Remove-Item Env:INSTA_FORCE_PASS
    Write-Host "tests exit $TestExit"
    Select-String -Path $TestLog -Pattern '^test result: FAILED' | ForEach-Object { $_.Line }
    Get-ChildItem -Recurse -Filter '*.snap.new' -File |
        Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' } |
        ForEach-Object { Resolve-Path -Relative $_.FullName }
}

# The suite built target/debug/flux with every feature; without it, build it
# here, for the sweeps and parity.
if ($SkipSuite) { cargo build --all-features --quiet }

if (-not $SkipSweeps) {
    $sweeps = [ordered]@{
        "sweep_$Tag"                       = 'cfg'
        "dump_${Tag}__dump_core"           = 'core'
        "dump_${Tag}__optimize___dump_core" = 'core-opt'
    }
    foreach ($dir in $sweeps.Keys) {
        Section "sweep $dir ($($sweeps[$dir]))"
        & (Join-Path $PSScriptRoot 'sweep/sweep.ps1') -OutDir (Join-Path $Out $dir) -Mode $sweeps[$dir]
    }

    Section 'internal errors'
    foreach ($dir in $sweeps.Keys) {
        $errors = Join-Path $Out "$dir/errors.txt"
        $count = @(Get-Content $errors -ErrorAction SilentlyContinue |
            Where-Object { $_ -match '^(ice|panic|timeout|crash)\t' }).Count
        Write-Host "${dir}: $count"
    }

    if ($Base) {
        foreach ($dir in $sweeps.Keys) {
            Section "compare $dir with $Base"
            $baseDir = Join-Path $Out ($dir -replace [regex]::Escape($Tag), $Base)
            & (Join-Path $PSScriptRoot 'sweep/compare.ps1') -Base $baseDir -Head (Join-Path $Out $dir)
        }
    }
}

Section 'parity'
$Flux = Join-Path $RepoRoot 'target/debug/flux.exe'
$ParityLog = Join-Path $Out "parity_$Tag.txt"
Remove-Item $ParityLog -ErrorAction SilentlyContinue
foreach ($dir in 'examples/type_classes', 'examples/generics', 'tests/flux', 'tests/parity') {
    "=== $dir" | Tee-Object -FilePath $ParityLog -Append
    & $Flux parity-check $dir --ways vm,vm_cached,vm_strict,llvm 2>&1 |
        ForEach-Object { "$_" } |
        Where-Object { $_ -notmatch '^\s+Compiling|^\s+Finished' } |
        Tee-Object -FilePath $ParityLog -Append |
        ForEach-Object { Strip-Ansi $_ } |
        Where-Object { $_ -match '^(PASS|MISMATCH|EXPECTED|SKIP|Total|Pass|Mismatch)' }
}

Write-Host "all-done: $Tag"
