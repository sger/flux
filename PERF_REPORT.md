# PERF Report

Baseline directory: `baseline_criterion`
Current directory: `target/criterion`

## Raw Comparison Output
```text
benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec
lexer/next_token_loop/comment_heavy|0.7555|0.7555|0.00|569480560.58|569480560.58
lexer/next_token_loop/identifier_heavy|1.9860|1.9860|0.00|349669633.91|349669633.91
lexer/next_token_loop/mixed_syntax|1.6625|1.6625|0.00|200250759.94|200250759.94
lexer/next_token_loop/string_escape_interp_heavy|1.5213|1.5213|0.00|232997068.51|232997068.51
lexer/tokenize/comment_heavy|0.8283|0.8283|0.00|519434854.36|519434854.36
lexer/tokenize/identifier_heavy|2.2658|2.2658|0.00|306491929.30|306491929.30
lexer/tokenize/mixed_syntax|2.1785|2.1785|0.00|152814589.13|152814589.13
lexer/tokenize/string_escape_interp_heavy|1.7289|1.7289|0.00|205012054.08|205012054.08
```

## Corpus: mixed
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/mixed_syntax | 2.1785 | 2.1785 | 0.00 | 152814589.13 | 152814589.13 |
| lexer/next_token_loop/mixed_syntax | 1.6625 | 1.6625 | 0.00 | 200250759.94 | 200250759.94 |

## Corpus: comment_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/comment_heavy | 0.8283 | 0.8283 | 0.00 | 519434854.36 | 519434854.36 |
| lexer/next_token_loop/comment_heavy | 0.7555 | 0.7555 | 0.00 | 569480560.58 | 569480560.58 |

## Corpus: ident_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/identifier_heavy | 2.2658 | 2.2658 | 0.00 | 306491929.30 | 306491929.30 |
| lexer/next_token_loop/identifier_heavy | 1.9860 | 1.9860 | 0.00 | 349669633.91 | 349669633.91 |

## Corpus: string_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/string_escape_interp_heavy | 1.7289 | 1.7289 | 0.00 | 205012054.08 | 205012054.08 |
| lexer/next_token_loop/string_escape_interp_heavy | 1.5213 | 1.5213 | 0.00 | 232997068.51 | 232997068.51 |
