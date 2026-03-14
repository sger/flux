- Feature Name: Bytecode Decode & Pass-Oriented Rewriting API
- Start Date: 2026-02-12
- Status: Not Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0023: Bytecode Decode & Pass-Oriented Rewriting API

## Summary
[summary]: #summary

Provide a structured bytecode inspection and rewriting API that operates on Flux's raw `Vec<u8>` instruction streams. Instead of a classic Visitor over raw bytes, this proposal introduces a zero-allocation decoder yielding `InstrView` values per instruction, an iterator for sequential decoding, and pass helpers (`for_each_instr`, `map_instrs`) that support instruction rewriting with automatic jump target fixup.

## Motivation
[motivation]: #motivation

Flux's compiled bytecode is stored as `Vec<u8>` (aliased as `Instructions`). The existing `disassemble()` function in `op_code.rs` decodes instructions for display but returns a `String` — there is no structured API for programmatic inspection or rewriting. Adding optimization passes, bytecode analysis, or instruction rewriting currently requires hand-rolling byte-level decode logic each time.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

This proposal should be read as a user-facing and contributor-facing guide for the feature.

- The feature goals, usage model, and expected behavior are preserved from the legacy text.
- Examples and migration expectations follow existing Flux conventions.
- Diagnostics and policy boundaries remain aligned with current proposal contracts.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Existing Infrastructure:** All in [op_code.rs](../../src/bytecode/op_code.rs): - **Jump Opcodes:** Three jump instructions, all using **u16 absolute** addresses (3 bytes tot...
- **Existing Infrastructure:** All in [op_code.rs](../../src/bytecode/op_code.rs): - `OpCode` enum — `#[repr(u8)]`, 50 variants (`OpConstant = 0` through `OpHashLong = 49`) - `type Instructions = Vec<u8>` — r...
- **Jump Opcodes:** Three jump instructions, all using **u16 absolute** addresses (3 bytes total each): | Opcode | Semantics | |--------|-----------| | `OpJump` | Unconditional jump | | `OpJumpNotT...
- **What This Enables:** A structured decode/rewrite API is foundational infrastructure for bytecode-level compiler work. Without it, every new pass must hand-roll byte-level decoding logic, duplicating...
- **Concrete Use Cases:** | Use Case | How it uses the API | |----------|-------------------| | **Peephole optimization** | `map_instrs` to pattern-match instruction sequences and replace with optimized...
- **Why Not Just Use `disassemble()`?:** `disassemble()` returns a `String` — useful for display, but unusable for programmatic analysis or rewriting. Every analysis pass that needs structured data would have to re-par...

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

### Future Extensions

- **Peephole optimizer pass** — use `map_instrs` to implement constant folding, dead store elimination
- **Bytecode verifier** — use `for_each_instr` to validate stack depth, jump target alignment
- **NOP sled removal** — use `map_instrs` to strip no-ops with automatic jump fixup
- **Relative jumps** — if encoding switches from absolute to relative offsets, only `map_instrs` Pass 2 needs updating

### Future Extensions
