# PERF Report

Baseline directory: `baseline_criterion`
Current directory: `target/criterion`

## Raw Comparison Output
```text
benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec
lexer/next_token_loop/comment_heavy|1.2390|1.2218|-1.39|347253420.95|352130807.11
lexer/next_token_loop/identifier_heavy|2.9685|3.0752|3.60|233937349.52|225819136.44
lexer/next_token_loop/mixed_syntax|2.4174|2.4271|0.40|137711261.98|137164025.78
lexer/next_token_loop/string_escape_interp_heavy|2.0119|2.1077|4.76|176177415.26|168170777.02
lexer/tokenize/comment_heavy|1.3576|1.2950|-4.62|316897288.60|332232346.02
lexer/tokenize/identifier_heavy|3.7695|3.8390|1.84|184227751.77|180893204.06
lexer/tokenize/mixed_syntax|3.5438|3.5104|-0.94|93941269.28|94835400.99
lexer/tokenize/string_escape_interp_heavy|2.8865|2.8167|-2.42|122795028.98|125839894.81
```

## Corpus: mixed
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/mixed_syntax | 3.5438 | 3.5104 | -0.94 | 93941269.28 | 94835400.99 |
| lexer/next_token_loop/mixed_syntax | 2.4174 | 2.4271 | 0.40 | 137711261.98 | 137164025.78 |

## Corpus: comment_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/comment_heavy | 1.3576 | 1.2950 | -4.62 | 316897288.60 | 332232346.02 |
| lexer/next_token_loop/comment_heavy | 1.2390 | 1.2218 | -1.39 | 347253420.95 | 352130807.11 |

## Corpus: ident_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/identifier_heavy | 3.7695 | 3.8390 | 1.84 | 184227751.77 | 180893204.06 |
| lexer/next_token_loop/identifier_heavy | 2.9685 | 3.0752 | 3.60 | 233937349.52 | 225819136.44 |

## Corpus: string_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/string_escape_interp_heavy | 2.8865 | 2.8167 | -2.42 | 122795028.98 | 125839894.81 |
| lexer/next_token_loop/string_escape_interp_heavy | 2.0119 | 2.1077 | 4.76 | 176177415.26 | 168170777.02 |

## Phase 5 Runtime Benchmarks (2026-02-10)

Command:
- `cargo bench --bench array_passing_bench -- --noplot`
- `cargo bench --bench closure_capture_bench -- --noplot`

### vm/array_passing
| Benchmark | Time (Current) | Throughput (Current) | Criterion Change |
|---|---:|---:|---:|
| vm/array_passing/array_pass_1k_x256 | 392.69-417.94 us | 21.764-23.164 MiB/s | +7.05% to +12.41% |
| vm/array_passing/array_pass_2k_x256 | 411.31-421.88 us | 35.125-36.027 MiB/s | +0.37% to +2.77% (within noise) |
| vm/array_passing/array_pass_chain_1k_x256 | 438.46-464.42 us | 19.701-20.867 MiB/s | +3.59% to +8.23% |

### vm/closure_capture
| Benchmark | Time (Current) | Throughput (Current) | Criterion Change |
|---|---:|---:|---:|
| vm/closure_capture/array_capture_1k | 436.28-458.81 us | 18.664-19.627 MiB/s | +6.15% to +12.22% |
| vm/closure_capture/string_capture_64k | 393.69-415.29 us | 156.62-165.21 MiB/s | +1.12% to +5.18% |
| vm/closure_capture/hash_capture_1k | 642.34-677.08 us | 25.923-27.326 MiB/s | +4.15% to +9.53% |
| vm/closure_capture/nested_capture_array_1k | 430.62-438.33 us | 19.594-19.945 MiB/s | -5.73% to -1.50% |
| vm/closure_capture/repeated_calls_captured_array | 657.46-670.42 us | 40.127-40.918 MiB/s | -8.30% to -4.97% |
| vm/closure_capture/capture_only_array_1k | 442.28-453.16 us | 19.092-19.562 MiB/s | no significant change |
| vm/closure_capture/no_capture_only_baseline | 372.39-378.61 us | 10.408-10.582 MiB/s | no significant change |
| vm/closure_capture/call_only_captured_array_1k | 419.44-425.95 us | 20.104-20.415 MiB/s | no significant change |
| vm/closure_capture/create_and_call_captured_array_1k | 538.03-552.37 us | 34.543-35.463 MiB/s | no significant change |
