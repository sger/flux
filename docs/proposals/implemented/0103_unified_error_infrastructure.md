- Feature Name: Unified Error Infrastructure
- Start Date: 2026-03-15
- Status: Implemented
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0080 (quality diagnostics), 0081 (diagnostic taxonomy)

# Proposal 0103: Unified Error Infrastructure

## Summary

Unify parser, compiler, VM, and JIT error reporting around structured
`Diagnostic` values. The JIT now carries typed compile/runtime errors through
its public API and renders text/json output directly from diagnostics rather
than flattening to strings and reparsing them later.

## What landed

- JIT public APIs now return typed errors instead of `String`.
- `JitContext` stores pending runtime failures as structured diagnostics.
- JIT runtime text output uses the shared runtime renderer, matching VM output
  for code/title/message/span shape.
- JIT JSON output emits directly from `Diagnostic`.
- CLI/runtime reparsing of already-rendered JIT diagnostics was removed.
- VM remains the reference renderer; the only accepted runtime-output gap is
  the missing JIT stack trace.

## Design outcome

### Principle: errors are data, rendering is separate

```
Error producer
  -> Diagnostic

Renderer / CLI boundary
  -> text or json output
```

The JIT no longer exposes raw runtime helper strings as its public error
transport. User-facing compile/runtime failures are structured diagnostics.
Only true internal failures still surface as plain internal messages.

### Current architecture

```
Parser / compiler
  -> Diagnostic -> DiagnosticsAggregator

VM runtime
  -> Diagnostic -> render_runtime_diagnostic

JIT runtime
  -> Diagnostic -> render_runtime_diagnostic / emit_diagnostics
```

## Implementation checklist

- [x] Phase 1: Normalize JIT runtime helper error rendering
- [x] Phase 1: Add `rt_render_error_with_span`
- [x] Phase 1: Emit span rendering after arithmetic ops
- [x] Phase 2: Introduce typed JIT error transport (`Compile`, `Runtime`, `Internal`)
- [x] Phase 2: Store pending JIT runtime failures as diagnostics
- [x] Phase 3: Convert JIT runtime bridges for primops, base calls, indirect calls, and effect errors to structured diagnostics
- [x] Phase 4: Emit JIT text/json output directly from diagnostics
- [ ] Future: JIT shadow stack for stack traces

## Remaining gap

The JIT still does not track runtime stack frames, so it cannot append the VM's
stack trace block. This is explicitly deferred and does not block the proposal.

## Success criteria

VM and JIT now produce matching runtime diagnostic code/title/message/span
signatures for equivalent failures, and JIT json output is emitted directly
from structured diagnostics instead of parsing rendered text.
