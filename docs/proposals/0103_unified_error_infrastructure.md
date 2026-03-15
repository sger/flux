- Feature Name: Unified Error Infrastructure
- Start Date: 2026-03-15
- Status: Draft
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0080 (quality diagnostics), 0081 (diagnostic taxonomy)

# Proposal 0103: Unified Error Infrastructure

## Summary

Formalize the error handling pipeline so that parser, compiler, and runtime
errors all flow through the same infrastructure — regardless of whether
execution happens on the bytecode VM or Cranelift JIT. Every error produces
an identical structured diagnostic with error code, source snippet, span
highlighting, and contextual hints.

## Motivation

Today, error rendering diverges between the VM and JIT:

| Backend | Error source | Rendering |
|---------|-------------|-----------|
| VM | `binary_ops.rs`, `dispatch.rs` | Full diagnostic via `DiagnosticsAggregator` + span + stack trace |
| JIT | `runtime_helpers.rs` | Raw string (`"cannot add Int and None"`) |

This means the same program can produce different error output depending on
the backend. Users shouldn't need to know which backend is running to
understand an error.

### Current state (post-0102 fixes)

We added `rt_render_error_with_span` which renders JIT arithmetic errors with
the correct span. But it's a point fix — other JIT runtime errors still
produce raw strings. We need a systematic approach.

## Design

### Principle: errors are data, rendering is separate

```
Error producer (VM op / JIT helper / parser / compiler)
    → ErrorDescription { code, title, message, hint }

Error renderer (call site with span info)
    → Diagnostic { code, title, message, span, file, source }
    → rendered string with source snippet + highlighting
```

The error producer never renders. It returns structured data. The call site
adds the span and renders via the shared `Diagnostic` pipeline.

### Three-layer architecture

```
Layer 1: ErrorCode constants (src/diagnostics/compiler_errors.rs, runtime_errors.rs)
  Static definitions: code, title, message template, hint template.
  Shared by VM, JIT, parser, and compiler.

Layer 2: Diagnostic builder (src/diagnostics/diagnostic.rs)
  Attaches span, file, source, phase, category to an ErrorCode.
  Produces a Diagnostic value.

Layer 3: Diagnostic renderer (src/diagnostics/rendering/)
  Renders Diagnostic → formatted string with source snippets.
  Used by DiagnosticsAggregator for multi-error reports.
```

### Error flow by phase

```
Parser error:
  parser.rs → Diagnostic::error(code, title) + span → DiagnosticsAggregator → output

Compiler error:
  compiler/expression.rs → diag_enhanced(ERROR_CODE) + span → DiagnosticsAggregator → output

VM runtime error:
  binary_ops.rs → runtime_error_enhanced(&ERROR_CODE, args) → Diagnostic + span from current_location() → output

JIT runtime error:
  rt_add() → ctx.error = "Invalid Operation\nCannot add Int and None values."
  Cranelift IR → rt_render_error_with_span(ctx, span) → Diagnostic + span from compile-time → output
```

### What changes

#### Phase 1: Normalize JIT runtime helper error messages (done)

All JIT runtime helpers produce messages in the format:
```
Title\nMessage details.
```

`rt_render_error_with_span` parses this into title + details, classifies the
error code via `classify_runtime_error_code`, and renders a full diagnostic.

#### Phase 2: Replace `classify_runtime_error_code` with explicit codes

Instead of parsing error messages to guess the error code, runtime helpers
should set the error code directly:

```rust
// Current (fragile):
ctx.error = Some("Invalid Operation\nCannot add Int and None values.".to_string());
// classify_runtime_error_code parses "Invalid Operation" → "E1009"

// Better:
ctx.set_error("E1009", "Invalid Operation", "Cannot add Int and None values.");
```

Add a `set_error` method to `JitContext`:

```rust
pub fn set_error(&mut self, code: &str, title: &str, message: &str) {
    self.error_code = Some(code.to_string());
    self.error = Some(format!("{}\n{}", title, message));
}
```

#### Phase 3: Emit `rt_render_error_with_span` at all JIT error sites

Currently only arithmetic ops emit span rendering. Extend to:
- Division by zero (inline guard in Cranelift IR)
- Prefix operators (negate, not)
- Base function calls (type mismatches)
- Indirect calls (wrong arity, not callable)
- Pattern match failures

This requires passing spans through the call chain. Most call sites already
have the span available from the AST/IR expression.

#### Phase 4: JSON error output parity

Both VM and JIT should produce identical JSON diagnostics when `--format json`
is used:

```json
{
  "code": "E1009",
  "phase": "runtime",
  "category": "runtime_execution",
  "title": "Invalid Operation",
  "message": "Cannot add Int and None values.",
  "file": "examples/Debug/runtime_trace_error.flx",
  "line": 3,
  "column": 3,
  "end_line": 3,
  "end_column": 11
}
```

### Error code ranges

| Range | Phase | Examples |
|-------|-------|---------|
| E001–E099 | Compiler (variables, bindings) | E003 Outer Assignment, E006 Duplicate Parameter |
| E100–E199 | Compiler (modules) | E100 Invalid Module Name |
| E200–E299 | Compiler (types) | E200 Undefined Type, E220 ADT Constructor |
| E300–E399 | Compiler (type inference) | E300 Type Mismatch, E301 Recursive Type |
| E400–E499 | Compiler (effects) | E400 Missing Effect, E413 Top-Level Effect |
| E500–E599 | Compiler (patterns) | Reserved |
| E600–E699 | Reserved | |
| E900–E999 | Internal (ICE) | E900 Internal Compiler Error |
| E1000–E1099 | Runtime (execution) | E1000 Wrong Arity, E1009 Invalid Operation |

### Stack trace gap (JIT)

The JIT doesn't track call frames, so it can't produce stack traces. This is
a known limitation. Options for the future:
1. Shadow stack: maintain a parallel stack of function names in `JitContext`
2. Debug info: use Cranelift debug info to reconstruct frames on error
3. Accept the gap: stack traces are a debugging aid, not a correctness issue

## Implementation plan

- [x] Phase 1: Normalize JIT arithmetic error messages (done in this session)
- [x] Phase 1: Add `rt_render_error_with_span` helper (done)
- [x] Phase 1: Emit span rendering after arithmetic ops (done)
- [ ] Phase 2: Add `set_error` method with explicit error codes
- [ ] Phase 3: Extend span rendering to all JIT error sites
- [ ] Phase 4: JSON error output parity between VM and JIT
- [ ] Future: JIT shadow stack for stack traces

## Success criteria

Running `scripts/check_parity.sh` on any example that produces a runtime
error should show identical diagnostic output between VM and JIT (excluding
stack traces).

## Prior art

- **Rust**: compiler and runtime errors share the same diagnostic format via
  `rustc_errors::Diagnostic`
- **Elm**: all errors (parser, type, runtime) share a consistent format with
  source highlighting — Flux's diagnostics are inspired by Elm
- **OCaml**: `Location.error` provides a unified error type across compiler phases
- **Koka**: uses the same diagnostic format for type errors and runtime errors
