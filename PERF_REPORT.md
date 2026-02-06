# PERF Report

Baseline directory: `baseline_criterion`
Current directory: `target/criterion`

## Raw Comparison Output
```text
benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec
lexer/next_token_loop/comment_heavy|0.8668|0.8645|-0.26|496341980.32|497642672.98
lexer/next_token_loop/identifier_heavy|1.7975|1.7951|-0.13|386343231.92|386848084.47
lexer/next_token_loop/mixed_syntax|1.9259|1.9458|1.03|172859943.11|171091198.19
lexer/next_token_loop/string_escape_interp_heavy|1.4756|1.4871|0.78|240200870.67|238347892.68
lexer/tokenize/comment_heavy|1.2202|1.2095|-0.88|352604893.22|355724130.35
lexer/tokenize/identifier_heavy|2.4773|2.4657|-0.47|280324253.95|281645942.48
lexer/tokenize/mixed_syntax|2.8604|2.8344|-0.91|116384551.96|117451978.38
lexer/tokenize/string_escape_interp_heavy|2.5342|2.5165|-0.70|139866147.14|140851011.97
```

## Corpus: mixed
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/mixed_syntax | 2.8604 | 2.8344 | -0.91 | 116384551.96 | 117451978.38 |
| lexer/next_token_loop/mixed_syntax | 1.9259 | 1.9458 | 1.03 | 172859943.11 | 171091198.19 |

## Corpus: comment_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/comment_heavy | 1.2202 | 1.2095 | -0.88 | 352604893.22 | 355724130.35 |
| lexer/next_token_loop/comment_heavy | 0.8668 | 0.8645 | -0.26 | 496341980.32 | 497642672.98 |

## Corpus: ident_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/identifier_heavy | 2.4773 | 2.4657 | -0.47 | 280324253.95 | 281645942.48 |
| lexer/next_token_loop/identifier_heavy | 1.7975 | 1.7951 | -0.13 | 386343231.92 | 386848084.47 |

## Corpus: string_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/string_escape_interp_heavy | 2.5342 | 2.5165 | -0.70 | 139866147.14 | 140851011.97 |
| lexer/next_token_loop/string_escape_interp_heavy | 1.4756 | 1.4871 | 0.78 | 240200870.67 | 238347892.68 |
