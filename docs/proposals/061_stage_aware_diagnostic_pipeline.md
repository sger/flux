# Proposal 061: Stage-Aware Diagnostic Pipeline

**Status:** Draft
**Date:** 2026-02-28
**Depends on:** `057_parser_diagnostics_with_inferred_types.md`, `060_parser_diagnostics_hm_typechecker_hardening.md`

---

## 1. Summary

Introduce a stage-aware diagnostic pipeline that filters, deduplicates, and presents errors according to the compilation phase that produced them. Instead of dumping all diagnostics from all phases at once, the pipeline follows a strict **Parse -> Type -> Effect** cascade: parser errors suppress type errors, type errors suppress effect errors. This produces fewer, higher-quality diagnostics that point to root causes rather than downstream consequences.

**Core principle:** Never mix parse errors with type errors in the same report. Prefer one high-quality diagnostic over many low-quality ones.

---

## 2. Motivation

### 2.1 Current Pipeline (Error-Accumulating Model)

Today, Flux accumulates all errors from all phases into a single `all_diagnostics` pool:

```
Parser -> Module Graph -> PASS 0 -> PASS 1 -> HM Inference -> PASS 2
    ↘           ↘           ↘          ↘           ↘            ↘
     ─────────── all_diagnostics ──────────────────────────────────→ Aggregator → Output
```

All phases run unconditionally. The `DiagnosticsAggregator` deduplicates structurally and sorts by source order, but has no concept of *which phase* produced each diagnostic. The result: a parse error on line 2 (unclosed string) can appear alongside an HM type error on line 5 and an effect error on line 14 — even though the type and effect errors are likely cascaded from the parse failure.

### 2.2 The Problem

When `let label: String = "The answer` (missing closing quote) appears on line 2, the parser recovers and continues, but the recovery may produce a malformed AST. If `fn add(a: Int, b: In)` on line 5 was parsed from a corrupted token stream, the type error `Cannot unify In with Int` is a **downstream cascade**, not a root cause. Showing both errors forces the user to mentally triage which errors are "real" and which are consequences of earlier failures.

### 2.3 The Elm/Rust Standard

Both Elm and Rust's compilers implement stage-aware filtering:
- **Elm**: Parse errors suppress all type checking. Type errors are shown only when parsing succeeds.
- **Rust**: Parse errors abort before type checking. `rustc` uses "error guarantees" (the `ErrorGuaranteed` type) to track that an error has been reported and skip downstream phases.

### 2.4 Architectural Gap

The `Diagnostic` struct has no `phase` field. The `DiagnosticsAggregator` has no stage-aware filtering. Error codes implicitly encode phase membership (E001-E086 = parser/compiler, E300-E302 = HM, E407 = effects, E1000+ = runtime) but this is never used for filtering decisions.

---

## 3. Goals

1. Tag every diagnostic with the compilation phase that produced it.
2. Implement strict stage-aware filtering in the aggregator: Parse errors suppress Type/Effect errors; Type errors suppress Effect errors.
3. Collapse cascading parser errors to the earliest root cause with a recovery note.
4. When downstream errors are suppressed, emit a summary note: `"Note: N type/effect errors were suppressed because parsing failed. Fix the parse errors first."`.
5. Preserve the error-accumulating model internally (all phases still run for IDE use cases), but filter at the output layer.
6. Add a `--all-errors` CLI flag to disable stage filtering for debugging.

---

## 4. Non-Goals

1. No changes to the compilation phases themselves (all phases still run to completion).
2. No changes to the HM inference algorithm.
3. No changes to error code numbering or diagnostic message text.
4. No changes to runtime error handling.
5. No "error guarantee" type system (Rust's `ErrorGuaranteed` approach) — this would require major refactoring.

---

## 5. Design

### 5.1 DiagnosticPhase Enum

```rust
// src/diagnostics/types/diagnostic_phase.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticPhase {
    /// Lexer and parser errors (E001–E086, E071, E076, etc.)
    Parse,
    /// Module graph resolution (import cycles, missing modules)
    ModuleGraph,
    /// Compiler validation passes (PASS 0: main validation, ADTs, contracts, strict mode)
    Validation,
    /// HM type inference (E300–E302)
    TypeInference,
    /// Compiler boundary checks (E055, E425) and bytecode generation (PASS 2)
    TypeCheck,
    /// Effect system errors (E407, effect propagation)
    Effect,
    /// Runtime errors (E1000+) — not filtered, always shown
    Runtime,
}
```

### 5.2 Diagnostic Struct Extension

```rust
// src/diagnostics/diagnostic.rs
pub struct Diagnostic {
    // ... existing fields ...
    pub(crate) phase: Option<DiagnosticPhase>,
}

impl Diagnostic {
    pub fn phase(&self) -> Option<DiagnosticPhase> {
        self.phase
    }

    pub fn with_phase(mut self, phase: DiagnosticPhase) -> Self {
        self.phase = Some(phase);
        self
    }
}
```

The `phase` field is `Option<DiagnosticPhase>` for backward compatibility. Diagnostics without a phase tag are treated as belonging to all phases (never filtered).

### 5.3 Phase Tagging at Emission Points

Each diagnostic emission site tags the phase. Most tagging happens at collection boundaries, not at individual error constructors:

| Location | Phase tag |
|----------|-----------|
| `src/main.rs`: `parser.errors` | `DiagnosticPhase::Parse` |
| `src/main.rs`: `graph_result.diagnostics` | `DiagnosticPhase::ModuleGraph` |
| `compiler/mod.rs`: PASS 0 validation calls | `DiagnosticPhase::Validation` |
| `compiler/mod.rs`: HM `hm.diagnostics` | `DiagnosticPhase::TypeInference` |
| `compiler/mod.rs`: PASS 2 `self.errors` | `DiagnosticPhase::TypeCheck` |
| `compiler/statement.rs`: effect validation | `DiagnosticPhase::Effect` |
| `runtime/vm/dispatch.rs`: runtime errors | `DiagnosticPhase::Runtime` |

**Bulk tagging helper:**
```rust
fn tag_diagnostics(diags: &mut [Diagnostic], phase: DiagnosticPhase) {
    for d in diags.iter_mut() {
        if d.phase.is_none() {
            d.phase = Some(phase);
        }
    }
}
```

### 5.4 Stage-Aware Filtering Rules

The filtering logic lives in `DiagnosticsAggregator`:

```rust
impl<'a> DiagnosticsAggregator<'a> {
    fn apply_stage_filtering(&self, diagnostics: Vec<&'a Diagnostic>) -> Vec<&'a Diagnostic> {
        if self.disable_stage_filtering {
            return diagnostics;
        }

        let has_parse_errors = diagnostics.iter().any(|d|
            d.severity() == Severity::Error &&
            matches!(d.phase(), Some(DiagnosticPhase::Parse))
        );

        let has_type_errors = diagnostics.iter().any(|d|
            d.severity() == Severity::Error &&
            matches!(d.phase(),
                Some(DiagnosticPhase::TypeInference) | Some(DiagnosticPhase::TypeCheck))
        );

        let keep_phase = |d: &Diagnostic| -> bool {
            match d.phase() {
                None => true,  // Untagged diagnostics always shown
                Some(DiagnosticPhase::Runtime) => true,  // Runtime always shown
                Some(DiagnosticPhase::Parse) => true,  // Parse always shown
                Some(DiagnosticPhase::ModuleGraph) => true,  // Module graph always shown
                Some(DiagnosticPhase::Validation) => true,  // Validation always shown
                Some(DiagnosticPhase::TypeInference | DiagnosticPhase::TypeCheck) => {
                    !has_parse_errors  // Suppress if parse errors exist
                }
                Some(DiagnosticPhase::Effect) => {
                    !has_parse_errors && !has_type_errors  // Suppress if parse or type errors exist
                }
            }
        };

        diagnostics.into_iter().filter(|d| keep_phase(d)).collect()
    }
}
```

### 5.5 Suppression Summary Note

When stage filtering removes diagnostics, append a note:

```rust
fn suppression_note(suppressed_count: usize, suppressed_phase: &str) -> Diagnostic {
    Diagnostic::make_note(
        "DOWNSTREAM ERRORS SUPPRESSED",
        format!(
            "{} {} error{} suppressed because parsing failed. Fix the parse errors first.",
            suppressed_count,
            suppressed_phase,
            if suppressed_count == 1 { " was" } else { "s were" },
        ),
        Span::default(),
    )
}
```

### 5.6 Parser Error Cascade Collapsing

Within the `Parse` phase, multiple errors often cascade from a single root cause (e.g., an unclosed delimiter triggers a chain of "unexpected token" errors). The aggregator collapses cascading parser errors:

**Rule:** If two parser errors share the same file and the second error's span starts within 3 lines of the first error's span, and the second error has a generic code (E034 UNEXPECTED_TOKEN), mark the second as a cascade.

```rust
fn collapse_parser_cascades(parse_errors: &mut Vec<&Diagnostic>) {
    if parse_errors.len() <= 1 {
        return;
    }

    // Sort by line
    parse_errors.sort_by_key(|d| d.span().map_or(0, |s| s.start.line));

    let mut root_line = parse_errors[0].span().map_or(0, |s| s.start.line);
    let mut keep = vec![true; parse_errors.len()];

    for i in 1..parse_errors.len() {
        let line = parse_errors[i].span().map_or(0, |s| s.start.line);
        let is_generic = parse_errors[i].code() == Some("E034");

        if is_generic && line <= root_line + 3 {
            keep[i] = false;  // Cascade, suppress
        } else {
            root_line = line;  // New root error
        }
    }

    // Add recovery note to root if cascades were suppressed
    let suppressed = keep.iter().filter(|&&k| !k).count();
    if suppressed > 0 {
        // Note appended to the last shown parse error
    }

    let mut idx = 0;
    parse_errors.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });
}
```

### 5.7 CLI Flag

```
--all-errors    Show all diagnostics from all phases, disabling stage-aware filtering
```

Parsed in `src/main.rs` CLI argument handling. Passed to `DiagnosticsAggregator::with_stage_filtering(bool)`.

---

## 6. Integration into Aggregator Pipeline

Updated processing pipeline:

```
all_diagnostics
  ↓
[1] Tag phase (if not already tagged)
  ↓
[2] Structural deduplication (existing — DiagnosticKey hash)
  ↓
[3] E300 neighborhood suppression (existing)
  ↓
[4] Stage-aware filtering (NEW — §5.4)
  ↓
[5] Parser cascade collapsing (NEW — §5.6)
  ↓
[6] Sorting (existing — file → line → column → severity)
  ↓
[7] Error limiting (existing — max_errors cap)
  ↓
[8] Suppression summary note (NEW — §5.5)
  ↓
[9] Rendering (existing)
```

Steps 4, 5, and 8 are new. All existing steps are unchanged.

---

## 7. Diagnostic Output Examples

### 7.1 Parse Errors Only (Type/Effect Suppressed)

**Input:** The multi-error test file from proposal 057 §7b with 6 errors across 3 phases.

**Current output:** 4 parser errors shown (the other 2 — type typo and unknown effect — are lost because the parser consumed the file incorrectly).

**After proposal 061:**

```
--> compiler error[E071]: UNTERMINATED STRING

String literal is missing closing quote.

  --> test.flx:2:21
  |
2 | let label: String = "The answer
  |                     ^^^^^^^^^^^

Hint: Add a closing " at the end of the string.


--> compiler error[E034]: UNEXPECTED TOKEN

Empty interpolation `#{}` — provide an expression between the braces.

  --> test.flx:11:14
   |
11 |     "Point(#{}, #{y})"
   |              ^


--> compiler error[E076]: UNCLOSED DELIMITER

Expected a closing `}` to match this opening `{` in function `main`.

  --> test.flx:14:18
   |
14 | fn main() with I {
   |                  ^

Help: Add `}` to close the body of function `main`.

Found 3 errors.

Note: 2 type/effect errors were suppressed because parsing failed.
      Fix the parse errors first, then re-compile to see remaining issues.
```

### 7.2 Type Errors Only (Parsing Succeeded, Effects Suppressed)

**Input:** A file with no parse errors but both type and effect errors.

```flux
fn greet(name: String) -> String {
    "Hello, #{name}!"
}

fn main() with IO {
    print(greet(42))
    let x: Bool = greet("world")
}
```

**Output:**

```
--> compiler error[E300]: TYPE UNIFICATION ERROR

Cannot unify String with Int.

  --> test.flx:6:17
  |
6 |     print(greet(42))
  |                 ^^
  |                 -- expected String, found Int
  |                 -- argument type is known at compile time

Help: argument #1 does not match function contract


--> compiler error[E300]: TYPE UNIFICATION ERROR

Cannot unify Bool with String.

  --> test.flx:7:19
  |
7 |     let x: Bool = greet("world")
  |                   ^^^^^^^^^^^^^^
  |                   -------------- expected Bool, found String
  |                   -------------- initializer type is known at compile time

Help: binding initializer does not match type annotation

Found 2 errors.
```

### 7.3 Effect Errors Only (Parsing + Typing Succeeded)

**Input:** A file with correct syntax and types but missing effects.

```flux
fn helper() -> Int {
    print("hello")
    42
}
```

**Output:**

```
--> compiler error[E015]: MISSING EFFECT ANNOTATION

Function `helper` calls `print` which requires the `IO` effect,
but `helper` does not declare `with IO`.

  --> test.flx:2:5
  |
2 |     print("hello")
  |     ^^^^^^^^^^^^^^ requires IO effect

Help: Add `with IO` to the function signature: `fn helper() -> Int with IO`

Found 1 error.
```

### 7.4 --all-errors Mode (No Filtering)

With `--all-errors`, all diagnostics from all phases are shown, similar to current behavior. This is useful for:
- IDE integrations that want comprehensive error lists
- Debugging the compiler's diagnostic pipeline
- Cases where the user knows a "downstream" error is real

---

## 8. Files Modified

| File | Change |
|------|--------|
| `src/diagnostics/types/diagnostic_phase.rs` | New file: `DiagnosticPhase` enum |
| `src/diagnostics/types/mod.rs` | Re-export `DiagnosticPhase` |
| `src/diagnostics/diagnostic.rs` | Add `phase: Option<DiagnosticPhase>` field + accessor + builder |
| `src/diagnostics/aggregator.rs` | Add `apply_stage_filtering`, `collapse_parser_cascades`, `suppression_note`; integrate into pipeline |
| `src/main.rs` | Tag parser/module graph diagnostics with phase; parse `--all-errors` flag; pass to aggregator |
| `src/bytecode/compiler/mod.rs` | Tag PASS 0/1/2 and HM diagnostics with phase at collection boundaries |
| `src/bytecode/compiler/statement.rs` | Tag effect diagnostics with `DiagnosticPhase::Effect` |

---

## 9. Implementation Plan

### T0 — Add DiagnosticPhase enum and Diagnostic.phase field
- **Goal:** Introduce the phase concept with zero behavior change.
- **Files:** `src/diagnostics/types/diagnostic_phase.rs`, `src/diagnostics/diagnostic.rs`, `src/diagnostics/types/mod.rs`.
- **Changes:** New enum, new field defaulting to `None`, accessor, builder method.
- **Tests:** Existing tests pass unchanged (no filtering yet).
- **Risk:** Low.

### T1 — Tag all diagnostic emission points
- **Goal:** Every diagnostic gets a phase tag.
- **Files:** `src/main.rs`, `src/bytecode/compiler/mod.rs`, `src/bytecode/compiler/statement.rs`.
- **Changes:** Bulk-tag parser errors as `Parse`, HM errors as `TypeInference`, compiler errors as `TypeCheck`, effect errors as `Effect`, etc.
- **Tests:** Unit test verifying all diagnostics from a multi-error file have non-None phases.
- **Risk:** Low.

### T2 — Stage-aware filtering in DiagnosticsAggregator
- **Goal:** Parse errors suppress type/effect errors. Type errors suppress effect errors.
- **Files:** `src/diagnostics/aggregator.rs`.
- **Changes:** Add `apply_stage_filtering` step to pipeline. Add `disable_stage_filtering: bool` option.
- **Tests:** Multi-error fixture: parse+type errors → only parse errors shown. Type+effect errors → only type errors shown.
- **Risk:** Medium — must verify no real errors are lost.

### T3 — Parser cascade collapsing
- **Goal:** Multiple parser errors from a single root cause collapse to one error + recovery note.
- **Files:** `src/diagnostics/aggregator.rs`.
- **Changes:** Add `collapse_parser_cascades`.
- **Tests:** Fixture with unclosed delimiter producing 3 cascading E034s → collapsed to 1 + note.
- **Risk:** Medium — heuristic may be too aggressive or too conservative.

### T4 — Suppression summary note + --all-errors flag
- **Goal:** User knows errors were suppressed. Debug escape hatch exists.
- **Files:** `src/diagnostics/aggregator.rs`, `src/main.rs`.
- **Changes:** Append suppression note when filtering removes errors. Parse `--all-errors` flag.
- **Tests:** Fixture verifying note appears when errors are suppressed.
- **Risk:** Low.

### T5 — Fixtures and snapshot update
- **Goal:** Snapshot tests reflect the new stage-aware output.
- **Files:** `examples/type_system/failing/`, `tests/snapshots/`.
- **Changes:** Update affected snapshots with intentional rationale.
- **Tests:** Full test gate.
- **Risk:** Low.

---

## 10. Task Dependencies

```
T0 → T1 → T2 → T4 → T5
              → T3 → T5
```

T2 and T3 are independent after T1. T4 depends on T2. T5 depends on T2+T3+T4.

---

## 11. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Stage filtering hides a real type error that isn't a cascade | `--all-errors` flag as escape hatch; phase tagging is conservative (untagged diagnostics are never filtered) |
| Parser cascade collapsing is too aggressive, hides independent parse errors | Heuristic only collapses generic E034 within 3 lines of a root error; specific error codes (E071, E076) are never collapsed |
| Effect error is independent of a type error but gets suppressed | Effect errors from PASS 0 validation (e.g., `fn main() with IO` missing) are tagged as `Validation`, not `Effect`, so they survive type-error filtering |
| Some effect-related diagnostics still appear while suppression note mentions effect suppression | This is intentional: structural checks such as `E400` are tagged `TypeCheck` and remain visible; suppression note reports only diagnostics actually removed by stage filtering |
| Performance overhead of phase tagging | Phase is a single byte enum stored in an `Option`; negligible overhead |
| IDE integrations need all errors | `--all-errors` or `disable_stage_filtering: true` when called from tooling |

---

## 12. Diagnostics Compatibility

- **No diagnostic codes change.** Same E-codes, same titles, same messages.
- **Output changes:** Fewer diagnostics shown per compile when errors span multiple phases. This is intentional and strictly better for users.
- **`--all-errors` preserves exact current behavior** for tools that depend on comprehensive error lists.
- **Snapshot tests:** Some snapshots will change (fewer errors in multi-phase failure tests). Each change is intentional and documented.

---

## 13. Fixtures

### New failing fixtures
- `examples/type_system/failing/150_stage_parse_suppresses_type.flx` — parse error + type error, only parse shown
- `examples/type_system/failing/151_stage_type_suppresses_effect.flx` — type error + effect error, only type shown
- `examples/type_system/failing/152_stage_cascade_collapse.flx` — 3 cascading parse errors collapse to 1
- `examples/type_system/failing/153_stage_all_errors_flag.flx` — same as 150 but with `--all-errors` shows both

### Notes
- `150`/`153` are best validated via `flux run` behavior rather than `--test`; test mode intentionally exits immediately on parse errors.

---

## 14. Validation Commands

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test
cargo test --test examples_fixtures_snapshots
cargo clippy --all-targets --all-features -- -D warnings
cargo run -- --no-cache examples/type_system/failing/150_stage_parse_suppresses_type.flx
cargo run -- --no-cache --all-errors examples/type_system/failing/150_stage_parse_suppresses_type.flx
```

---

## 15. Acceptance Criteria

1. Parse errors suppress type/effect errors in default output.
2. Type errors suppress effect errors in default output.
3. A suppression note is shown when errors are filtered.
4. `--all-errors` disables all stage filtering and shows current behavior.
5. Parser cascades within 3 lines of a root error collapse to the root.
6. All existing tests pass.
7. Snapshot changes are intentional and documented.
8. No diagnostic codes, titles, or message text changes.
