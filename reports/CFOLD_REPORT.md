# Constant Folding Benchmark Report

- Generated: 2026-03-10 11:02:10 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `cfold/flux` | 67.6 ± 25.2 | 30.4 | 113.3 | 1.69 ± 2.15 |
| `cfold/flux-jit` | 83.6 ± 25.4 | 38.2 | 123.7 | 2.09 ± 2.63 |
| `cfold/rust` | 118.6 ± 55.9 | 10.8 | 179.0 | 2.97 ± 3.87 |
| `cfold/python` | 259.8 ± 179.1 | 72.7 | 540.7 | 6.50 ± 9.10 |
| `cfold/haskell` | 56.8 ± 35.6 | 23.0 | 127.5 | 1.42 ± 1.95 |
| `cfold/ocaml` | 40.0 ± 48.7 | 5.2 | 152.9 | 1.00 |
