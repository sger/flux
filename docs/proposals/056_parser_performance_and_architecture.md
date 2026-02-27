# Proposal 056: Parser Performance and Architecture Hardening

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `044_compiler_phase_pipeline_refactor.md`, `055_lexer_performance_and_architecture.md`

Implementation note (v0.0.4 M5 safe subset):
- Parser benchmark harness landed as `benches/parser_bench.rs`.
- Pratt parse-loop lookup now uses a single precedence table lookup in hot loop paths (`src/syntax/precedence.rs`, `src/syntax/parser/expression.rs`).

Evidence snapshot (baseline -> current):
- `parser/parse_program/expression_operator_heavy`: `18.951 .. 19.278 ms` -> `18.527 .. 18.933 ms`
- `parser/parse_program/string_interp_comment_heavy`: `4.6056 .. 4.6790 ms` -> `4.4197 .. 4.5559 ms`
- `parser/parse_program/malformed_recovery_heavy`: `5.5701 .. 5.6711 ms` -> `5.3812 .. 5.4969 ms` on isolated rerun
- logs:
  - baseline: `perf_logs/parser-bench-20260227-164004.log`
  - current: `perf_logs/parser-bench-20260227-175000.log`
  - malformed-only validation:
    - `perf_logs/parser-malformed-only-20260227-175351.log`
    - `perf_logs/parser-malformed-only-20260227-175409.log`

---

## 1. Summary

Harden Flux parser performance and architecture in a phased, behavior-preserving program: baseline first, hot-path optimization second, architecture cleanup third, parser/lexer contract stabilization last.

Target outcome: measurable parsing speed/allocation gains with zero grammar and diagnostics drift.

---

## 2. Motivation

Current parser design is functional but has avoidable cost and complexity risks:

1. Large dispatch paths in statement/expression parsing increase branch cost and maintenance friction.
2. Recovery and delimiter helpers are powerful but not fully contract-locked, increasing regression risk.
3. Lexer-parser assumptions (lookahead, doc-comment skip behavior, interpolation boundaries) are implicit.
4. Performance changes are hard to evaluate without standardized parser benchmarks/counters.

---

## 3. Goals

1. Improve parser throughput and/or reduce allocation pressure on representative corpora.
2. Preserve parser behavior, grammar acceptance, and diagnostic compatibility.
3. Make parser invariants explicit and test-locked.
4. Strengthen parser/lexer interface contracts to reduce drift.

---

## 4. Non-Goals

1. No syntax/grammar expansion.
2. No parser algorithm replacement (no generator migration in this proposal).
3. No diagnostic code/title policy changes.
4. No typed-AST migration (covered by proposal 046 track).

---

## 5. Scope

### In Scope

- `src/syntax/parser/mod.rs`
- `src/syntax/parser/expression.rs`
- `src/syntax/parser/statement.rs`
- `src/syntax/parser/helpers.rs`
- `src/syntax/parser/literal.rs`

### Out of Scope

- Runtime/compiler semantic changes
- Lexer Unicode semantics expansion
- Proposal-level language feature additions

---

## 6. Architecture and Performance Plan (4 Weeks)

### Week 1: Baseline + Instrumentation

1. Add parser benchmark harness with 4 corpora:
   - declaration/identifier-heavy
   - operator/expression-heavy
   - string/interpolation/comment-heavy
   - malformed/recovery-heavy

2. Add debug counters:
   - tokens consumed
   - lookahead transitions (`current/peek/peek2`)
   - recovery invocations (`synchronize`, delimiter recovery)
   - parser-side literal/escape decode hits

3. Publish baseline metrics table in this proposal.

### Week 2: Hot-Path Optimizations

1. Optimize statement dispatch ordering in `statement.rs` by common-path frequency.
2. Reduce repeated precedence/helper calls in Pratt loop (`expression.rs`).
3. Reduce transient allocations in literal/list parsing fast paths.
4. Keep parser-visible behavior unchanged.

### Week 3: Architecture Cleanup

1. Split parser flows into clearer internal lanes (declarations/control-flow/expr-stmt/module-import).
2. Normalize helper APIs for delimited/trailing-separator list parsing.
3. Add explicit invariants for lookahead and recovery transitions.
4. Isolate cold error-recovery paths from hot parse loops.

### Week 4: Contract Hardening + Stabilization

1. Add parser/lexer contract tests:
   - doc-comments
   - multiline/interpolation boundaries
   - unterminated string/comment behavior
   - lookahead assumptions

2. Re-run benchmarks and compare against baseline.
3. Finalize decisions and deferred work list.

---

## 7. Parser/Lexer Contract

Lock these invariants:

1. `next_token` maintains coherent `current/peek/peek2` progression.
2. `DocComment` skip behavior remains parser-compatible.
3. Interpolation token boundaries are stable for parser consumers.
4. Recovery entry/exit preserves delimiter and lookahead consistency.

---

## 8. Diagnostics Compatibility Contract

Must preserve:

1. Existing parser diagnostic codes/titles where semantics are unchanged.
2. Primary label stability for existing parser failures.
3. No unintended cascade inflation from recovery changes.

Any intentional wording update requires explicit snapshot review note.

---

## 9. Test Matrix

### Unit

1. Lookahead transition correctness.
2. Delimiter/recovery helper behavior.
3. Literal/interpolation edge handling.

### Integration

1. Parser regression fixtures across representative syntax categories.
2. Parser/lexer contract fixtures for doc comments and interpolation.
3. Existing parser/compiler integration tests remain green.

### Performance

1. Before/after throughput (`files/s`, `ns/token`).
2. Allocation profile delta where measurable.
3. No pathological regression on malformed corpus.

---

## 10. Validation Commands

1. `cargo fmt --all -- --check`
2. `cargo check --all --all-features`
3. `cargo test --lib`
4. `cargo test --test compiler_rules_tests`
5. `cargo test --test purity_vm_jit_parity_snapshots`
6. Parser benchmark command set (to be defined in implementation PR) run before/after.

---

## 11. Risks and Mitigations

1. **Risk:** perf changes alter token-consumption behavior  
   **Mitigation:** contract tests + parser regression fixtures.

2. **Risk:** helper refactor introduces recovery regressions  
   **Mitigation:** malformed corpus + recovery-focused unit tests.

3. **Risk:** benchmark noise misleads decisions  
   **Mitigation:** fixed corpora, repeated runs, median reporting.

4. **Risk:** architecture cleanup grows too broad  
   **Mitigation:** week gates and behavior-preserving scope lock.

---

## 12. Rollout and Governance

1. Implement in small PRs by week phase.
2. Require baseline/perf evidence for each optimization PR.
3. Block merges that change parser diagnostics without explicit review.
4. Update `docs/proposals/000_index.md` with proposal 056 status and action.

---

## 13. Acceptance Criteria

1. Parser behavior remains compatible for existing syntax/fixtures.
2. Parser/lexer contract invariants are explicit and test-covered.
3. Measurable speedup and/or allocation reduction is demonstrated.
4. No unintended diagnostic tuple drift (code/title/primary label).
5. Proposal 056 is execution-ready and evidence-backed.

---

## 14. Explicit Assumptions and Defaults

1. Phased balanced strategy is selected.
2. Parser compatibility is a hard constraint.
3. This is internal hardening only; no user-facing syntax change.
4. Typed-AST migration stays in proposal 046.
5. Lexer and parser perf tracks remain coordinated but independently scoped.
