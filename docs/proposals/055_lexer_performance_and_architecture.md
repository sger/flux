# Proposal 055: Lexer Performance and Architecture Hardening

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `014_lexer_parser_code_review.md`, `054_0_0_4_hm_adt_exhaustiveness_critical_path.md`

Implementation note (v0.0.4 M5 safe subset):
- Lexer benchmark harness remains the baseline source (`benches/lexer_bench.rs`), and M5 validation is gated by parser/diagnostics parity suites.

Evidence snapshot (baseline -> current):
- `lexer/next_token_loop/identifier_heavy`: `3.7637 .. 3.8832 ms` -> `3.7306 .. 3.8499 ms`
- `lexer/next_token_loop/string_escape_interp_heavy`: `2.7487 .. 2.8046 ms` -> `2.6624 .. 2.7470 ms`
- logs:
  - baseline: `perf_logs/lexer-bench-20260227-163754.log`
  - current: `perf_logs/lexer-bench-20260227-174828.log`

---

## 1. Summary

Improve Flux lexer in a phased sequence: measure first, optimize hot paths second, then refactor architecture with parser-contract guardrails.

Primary objective:
- measurable tokenization speedup with zero parser-visible behavior drift.

---

## 2. Scope

### In scope

1. Lexer hot-path performance in:
   - `src/syntax/lexer/mod.rs`
   - `src/syntax/lexer/reader.rs`
   - `src/syntax/lexer/strings.rs`
   - `src/syntax/lexer/comments.rs`
2. Token payload/ownership efficiency (span-backed paths, lower transient allocation).
3. Reader API simplification and invariant hardening.
4. Lexer-parser interface contract tests.
5. Benchmark and regression gates.

### Out of scope

1. Syntax/grammar changes.
2. Parser algorithm redesign.
3. Diagnostics code/title changes.
4. Unicode semantics expansion.

---

## 3. Motivation (Current State)

Flux lexer already has modular structure and byte-level fast paths, but there is still opportunity in:

1. tighter hot-loop dispatch in `next_token`,
2. reduced transient token payload allocation,
3. clearer `CharReader` contract and helper boundaries,
4. stronger token-stream regression protection for parser lookahead behavior.

This proposal targets maintainable wins rather than a risky rewrite.

---

## 4. Goals

1. Improve lexer throughput and/or allocation profile on representative corpora.
2. Keep token stream semantics stable for parser consumers.
3. Clarify reader/lexer invariants in code and tests.
4. Maintain diagnostics compatibility for lexing-origin failures.

---

## 5. Non-Goals

1. Change language syntax or token set semantics.
2. Introduce new parser parsing strategies.
3. Redefine interpolation/comment/string language behavior.

---

## 6. Architecture and Performance Plan

### Phase 1 (Week 1): Baseline + Instrumentation

1. Add benchmark harness with 4 corpora:
   - identifiers-heavy
   - operators/numbers-heavy
   - strings/interpolation-heavy
   - comments/doc-comments-heavy
2. Add debug counters:
   - token count
   - interner inserts/lookups
   - unicode slow-path hits
   - escape/interpolation path hits
3. Record baseline metrics and include artifact notes in this proposal.

### Phase 2 (Week 2): Hot-Path Perf Wins

1. Optimize `next_token` dispatch in `mod.rs`:
   - strict staged branch order
   - single cursor snapshot where possible
   - reduce repeated helper calls in hot loop
2. Reduce allocation churn:
   - keep span-backed token data where possible
   - avoid temporary string/literal creation in common paths
3. Keep identifier intern path deterministic and fast.

### Phase 3 (Week 3): Reader/State Architecture Cleanup

1. Consolidate `CharReader` helper usage into canonical fast path + explicit slow path.
2. Make reader invariants explicit and test-locked:
   - `position/read_position/current_char`
   - line/column updates including CRLF
3. Isolate interpolation transitions into focused state handlers while preserving behavior.

### Phase 4 (Week 4): Parser Contract Hardening + Stabilization

1. Add token-stream regression tests for parser-facing behavior:
   - doc comments
   - multiline strings
   - interpolation boundaries
   - unterminated comment/string tokens
2. Run parser integration tests to confirm no lookahead contract drift.
3. Re-run benchmarks and compare against baseline.
4. Publish final decisions and measured deltas.

---

## 7. Important Internal Interface Changes

1. No user-facing syntax changes.
2. Internal lexer interfaces may be simplified:
   - reader helper API normalization
   - interpolation state handling boundaries
3. Parser-visible token stream semantics must remain compatible.

---

## 8. Diagnostics Compatibility Contract

1. Unterminated string/comment token classes remain unchanged.
2. Illegal token handling remains unchanged in code/title category.
3. Any incidental message text updates must keep code/title/primary-label stability where snapshots enforce it.

---

## 9. Tests and Scenarios

### Unit scenarios

1. Reader position correctness across ASCII/newline/CRLF/Unicode.
2. String/interpolation transitions (`String`, `InterpolationStart`, `StringEnd`, unterminated).
3. Comment handling correctness (line/block/doc/unterminated).

### Integration scenarios

1. Parser token-lookahead stability (`current/peek/peek2`).
2. Token-stream snapshots for representative files.
3. Existing parser tests remain green.

### Performance scenarios

1. Throughput before/after benchmark comparison.
2. Allocation profile comparison.
3. Improvement target: measurable speedup and/or allocation reduction on at least 2/4 corpora with no semantic drift.

---

## 10. Rollout and Gates

Required checks:

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --lib
cargo test --test parser_tests
```

Performance/reliability checks:

1. benchmark runs before and after each phase,
2. token-stream regression suite remains stable,
3. parser integration remains stable.

---

## 11. Acceptance Criteria

1. Lexer behavior remains unchanged for existing syntax.
2. Parser integration and lookahead assumptions remain stable.
3. Lexing-origin diagnostics remain compatible.
4. Benchmarks show clear improvement in throughput and/or allocation profile.
5. Refactored lexer code has explicit invariants and reduced hot-path complexity.

---

## 12. Risks and Mitigations

1. Risk: perf tweaks alter token stream behavior.  
   Mitigation: token-stream regression snapshots + parser integration gates.
2. Risk: reader cleanup introduces position bugs.  
   Mitigation: dedicated invariant tests for byte offsets/line/column.
3. Risk: optimization complexity grows too large.  
   Mitigation: phase gates; stop at Phase 2 if gains are sufficient.

---

## 13. Explicit Assumptions and Defaults

1. Optimization strategy is phased, not big-bang rewrite.
2. Parser compatibility is a hard constraint.
3. Diagnostics compatibility is a hard constraint.
4. This track is post-0.0.4 under roadmap sequencing in `054`.
