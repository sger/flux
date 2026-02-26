# Proposal 039: Typed Module Contracts (Boundary-First Typing)

**Status:** Draft  
**Date:** 2026-02-20  
**Depends on:** None

---


**Status:** Draft  
**Date:** 2026-02-20

---

## 1. Motivation

Flux remains gradual/dynamic internally, but most production failures happen at API boundaries between modules.

Typed Module Contracts add type/effect guarantees at exports/imports first, without requiring full-program inference.

This delivers immediate value:

- safer module APIs
- clearer compiler diagnostics at call boundaries
- low migration cost for existing dynamic code

---

## 2. Scope

v1 scope:

- typed signatures for exported module functions
- compile-time checks at typed call sites
- runtime boundary checks (`Any -> T`) at dynamic call sites
- effect annotation checks on exported signatures (`with IO`, etc.)

Out of scope:

- full Hindley-Milner over entire program
- type classes/traits
- full algebraic effect handler typing

---

## 3. User Model

Example:

```flux
module Math {
    export fn add(a: Int, b: Int) -> Int { a + b }
}
```

Contract rule:

- exported functions must carry explicit parameter/return annotations
- unexported/private functions may remain inferred/dynamic in v1

Call behavior:

- typed caller + typed callee: compile-time mismatch error
- dynamic caller + typed callee: runtime boundary cast at call entry

---

## 4. Type Surface (v1)

Supported contract types:

- `Int`, `Float`, `Bool`, `String`
- `Array<T>`, `Map<K, V>`, `Option<T>`
- `Any` (explicit escape hatch)
- function return types + effect clause `with ...`

Deferred:

- user-defined ADTs in contracts
- full tuple polymorphism
- higher-kinded generic constraints

---

## 5. Compiler Design

Add `ModuleContractTable` built during module compilation:

- key: `(module_name, function_name, arity)`
- value: `FnContract { params, ret, effects }`

Lowering steps:

1. Parse/store annotations on exported functions.
2. Validate annotation syntax + referenced type names.
3. At direct calls to known exported symbols:
   - if argument types known and incompatible -> compile-time error
   - if argument is `Any`/unknown -> emit runtime boundary check node
4. For unknown/generic calls, keep existing dynamic path.

No syntax break required if `export` already exists; otherwise module export marker can be introduced in same phase.

---

## 6. Runtime/VM Design

Introduce lightweight runtime contract checks:

- `check_arg_type(value, expected_type, span)`
- `check_return_type(value, expected_type, span)`

Check insertion points:

- function entry for exported typed functions called from dynamic sites
- function return boundary before handing value to typed caller

Behavior:

- on mismatch: structured runtime type error with expected/actual + source location
- keep error wording aligned with existing diagnostics style

---

## 7. JIT Parity

Policy parity:

- JIT must enforce the same boundary checks as VM.

Implementation strategy:

- represent boundary checks as shared runtime helper calls first
- later specialize hot checks (`Int`, `Float`, `Bool`) inline in Cranelift when safe

Acceptance condition:

- VM and JIT produce equivalent failures/success for contract violations.

---

## 8. Effect Contract Layer

Exported contracts also carry declared effect set.

v1 rules:

- if export signature is pure, body cannot call effectful primops/base functions
- if export signature has `with IO`, IO operations are allowed
- typed callers must satisfy callee effect requirements

This leverages existing `PrimEffect` + `EffectSummary` groundwork.

---

## 9. Diagnostics

Compile-time mismatch:

```
error[E3xx]: contract mismatch calling Math.add
expected: (Int, Int) -> Int
found:    (String, Int) -> ?
at: src/main.flx:12:15
```

Runtime boundary mismatch:

```
runtime type error: contract violation at Math.add argument #1
expected: Int
actual: String
at: src/main.flx:12:15
```

---

## 10. Test Plan

1. Parser/AST tests:
- exported signatures parsed and stored correctly

2. Compiler tests:
- typed caller mismatch yields compile-time error
- unknown/dynamic caller emits boundary checks

3. VM tests:
- runtime contract mismatch fails with expected message
- successful contract calls unchanged semantically

4. JIT parity tests:
- same pass/fail outcomes as VM on contract fixtures

5. Regression:
- existing untyped modules continue to run unchanged

---

## 11. Rollout Plan

Phase A:
- read/store contract metadata
- no enforcement (warn-only mode)

Phase B:
- enforce compile-time checks for typed call sites
- insert runtime boundary checks for dynamic crossings

Phase C:
- effect clause enforcement on exported signatures
- strict-mode requirement for exported API annotations

---

## 12. Real Benefits

1. Safer public APIs now, without waiting for full type-system completion.
2. Better errors at boundaries where teams integrate modules.
3. Keeps gradual typing promise: internals can stay dynamic.
4. Gives JIT future specialization anchors (contract-known types).
5. Natural stepping stone toward full proposal `030`.
