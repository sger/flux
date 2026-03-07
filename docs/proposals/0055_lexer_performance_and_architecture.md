- Feature Name: Lexer Performance and Architecture Hardening
- Start Date: 2026-02-26
- Status: Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0055: Lexer Performance and Architecture Hardening

## Summary
[summary]: #summary

Improve Flux lexer in a phased sequence: measure first, optimize hot paths second, then refactor architecture with parser-contract guardrails.

## Motivation
[motivation]: #motivation

Flux lexer already has modular structure and byte-level fast paths, but there is still opportunity in: Flux lexer already has modular structure and byte-level fast paths, but there is still opportunity in:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 4. Goals

1. Improve lexer throughput and/or allocation profile on representative corpora.
2. Keep token stream semantics stable for parser consumers.
3. Clarify reader/lexer invariants in code and tests.
4. Maintain diagnostics compatibility for lexing-origin failures.

### 5. Non-Goals

1. Change language syntax or token set semantics.
2. Introduce new parser parsing strategies.
3. Redefine interpolation/comment/string language behavior.

### 4. Goals

### 5. Non-Goals

### 5. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** Implementation note (v0.0.4 M5 safe subset): - Lexer benchmark harness remains the baseline source (`benches/lexer_bench.rs`), and M5 validation is gated by...
- **Detailed specification (migrated legacy content):** Implementation note (v0.0.4 M5 safe subset): - Lexer benchmark harness remains the baseline source (`benches/lexer_bench.rs`), and M5 validation is gated by parser/diagnostics p...
- **In scope:** 1. Lexer hot-path performance in: - `src/syntax/lexer/mod.rs` - `src/syntax/lexer/reader.rs` - `src/syntax/lexer/strings.rs` - `src/syntax/lexer/comments.rs` 2. Token payload/ow...
- **Out of scope:** 1. Syntax/grammar changes. 2. Parser algorithm redesign. 3. Diagnostics code/title changes. 4. Unicode semantics expansion.
- **Phase 1 (Week 1): Baseline + Instrumentation:** 1. Add benchmark harness with 4 corpora: - identifiers-heavy - operators/numbers-heavy - strings/interpolation-heavy - comments/doc-comments-heavy 2. Add debug counters: - token...
- **Phase 2 (Week 2): Hot-Path Perf Wins:** 1. Optimize `next_token` dispatch in `mod.rs`: - strict staged branch order - single cursor snapshot where possible - reduce repeated helper calls in hot loop 2. Reduce allocati...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 5. Non-Goals

1. Change language syntax or token set semantics.
2. Introduce new parser parsing strategies.
3. Redefine interpolation/comment/string language behavior.

### 5. Non-Goals

### 12. Risks and Mitigations

1. Risk: perf tweaks alter token stream behavior.  
   Mitigation: token-stream regression snapshots + parser integration gates.
2. Risk: reader cleanup introduces position bugs.  
   Mitigation: dedicated invariant tests for byte offsets/line/column.
3. Risk: optimization complexity grows too large.  
   Mitigation: phase gates; stop at Phase 2 if gains are sufficient.

### 5. Non-Goals

### 12. Risks and Mitigations

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
