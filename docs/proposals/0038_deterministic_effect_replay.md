- Feature Name: Deterministic Effect Replay
- Start Date: 2026-02-20
- Status: Not Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0038: Deterministic Effect Replay

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for Deterministic Effect Replay in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

Flux now has explicit effectful operation boundaries (`PrimOp` effects + base call boundaries), but reproducing failures is still difficult when programs depend on: Flux now has explicit effectful operation boundaries (`PrimOp` effects + base call boundaries), but reproducing failures is still difficult when programs depend on:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 4. Runtime Design

Introduce runtime mode:

- `ReplayMode::Off`
- `ReplayMode::Record { sink }`
- `ReplayMode::Replay { source, cursor }`

Add an `EffectRuntime` trait used by effectful primops/base functions:

- `on_read_file(path) -> Result<String, Error>`
- `on_read_lines(path) -> Result<Vec<String>, Error>`
- `on_read_stdin() -> Result<String, Error>`
- `on_now_ms() -> Result<i64, Error>`
- `on_print(rendered) -> Result<(), Error>`
- `on_panic(message) -> Result<(), Error>`

Behavior:

- `Off`: call current runtime behavior directly.
- `Record`: call real behavior + persist event/result.
- `Replay`: validate next event kind/payload; return recorded result and never touch OS.

### 4. Runtime Design

Introduce runtime mode:

Behavior:

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **2. User-Facing Behavior:** - `flux run --record <trace_file> <program.flx>` - `flux run --replay <trace_file> <program.flx>` - **3. Effect Event Model:** Define a compact ev...
- **2. User-Facing Behavior:** Add two CLI modes: - `flux run --record <trace_file> <program.flx>` - `flux run --replay <trace_file> <program.flx>`
- **3. Effect Event Model:** Define a compact event log with strict order: 1. `ReadFile { path, result }` 2. `ReadLines { path, result }` 3. `ReadStdin { result }` 4. `NowMs { result }` 5. `Print { rendered...
- **5. VM/JIT Parity Strategy:** Policy parity requirement: - both backends must route effectful operations through the same `EffectRuntime` interface.
- **6. Compiler and Bytecode Impact:** Optional debug metadata improvement: - include instruction offset + source span in replay mismatch diagnostics.
- **7. Failure Modes and Diagnostics:** Replay must fail fast for: - event kind mismatch - payload mismatch (e.g., different file path argument) - trace exhausted before program finishes - leftover trace events after...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

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
