# PERF Report

Baseline directory: `baseline_criterion`
Current directory: `target/criterion`

## Raw Comparison Output
```text
benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec
lexer/next_token_loop/comment_heavy|1.3211|1.3211|0.00|325669642.19|325669642.19
lexer/next_token_loop/identifier_heavy|3.4259|3.4259|0.00|202708376.61|202708376.61
lexer/next_token_loop/mixed_syntax|2.4862|2.4862|0.00|133900387.93|133900387.93
lexer/next_token_loop/string_escape_interp_heavy|2.4798|2.4798|0.00|142933968.27|142933968.27
lexer/tokenize/comment_heavy|1.4065|1.4065|0.00|305899612.72|305899612.72
lexer/tokenize/identifier_heavy|4.0195|4.0195|0.00|172772180.60|172772180.60
lexer/tokenize/mixed_syntax|3.7504|3.7504|0.00|88766455.13|88766455.13
lexer/tokenize/string_escape_interp_heavy|3.2966|3.2966|0.00|107520547.73|107520547.73
```

## Corpus: mixed
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/mixed_syntax | 3.7504 | 3.7504 | 0.00 | 88766455.13 | 88766455.13 |
| lexer/next_token_loop/mixed_syntax | 2.4862 | 2.4862 | 0.00 | 133900387.93 | 133900387.93 |

## Corpus: comment_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/comment_heavy | 1.4065 | 1.4065 | 0.00 | 305899612.72 | 305899612.72 |
| lexer/next_token_loop/comment_heavy | 1.3211 | 1.3211 | 0.00 | 325669642.19 | 325669642.19 |

## Corpus: ident_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/identifier_heavy | 4.0195 | 4.0195 | 0.00 | 172772180.60 | 172772180.60 |
| lexer/next_token_loop/identifier_heavy | 3.4259 | 3.4259 | 0.00 | 202708376.61 | 202708376.61 |

## Corpus: string_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/string_escape_interp_heavy | 3.2966 | 3.2966 | 0.00 | 107520547.73 | 107520547.73 |
| lexer/next_token_loop/string_escape_interp_heavy | 2.4798 | 2.4798 | 0.00 | 142933968.27 | 142933968.27 |
