# Proposal 040: Hygienic Macro System for Flux

**Status:** Draft  
**Date:** 2026-02-20  
**Depends on:** None
**Supersedes:** `009_macro_system.md`

---


**Status:** Draft  
**Date:** 2026-02-20

---

## 1. Goal

Add a macro system that increases language expressiveness without sacrificing determinism, tooling, or VM/JIT parity.

Design priorities:

- hygienic by default
- deterministic expansion
- expansion happens before lowering (backend-neutral)
- staged rollout from safe pattern macros to limited compile-time evaluation

---

## 2. Scope and Phases

### Phase 1 (v1): Hygienic Pattern Macros

- `macro_rules`-style expression/statement macros
- token-tree/pattern matching + template expansion
- no arbitrary compile-time execution
- module-local macro definitions + explicit imports

### Phase 2: Typed Expansion Validation

- macro expansion outputs must type-check in destination context
- diagnostics point to both call site and expanded fragment

### Phase 3: Limited `comptime` Evaluation

- allow evaluation of pure functions during compilation
- forbid IO/time/control effects at compile time

### Phase 4: Attribute/derive-style Macros

- structured code generation for declarations
- still deterministic and hygiene-preserving

---

## 3. Syntax (Phase 1)

Definition:

```flux
macro_rules! assert_eq {
    ($a, $b) => {
        if $a != $b {
            panic("assert_eq failed")
        }
    }
}
```

Use:

```flux
assert_eq!(x + 1, y)
```

Rules:

- invocation uses `name!(...)`
- macro names live in a dedicated namespace
- explicit import/export for cross-module usage

---

## 4. Hygiene Model

Default: hygienic identifiers.

- identifiers introduced by macro templates are gensym-scoped
- macro expansion cannot accidentally capture user locals
- user-passed identifiers preserve original binding

Escape hatch (later, explicit and rare):

- `unhygienic` mode for advanced metaprogramming (not in Phase 1)

---

## 5. Compiler Pipeline Integration

Add a new compiler stage:

1. Parse source into AST + macro definitions.
2. Build macro environment (module-scoped, import-aware).
3. Expand macro invocations recursively with depth limit.
4. Run existing AST passes on expanded AST.
5. Lower to bytecode/JIT as today.

Expansion must complete before:

- PrimOp/Base lowering
- type/effect checking phases
- optimization passes

---

## 6. Determinism and Caching

Macro expansion must be deterministic for identical inputs:

- no wall-clock/file/network access in Phase 1/2
- stable expansion order
- fixed recursion/expansion limit

Cache impact:

- macro definition hashes become part of module cache key
- expansion output can be cached as a normalized AST artifact

---

## 7. Error Model

Compile-time macro errors:

1. Undefined macro
2. No matching rule
3. Ambiguous match
4. Expansion recursion limit exceeded
5. Invalid expansion form (e.g., expression expected, statement produced)

Diagnostic requirements:

- primary span at invocation site
- note with matched rule span
- optional expanded snippet for debugging

---

## 8. Type System and Effects Interaction

- macros are syntax transforms, not runtime entities
- expanded code is checked by the same type/effect rules as handwritten code
- typed module contracts (`036`) apply after expansion, unchanged
- effect metadata (`PrimEffect`, function `EffectSummary`) comes from expanded result

For Phase 3 `comptime`:

- compile-time evaluation restricted to pure/effect-free operations
- effectful primops/base functions are rejected in `comptime` context

---

## 9. VM/JIT Impact

No runtime opcode additions needed for Phase 1/2.

Reason:

- macros are fully expanded before backend lowering
- VM and JIT consume identical post-expansion AST/IR

Parity risk is low if expansion is centralized and deterministic.

---

## 10. Test Plan

1. Parser tests:
- macro definition/invocation parsing

2. Expansion tests:
- single-rule and multi-rule matches
- hygiene (no accidental capture)
- recursion limit behavior

3. Integration tests:
- macros interacting with match/pipes/modules
- expansion + PrimOp lowering path stability

4. Type/effect tests:
- expanded typed errors are attributed correctly
- expanded effectful code follows effect rules

5. Backend parity:
- VM and JIT outputs identical on macro-heavy fixtures

---

## 11. Rollout Plan

Phase A:
- behind feature flag (`--enable-macros`)
- expression macros only

Phase B:
- statement macros
- module export/import support

Phase C:
- typed expansion diagnostics polish
- enable by default once test coverage is stable

Phase D:
- RFC for limited `comptime` evaluation

---

## 12. Real Benefits

1. Removes boilerplate while staying explicit.
2. Enables domain-specific mini-DSLs without changing core syntax.
3. Preserves backend simplicity (expand first, lower once).
4. Complements typed/effect roadmap rather than bypassing it.
5. Gives Flux a distinctive systems+FP ergonomics layer.
