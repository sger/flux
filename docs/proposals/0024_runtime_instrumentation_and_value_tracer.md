- Feature Name: Runtime Instrumentation Hooks & Value Graph Tracer
- Start Date: 2026-02-12
- Status: Partially Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0024: Runtime Instrumentation Hooks & Value Graph Tracer

## Summary
[summary]: #summary

Two complementary runtime infrastructure pieces: Two complementary runtime infrastructure pieces:

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

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

- **Consolidated technical points:** - **What This Enables:** | Use Case | Part | How it uses the API | |----------|------|-------------------| | **Instruction profiling** | A | `on_instruction` callback counts opc...
- **What This Enables:** | Use Case | Part | How it uses the API | |----------|------|-------------------| | **Instruction profiling** | A | `on_instruction` callback counts opcode frequencies, measures...
- **Why a Trait Instead of the Existing `trace: bool` Flag?:** The VM currently has a `trace: bool` field and a `trace_instruction()` method that prints to stdout ([vm/mod.rs:30](src/runtime/vm/mod.rs#L30), [vm/trace.rs:171](src/runtime/vm/...
- **VM Structure ([vm/mod.rs](src/runtime/vm/mod.rs)):** ```rust pub struct VM { constants: Vec<Value>, stack: Vec<Value>, // growable, max 1,048,576 slots sp: usize, last_popped: Value, pub globals: Vec<Value>, // 65,536 slots frames...
- **Execution Loop:** ``` execute_current_instruction() → read ip and op from current frame → if trace: trace_instruction(ip, op) ← HOOK POINT: on_instruction → dispatch_instruction(ip, op) → OpCall...
- **Value Memory Model ([value.rs](src/runtime/value.rs)):** 14 variants. Primitives unboxed; containers use `Rc<T>`: | Variant | Inner type | Contains Values? | |---------|-----------|-----------------| | `Integer(i64)` | primitive | no...

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

- **`on_alloc` hook** — call observer when Rc-wrapped values are created (at `leak_detector::record_*` sites)
- **`on_gc` hook** — if Proposal 0017 (GC) is implemented, observer receives collection events
- **Structured `VmError` type** — replace `String` errors with an enum for richer `on_error` callbacks
- **Profiling observer** — timing per opcode category, call graph with durations, hot-path analysis
- **Heap size estimation** — extend `Tracer` to sum approximate byte sizes of visited values

### Future Extensions
