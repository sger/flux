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

---

## Higher-Order Array Builtins (map/filter/fold)

Performance benchmarks for Proposal 020 higher-order array builtins.

### Summary
- **map**: ~324µs for 100 elements, ~425µs for 1k, ~556µs for 2k
- **filter**: ~323µs for 100 elements, ~472µs for 1k, ~618µs for 2k
- **fold**: ~339µs for 100 elements, ~442µs for 1k, ~557µs for 2k
- **Chained operations**: ~356µs for 100 elements, ~599µs for 1k, ~870µs for 2k

### Throughput Analysis
| Operation | 100 elements | 1k elements | 2k elements | Elements/sec (2k) |
|-----------|-------------|-------------|-------------|-------------------|
| map       | 309 Kelem/s | 2.35 Melem/s | 3.59 Melem/s | 3,590,000 |
| filter    | 309 Kelem/s | 2.12 Melem/s | 3.23 Melem/s | 3,230,000 |
| fold      | 294 Kelem/s | 2.26 Melem/s | 3.58 Melem/s | 3,580,000 |
| chain (3 ops) | 280 Kelem/s | 1.67 Melem/s | 2.30 Melem/s | 2,300,000 |

### Detailed Results

| Benchmark | Mean Time | Throughput (MiB/s) | Throughput (elem/s) |
|-----------|-----------|-------------------|---------------------|
| vm/map_filter_fold/map_100 | 324.28 µs | 1.22 MiB/s | 308,356 elem/s |
| vm/map_filter_fold/map_1k | 425.64 µs | 11.01 MiB/s | 2,349,822 elem/s |
| vm/map_filter_fold/map_2k | 556.83 µs | 17.66 MiB/s | 3,591,895 elem/s |
| vm/map_filter_fold/filter_100 | 323.57 µs | 1.22 MiB/s | 309,118 elem/s |
| vm/map_filter_fold/filter_1k | 472.29 µs | 9.94 MiB/s | 2,117,377 elem/s |
| vm/map_filter_fold/filter_2k | 618.31 µs | 16.85 MiB/s | 3,235,003 elem/s |
| vm/map_filter_fold/fold_100 | 339.59 µs | 1.20 MiB/s | 294,468 elem/s |
| vm/map_filter_fold/fold_1k | 442.62 µs | 10.61 MiB/s | 2,259,563 elem/s |
| vm/map_filter_fold/fold_2k | 558.38 µs | 18.66 MiB/s | 3,582,463 elem/s |
| vm/map_filter_fold/chain_100 | 356.62 µs | 1.46 MiB/s | 280,490 elem/s |
| vm/map_filter_fold/chain_1k | 599.50 µs | 8.03 MiB/s | 1,668,611 elem/s |
| vm/map_filter_fold/chain_2k | 870.51 µs | 12.10 MiB/s | 2,298,365 elem/s |

### Performance Characteristics
- **Linear scaling**: Performance scales linearly with array size (O(n) as expected)
- **Consistent overhead**: ~320-340µs base overhead for VM setup and builtin dispatch
- **Chain efficiency**: Chaining operations shows minimal additional overhead beyond individual ops
- **Memory throughput**: 10-18 MiB/s sustained throughput for large arrays

### Known Limitations
⚠️ **Stack Overflow**: Arrays larger than ~2.5k elements cause stack overflow due to deep frame nesting in callback invocations. This is a blocking limitation that must be addressed before production use with large datasets. See Proposal 020 for mitigation strategies.
