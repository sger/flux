# PERF Report

Baseline directory: `baseline_criterion`
Current directory: `target/criterion`

## Raw Comparison Output
```text
benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec
lexer/next_token_loop/comment_heavy|0.7907|0.8113|2.59|544083298.52|530325712.28
lexer/next_token_loop/identifier_heavy|2.1867|2.2364|2.28|317585311.14|310517834.84
lexer/next_token_loop/mixed_syntax|1.7004|1.8281|7.51|195782202.83|182102794.82
lexer/next_token_loop/string_escape_interp_heavy|1.6206|1.6689|2.98|218713541.96|212387828.02
lexer/tokenize/comment_heavy|0.8967|0.9505|6.00|479822012.54|452653604.84
lexer/tokenize/identifier_heavy|2.4743|2.5760|4.11|280668691.41|269588100.74
lexer/tokenize/mixed_syntax|2.2392|2.1294|-4.90|148674287.73|156342213.20
lexer/tokenize/string_escape_interp_heavy|1.8442|1.8844|2.18|192195328.80|188099034.53
```

## Corpus: mixed
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/mixed_syntax | 2.2392 | 2.1294 | -4.90 | 148674287.73 | 156342213.20 |
| lexer/next_token_loop/mixed_syntax | 1.7004 | 1.8281 | 7.51 | 195782202.83 | 182102794.82 |

## Corpus: comment_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/comment_heavy | 0.8967 | 0.9505 | 6.00 | 479822012.54 | 452653604.84 |
| lexer/next_token_loop/comment_heavy | 0.7907 | 0.8113 | 2.59 | 544083298.52 | 530325712.28 |

## Corpus: ident_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/identifier_heavy | 2.4743 | 2.5760 | 4.11 | 280668691.41 | 269588100.74 |
| lexer/next_token_loop/identifier_heavy | 2.1867 | 2.2364 | 2.28 | 317585311.14 | 310517834.84 |

## Corpus: string_heavy
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |
|---|---:|---:|---:|---:|---:|
| lexer/tokenize/string_escape_interp_heavy | 1.8442 | 1.8844 | 2.18 | 192195328.80 | 188099034.53 |
| lexer/next_token_loop/string_escape_interp_heavy | 1.6206 | 1.6689 | 2.98 | 218713541.96 | 212387828.02 |
