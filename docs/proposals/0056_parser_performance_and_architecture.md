- Feature Name: Parser Performance and Architecture Hardening
- Start Date: 2026-02-26
- Proposal PR: 
- Flux Issue: 

# Proposal 0056: Parser Performance and Architecture Hardening

## Summary
[summary]: #summary

Harden Flux parser performance and architecture in a phased, behavior-preserving program: baseline first, hot-path optimization second, architecture cleanup third, parser/lexer contract stabilization last.

## Motivation
[motivation]: #motivation

Current parser design is functional but has avoidable cost and complexity risks: Current parser design is functional but has avoidable cost and complexity risks:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Improve parser throughput and/or reduce allocation pressure on representative corpora.
2. Preserve parser behavior, grammar acceptance, and diagnostic compatibility.
3. Make parser invariants explicit and test-locked.
4. Strengthen parser/lexer interface contracts to reduce drift.

### 4. Non-Goals

1. No syntax/grammar expansion.
2. No parser algorithm replacement (no generator migration in this proposal).
3. No diagnostic code/title policy changes.
4. No typed-AST migration (covered by proposal 046 track).

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** Implementation note (v0.0.4 M5 safe subset): - Parser benchmark harness landed as `benches/parser_bench.rs`. - Pratt parse-loop lookup now uses a single prec...
- **Detailed specification (migrated legacy content):** Implementation note (v0.0.4 M5 safe subset): - Parser benchmark harness landed as `benches/parser_bench.rs`. - Pratt parse-loop lookup now uses a single precedence table lookup...
- **In Scope:** - `src/syntax/parser/mod.rs` - `src/syntax/parser/expression.rs` - `src/syntax/parser/statement.rs` - `src/syntax/parser/helpers.rs` - `src/syntax/parser/literal.rs`
- **Out of Scope:** - Runtime/compiler semantic changes - Lexer Unicode semantics expansion - Proposal-level language feature additions
- **Week 1: Baseline + Instrumentation:** 1. Add parser benchmark harness with 4 corpora: - declaration/identifier-heavy - operator/expression-heavy - string/interpolation/comment-heavy - malformed/recovery-heavy
- **Week 2: Hot-Path Optimizations:** 1. Optimize statement dispatch ordering in `statement.rs` by common-path frequency. 2. Reduce repeated precedence/helper calls in Pratt loop (`expression.rs`). 3. Reduce transie...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. No syntax/grammar expansion.
2. No parser algorithm replacement (no generator migration in this proposal).
3. No diagnostic code/title policy changes.
4. No typed-AST migration (covered by proposal 046 track).

### 4. Non-Goals

### 11. Risks and Mitigations

1. **Risk:** perf changes alter token-consumption behavior  
   **Mitigation:** contract tests + parser regression fixtures.

2. **Risk:** helper refactor introduces recovery regressions  
   **Mitigation:** malformed corpus + recovery-focused unit tests.

3. **Risk:** benchmark noise misleads decisions  
   **Mitigation:** fixed corpora, repeated runs, median reporting.

4. **Risk:** architecture cleanup grows too broad  
   **Mitigation:** week gates and behavior-preserving scope lock.

### 4. Non-Goals

### 11. Risks and Mitigations

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
