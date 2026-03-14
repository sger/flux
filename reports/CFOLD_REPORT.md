# Constant Folding Benchmark Report

- Generated: 2026-03-10 15:36:02 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `cfold/flux` | 13.6 ± 0.4 | 13.1 | 14.3 | 7.35 ± 0.80 |
| `cfold/flux-jit` | 32.6 ± 1.3 | 30.9 | 34.8 | 17.65 ± 2.00 |
| `cfold/rust` | 1.8 ± 0.2 | 1.6 | 2.1 | 1.00 |
| `cfold/python` | 14.3 ± 0.4 | 13.9 | 15.0 | 7.72 ± 0.84 |
| `cfold/haskell` | 10.9 ± 0.2 | 10.8 | 11.2 | 5.89 ± 0.63 |
