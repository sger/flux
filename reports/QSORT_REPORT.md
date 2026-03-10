# Quicksort Benchmark Report

- Generated: 2026-03-10 10:52:37 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `qsort/flux` | 132.7 ± 37.3 | 92.5 | 193.1 | 2.44 ± 1.78 |
| `qsort/flux-jit` | 112.2 ± 38.9 | 85.5 | 209.7 | 2.07 ± 1.56 |
| `qsort/rust` | 169.5 ± 193.4 | 7.2 | 659.4 | 3.12 ± 4.13 |
| `qsort/python` | 191.9 ± 79.7 | 101.1 | 364.6 | 3.53 ± 2.79 |
| `qsort/haskell` | 122.0 ± 63.3 | 48.0 | 221.4 | 2.25 ± 1.91 |
| `qsort/ocaml` | 54.3 ± 36.5 | 9.1 | 127.9 | 1.00 |
