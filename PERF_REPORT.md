# PERF Report

Baseline directory: `baseline_criterion`
Current directory: `target/criterion`

## Raw Comparison Output
```text
benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec
lexer/next_token_loop/comment_heavy|2.1690|1.4193|-34.56|198356071.57|303129618.83
lexer/next_token_loop/identifier_heavy|4.3723|1.5588|-64.35|158830569.10|445509613.29
lexer/next_token_loop/mixed_syntax|4.1164|1.7212|-58.19|80873333.16|193418752.53
lexer/next_token_loop/string_escape_interp_heavy|3.1274|1.6926|-45.88|113338613.50|209406561.20
lexer/tokenize/comment_heavy|2.7631|1.5985|-42.15|155705929.39|269147210.04
lexer/tokenize/identifier_heavy|5.4756|1.7836|-67.43|126825880.49|389351064.93
lexer/tokenize/mixed_syntax|7.4432|2.9615|-60.21|44726582.48|112410848.02
lexer/tokenize/string_escape_interp_heavy|4.5863|2.1354|-53.44|77284583.24|165987549.17
```

## Corpus: mixed
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/mixed_syntax | 7.4432 | 2.9615 | -60.21 | 44726582.48 | 112410848.02 |
| lexer/next_token_loop/mixed_syntax | 4.1164 | 1.7212 | -58.19 | 80873333.16 | 193418752.53 |

## Corpus: comment_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/comment_heavy | 2.7631 | 1.5985 | -42.15 | 155705929.39 | 269147210.04 |
| lexer/next_token_loop/comment_heavy | 2.1690 | 1.4193 | -34.56 | 198356071.57 | 303129618.83 |

## Corpus: ident_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/identifier_heavy | 5.4756 | 1.7836 | -67.43 | 126825880.49 | 389351064.93 |
| lexer/next_token_loop/identifier_heavy | 4.3723 | 1.5588 | -64.35 | 158830569.10 | 445509613.29 |

## Corpus: string_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/string_escape_interp_heavy | 4.5863 | 2.1354 | -53.44 | 77284583.24 | 165987549.17 |
| lexer/next_token_loop/string_escape_interp_heavy | 3.1274 | 1.6926 | -45.88 | 113338613.50 | 209406561.20 |
