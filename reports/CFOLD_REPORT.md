# Constant Folding Benchmark Report

- Generated: 2026-03-10 10:23:36 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `cfold/flux` | 74.8 ± 40.9 | 33.6 | 168.3 | 3.08 ± 1.84 |
| `cfold/flux-jit` | 69.7 ± 33.9 | 38.6 | 153.6 | 2.87 ± 1.55 |
| `cfold/rust` | 24.3 ± 5.7 | 15.6 | 32.8 | 1.00 |
| `cfold/python` | 103.3 ± 47.1 | 60.2 | 229.7 | 4.26 ± 2.18 |
| `cfold/haskell` | 36.9 ± 10.2 | 24.9 | 53.6 | 1.52 ± 0.55 |
| `cfold/ocaml` | 35.1 ± 22.2 | 16.6 | 94.4 | 1.45 ± 0.98 |
