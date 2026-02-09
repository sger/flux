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
