- Feature Name: Diagnostic Rendering Improvements
- Start Date: 2026-03-26
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Improve the Flux diagnostic renderer to produce compact, readable error output when labels span distant source lines. Currently, when a diagnostic has labels on lines far apart (e.g., a function definition at line 74 and a call site at line 346), the renderer can dump hundreds of irrelevant source lines. This proposal introduces three targeted fixes: line elision with `...` markers, narrow label spans (signature-only instead of full body), and deduplication of shared related notes.

## Motivation
[motivation]: #motivation

Flux's Elm-style diagnostic system is a key part of the developer experience. However, several structural issues degrade output quality for real-world programs:

### Problem 1: Hundreds of irrelevant source lines

When a diagnostic includes labels on distant lines — for example, a function definition label at line 74 and a type-mismatch label at line 346 — the renderer walks every line in between, producing 270+ lines of unrelated source code. In practice this can produce 575 lines of output for a single error, making it nearly impossible to find the actual problem.

**Example (before fix):**
```
  74 | fn process_data(input) with IO {
  75 |     let result = ...
  ...            ← 270 lines of function body ←
 345 |
 346 |     process_data(42)
     |     ^^^^^^^^^^^^^^^^ expected String, got Int
```

The user has to scroll through the entire function body to reach the error.

### Problem 2: Labels span too much source

When the compiler attaches a "defined here" label to a function, it currently spans the entire function body (from `fn` to closing `}`). This is the root cause of Problem 1 — the label range forces the renderer to walk all lines. The label should span only the function signature (name + parameters), since that's the relevant information.

### Problem 3: Duplicate related notes

When two call sites to the same function both fail type-checking, the compiler emits the "function defined here" related note twice — once for each diagnostic. In a file with N calls to the same function, users see N identical notes. These should be deduplicated or at minimum grouped.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

After this proposal, Flux diagnostics will be more compact and focused:

**Elided output (Phase 1 — already implemented):**
```
  74 | fn process_data(input) with IO {
     | -------------- defined here
 ... |
 346 |     process_data(42)
     |     ^^^^^^^^^^^^^^^^ expected String, got Int
```

Lines between labeled regions are replaced with `...`, showing only 2 lines of context around each label.

**Narrow signature spans (Phase 2):**
```
  74 | fn process_data(input) with IO {
     |    ^^^^^^^^^^^^^^^^^^^^ defined here: (String) -> Unit with IO
```

The label covers only the function name and parameters, not the body.

**Deduplicated related notes (Phase 3):**

When multiple errors reference the same function definition, the "defined here" note appears once with a count:
```
note: function `process_data` defined here (referenced by 3 errors above)
  74 | fn process_data(input) with IO {
     |    ^^^^^^^^^^^^^^^^^^^^
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 1: Line Elision (COMPLETED)

**File:** `src/diagnostics/rendering/source.rs`

A visibility bitmap is computed before rendering. Lines within `CONTEXT=2` lines of any label or the primary span are marked visible. All other lines are skipped with a `...` marker.

```rust
const CONTEXT: usize = 2;
let mut visible = vec![false; (actual_end + 1).saturating_sub(actual_start)];

// Mark lines near primary span
for line_no in start_line..=end_line {
    let base = line_no.saturating_sub(actual_start);
    for offset in base.saturating_sub(CONTEXT)..=(base + CONTEXT).min(visible.len() - 1) {
        visible[offset] = true;
    }
}

// Mark lines near each label
for label in labels {
    for line_no in label.span.start.line..=label.span.end.line {
        // same context window logic
    }
}
```

During rendering, non-visible lines are skipped. When transitioning from visible to non-visible, a single `...` line is emitted. This prevents consecutive `...` markers.

**Impact:** Reduces diagnostic output from 575 lines to ~20 lines for the AoC day06 example with distant labels.

### Phase 2: Narrow Function Definition Labels

**Files:** `src/ast/type_infer/`, `src/diagnostics/compiler_errors.rs`

Currently, when the type checker creates a "defined here" label for a function, it uses the span of the entire `FnDecl` AST node (from `fn` keyword to closing `}`). This should be narrowed:

1. **Add a `name_span` or `signature_span` field to `FnDecl`** in the AST. The parser already knows the exact position of the function name and closing `)` of parameters — store this as a separate span.

2. **Update type inference label creation** to use `fn_decl.signature_span` instead of `fn_decl.span` when attaching "defined here" labels to diagnostics.

3. **Update pattern match exhaustiveness** and other diagnostic sites that reference function definitions.

The signature span covers `process_data(input)` — from the function name through the closing parenthesis. This is sufficient context for the user to understand which function is referenced and what its parameter types are.

**AST change:**
```rust
// In ast/mod.rs (FnDecl struct)
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Box<Expr>,
    pub span: Span,           // Full span (fn keyword to closing })
    pub signature_span: Span, // NEW: name through closing )
    // ...
}
```

**Parser change:**
```rust
// In syntax/parser.rs, when parsing fn declarations
let sig_start = name_token.span.start;
// ... parse params ...
let sig_end = close_paren_token.span.end;
let signature_span = Span::new(sig_start, sig_end);
```

### Phase 3: Deduplicate Related Notes

**File:** `src/diagnostics/aggregator.rs`

The aggregator already groups diagnostics by file and sorts them. Extend this with a deduplication pass for related notes:

1. **Collect related notes** across all diagnostics in a group. Key them by `(file, span_start_line, message)`.

2. **For duplicates**, keep only the first occurrence and annotate it with the count:
   ```
   note: function `process_data` defined here (referenced by 3 errors)
   ```

3. **Attach the deduplicated note** to the first diagnostic in the group that references it. Remove it from subsequent diagnostics.

This only applies to related notes with identical spans and messages. Notes with different messages or spans remain separate.

## Drawbacks
[drawbacks]: #drawbacks

- **Phase 2 AST change** adds a field to every `FnDecl` node. This is a small memory cost but affects every function declaration. Mitigated by the fact that `Span` is only 32 bytes (4 usizes).

- **Phase 3 deduplication** changes the output format, which could affect snapshot tests and tools that parse diagnostic output. All snapshot tests will need updating.

- **Line elision** (Phase 1) could theoretically hide relevant context if a meaningful comment or code exists between two labels. The 2-line context window mitigates this, and users can always look at the source file directly.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why this design?

- **Phased approach**: Each phase is independently valuable and can ship separately. Phase 1 is already implemented and provides the biggest improvement.
- **Elision over truncation**: Showing `...` with line numbers lets users know exactly what was skipped. Truncating to "first N lines" would lose the second label entirely.
- **Signature spans over body spans**: Every major compiler (rustc, GHC, elm) points to function signatures, not bodies, in "defined here" notes.

### Alternatives considered

1. **Collapse to single-line references**: Instead of showing source snippets for "defined here" notes, just show `note: defined at file.flx:74`. This loses visual context but is maximally compact. Could be a follow-up for very long diagnostics.

2. **Configurable context window**: Let users set `CONTEXT` via environment variable (e.g., `FLUX_DIAG_CONTEXT=5`). Adds complexity for marginal benefit — 2 lines is a good default.

3. **Smart label shortening**: Automatically shorten labels that would cause >N lines of output. This is harder to implement correctly and could produce confusing results when the shortened span doesn't align with token boundaries.

### Impact of not doing this

Users working on files with 100+ lines will continue to see bloated diagnostic output that requires scrolling. This is especially painful in CI logs and terminal environments without scrollback.

## Prior art
[prior-art]: #prior-art

- **Rust (rustc)**: Uses `...` elision between distant labels, narrow spans for "defined here" notes pointing to function signatures, and deduplication of identical notes. Flux's diagnostic system is modeled after rustc's approach.

- **Elm**: Shows focused, single-error diagnostics with minimal context. Elm avoids the problem by never showing distant labels — each error is self-contained with inline suggestions. Flux's multi-label approach is more informative but requires elision.

- **GHC (Haskell)**: Points to function type signatures (not bodies) in type error diagnostics. Uses `...` for multi-line spans. Recent versions (9.6+) added improved error grouping.

- **TypeScript (tsc)**: Shows only the error line with no surrounding context by default. The `--pretty` flag adds 2 lines of context. This is too minimal for Flux's rich diagnostic style.

- **codespan-reporting (Rust crate)**: The Rust diagnostic rendering library that inspired Flux's system. It already implements label-aware line elision and is the closest prior art for our approach.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Should `CONTEXT` be configurable?** Currently hardcoded to 2. A value of 1 would be more compact but might clip important context on adjacent lines. The current value works well in practice.

- **Should Phase 2 use `name_span` or `signature_span`?** `name_span` covers just the function name; `signature_span` covers name through parameters. The latter is more informative for type errors but wider. Leaning toward `signature_span`.

- **How should Phase 3 handle related notes with different messages but the same span?** For example, one note says "defined here" and another says "first defined here" for the same function. These should probably not be deduplicated.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Diagnostic severity-based verbosity**: Show more context for errors, less for warnings. Errors get 3 lines of context, warnings get 1.

- **IDE integration**: LSP diagnostics already use spans. Narrow signature spans (Phase 2) would improve hover diagnostics in VS Code and other editors.

- **Interactive diagnostics**: A `--explain E001` flag that shows the full diagnostic with all context (no elision) for a specific error code. Useful for debugging complex type errors.

- **Diagnostic diffing**: When re-running after a fix, show only new or changed diagnostics. Requires a diagnostic fingerprinting system.

- **Related note folding**: In terminal output, use ANSI escape codes to make related notes collapsible. Click or press a key to expand.
