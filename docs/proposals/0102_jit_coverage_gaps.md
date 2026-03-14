- Feature Name: JIT Coverage Gaps
- Start Date: 2026-03-14
- Status: Draft
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0097 (JIT default path and coverage)

# Proposal 0102: JIT Coverage Gaps

## Summary

Track and fix 9 known JIT test failures that represent gaps in the Cranelift JIT
backend's language coverage. These are pre-existing issues discovered during the
Core IR consolidation work. All tests are currently `#[ignore]`d with references
to this proposal.

## Motivation

The JIT backend must produce identical results to the VM for all valid programs.
Currently, 9 tests across 3 test files fail because the JIT lacks support for
certain language constructs or has parity issues with the VM's error handling.

These gaps prevent `--jit` from being a drop-in replacement for the VM, which
blocks the roadmap goal of JIT-by-default (proposal 0097).

## Failing Tests

### Category A: Nested function statements (2 tests)

**Files:** `tests/jit_phase3_tests.rs`

| Test | Error | Root cause |
|------|-------|-----------|
| `jit_local_function_statement_captures_outer_local` | `missing JIT CFG named tail-call target` | Nested `fn` inside another `fn` not declared in JIT scope |
| `jit_local_recursive_function_statement_works` | `missing JIT CFG named tail-call target` | Same — recursive nested function |

**Root cause:** `predeclare_ir_functions` in `src/jit/compiler.rs` only iterates
top-level `IrTopLevelItem::Function` entries. Nested function statements inside
another function's body (stored in `IrStructuredBlock`) are never registered in
`scope.functions`. When the JIT encounters `IrTerminator::TailCall { callee:
Named(inner_fn) }`, the lookup fails.

**Fix approach:**
1. Make `predeclare_ir_functions` recurse into function bodies
2. Make `compile_ir_functions` also recurse
3. Handle the JIT tagged value representation for nested closures (initial
   recursion attempt hit `unknown JIT tagged value tag: 5`)

**Complexity:** Medium — the declaration/compilation recursion is straightforward,
but nested functions capture outer locals which requires correct closure ABI in
the JIT's tagged value system.

### Category B: String interpolation (3 tests)

**Files:** `tests/jit_phase4_tests.rs`

| Test | Error |
|------|-------|
| `jit_string_interpolation_basic` | `Cranelift Verifier errors` |
| `jit_string_interpolation_expression` | `Cranelift Verifier errors` |
| `jit_string_interpolation_multiple` | `Cranelift Verifier errors` |

**Root cause:** The JIT's `InterpolatedString` lowering generates invalid
Cranelift IR. The verifier rejects it before code generation.

**Fix approach:** Audit the `compile_interpolated_string` path in
`src/jit/compiler.rs`. The issue is likely incorrect block parameter types or
a missing value conversion between string parts and the concatenation helper.

**Complexity:** Low-medium — the fix is contained in one codegen function.

### Category C: VM/JIT parity (4 tests)

**Files:** `tests/primop_vm_jit_parity_tests.rs`

| Test | Error | Root cause |
|------|-------|-----------|
| `vm_and_jit_match_phase2_primop_errors` | Error message mismatch | JIT runtime error format differs from VM |
| `jit_indirect_call_runtime_errors_render_diagnostics` | `tagged array slot must be preallocated` | JIT panic during indirect call error path |
| `vm_and_jit_match_effectful_read_file_primop_value` | `TOP-LEVEL EFFECT` error | JIT effect annotation handling differs from VM |
| `vm_and_jit_match_base_except_with_qualified_access` | Type mismatch | JIT type resolution for `import .. except` differs |

**Root cause:** These are individual parity issues where the JIT handles edge
cases differently from the VM. Each has a distinct root cause.

**Fix approach:**
1. **Error messages**: Normalize JIT runtime error formatting to match VM output
2. **Tagged array panic**: Preallocate the tagged array slot before indirect calls
3. **Effect annotations**: Ensure JIT applies the same effect checking as the VM
4. **Base except**: Fix qualified name resolution in JIT for excluded base members

**Complexity:** Low per issue, but there are 4 distinct fixes.

## Implementation plan

### Phase 1: String interpolation (Category B)

Lowest effort, highest test count. Fix the Cranelift verifier error in
interpolated string codegen. Unblocks 3 tests.

### Phase 2: VM/JIT parity (Category C)

Fix the 4 individual parity issues. Each is a small, contained fix. Unblocks 4 tests.

### Phase 3: Nested function statements (Category A)

Requires careful work on closure ABI in the JIT's tagged value system. Unblocks 2 tests.

## Test tracking

All 9 tests are marked `#[ignore = "... (proposal 0102)"]`. As each category is
fixed, remove the `#[ignore]` annotations and verify CI passes.

```
tests/jit_phase3_tests.rs:
  - jit_local_function_statement_captures_outer_local
  - jit_local_recursive_function_statement_works

tests/jit_phase4_tests.rs:
  - jit_string_interpolation_basic
  - jit_string_interpolation_expression
  - jit_string_interpolation_multiple

tests/primop_vm_jit_parity_tests.rs:
  - vm_and_jit_match_phase2_primop_errors
  - jit_indirect_call_runtime_errors_render_diagnostics
  - vm_and_jit_match_effectful_read_file_primop_value
  - vm_and_jit_match_base_except_with_qualified_access
```

### Category D: Release parity (5 tests)

**Files:** `tests/runtime_vm_jit_parity_release.rs`

| Test | Error | Root cause |
|------|-------|-----------|
| `release_runtime_parity_tail_recursive_countdown` | Stack overflow | JIT doesn't apply tail-call optimization for simple recursion |
| `release_jit_base_runtime_errors_use_full_span_highlights` | Span mismatch | JIT runtime error spans differ from VM |
| `release_jit_primop_runtime_errors_use_full_span_highlights` | Span mismatch | Same — primop error highlight parity |
| `release_jit_indirect_call_wrong_arity_renders_runtime_signature` | Output mismatch | JIT indirect call error differs from VM |
| `release_jit_indirect_call_not_callable_renders_runtime_signature` | Output mismatch | Same — non-callable value error |

**Fix approach:** Tail-call optimization in JIT needs to work for simple recursion.
Runtime error formatting needs to match VM output (spans, messages, stack traces).

```
tests/runtime_vm_jit_parity_release.rs:
  - release_runtime_parity_tail_recursive_countdown
  - release_jit_base_runtime_errors_use_full_span_highlights
  - release_jit_primop_runtime_errors_use_full_span_highlights
  - release_jit_indirect_call_wrong_arity_renders_runtime_signature
  - release_jit_indirect_call_not_callable_renders_runtime_signature
```

## Success criteria

All 14 tests pass without `#[ignore]`. `cargo test --all --all-features` reports
0 failures.
