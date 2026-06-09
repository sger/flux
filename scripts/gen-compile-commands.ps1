# gen-compile-commands.ps1 — regenerate runtime/c/compile_commands.json for clangd.
#
# The C runtime is built by runtime/c/Makefile with Unix tools (cc/ar) that don't
# run cleanly on Windows, so instead of shimming `make` we synthesize the compile
# database directly from the same flags the Makefile uses (CFLAGS in the Makefile).
#
# Run from anywhere:  pwsh -File scripts/gen-compile-commands.ps1

$ErrorActionPreference = 'Stop'

$runtimeDir = Join-Path $PSScriptRoot '..\runtime\c' | Resolve-Path
$dir = ($runtimeDir.Path -replace '\\', '/')

# Mirror Makefile CFLAGS. smoke_test.c is compiled with -DFLUX_RT_NO_MAIN.
$cflags = '-std=c11 -Wall -Wextra -Wpedantic -O2 -g'

$entries = Get-ChildItem -Path $runtimeDir -Filter '*.c' | ForEach-Object {
    $name = $_.Name
    $extra = if ($name -eq 'smoke_test.c') { ' -DFLUX_RT_NO_MAIN' } else { '' }
    [ordered]@{
        directory = $dir
        file      = "$dir/$name"
        command   = "clang $cflags$extra -c $name"
    }
}

$json = $entries | ConvertTo-Json -Depth 3
$out = Join-Path $runtimeDir 'compile_commands.json'
Set-Content -Path $out -Value $json -Encoding utf8
Write-Host "Wrote $out ($($entries.Count) entries)"
