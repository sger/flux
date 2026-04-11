# Flux v0.0.5 Implementation Plan

## Overview

**Theme: Language Completeness**

v0.0.5 expands Flux from a well-typed functional language into one with traits,
records, and a mature compiler architecture. Building on v0.0.4's hardening work
and the Core IR optimization framework (proposals 0101/0102), this release
focuses on filling the remaining language gaps that block real-world adoption.

---

## Current State (v0.0.4 — Complete)

**Foundations delivered:**
- HM type inference with strict typed-path authority (no `Any` fallback in typed paths)
- ADT semantics hardened with constructor/arity/type checks
- Strong exhaustiveness checking (general + ADT-specific)
- VM/JIT diagnostics parity locked
- Core IR optimization framework: 7 passes (beta, case-of-case, COKC, inlining, dead-let, evidence-passing, ANF)
- Effect handler optimizations: static resolution + evidence-passing (proposals 0101/0102)
- Typed backend IR: `IrFunction` carries HM-inferred param/return types

**Current gaps for v0.0.5:**
- No traits/typeclasses — `print`, `equal`, `min`, `max` use `Any` for ad-hoc polymorphism
- No typed records — only hash maps with string keys
- Bytecode compiler is a 11K-line monolith — hard to extend safely
- No selective imports (`import Foo { bar }`) — only whole-module import
- No numeric conversion functions (`to_float`, `to_int`)

---

## Version Goals for v0.0.5

**Primary objectives:**
1. **Compiler architecture** (0044): Split the bytecode compiler into explicit phase modules
2. **Traits Phase A** (0053): Deliver `Show`, `Eq`, `Ord` traits with constrained polymorphism
3. **Typed records** (0048): Named-field struct types with type-checked access
4. **Module system usability** (0011): Selective imports

**Secondary objectives:**
5. Numeric conversion builtins (`to_float`, `to_int`)
6. Fix pre-HM example files with mixed-type arithmetic
7. Expand test/example coverage for new features

**Success criteria:**
- `print` uses `Show` trait constraint instead of `Any`
- Record types with named fields work in pattern matching
- Bytecode compiler split into ≤1000-line focused modules
- `import Foo { bar, baz }` syntax works
- All existing tests + 102/102 VM/JIT parity maintained

---

## Timeline: 8 weeks

```
┌─────────────────────────────────────────────────────────────────┐
│ Weeks 1-2: Compiler Phase Pipeline (0044)                       │
│   ✓ Split mod.rs (3033 lines) into phase modules                │
│   ✓ Split expression.rs (4628 lines) into focused compilers     │
│   ✓ Add phase timing infrastructure                             │
│   ✓ Zero behavioral changes — all tests green                   │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Weeks 3-5: Traits Phase A (0053)                                │
│   ✓ Trait/impl syntax parsing                                   │
│   ✓ Trait environment + coherence checking                      │
│   ✓ Constrained HM integration (Show, Eq, Ord)                 │
│   ✓ Migrate base functions from Any to trait constraints        │
│   ✓ Deterministic trait-error diagnostics                       │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Weeks 6-7: Typed Records (0048) + Selective Imports (0011)      │
│   ✓ Record type syntax: { name: String, age: Int }              │
│   ✓ Record construction + field access + pattern matching       │
│   ✓ Import Foo { bar, baz } selective import syntax             │
│   ✓ Numeric conversion builtins (to_float, to_int)              │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 8: Polish + Release                                        │
│   ✓ Fix pre-HM example files                                    │
│   ✓ Documentation updates                                       │
│   ✓ VM/JIT parity lock                                          │
│   ✓ Release sign-off                                            │
└─────────────────────────────────────────────────────────────────┘
```

---

## Milestone Details

### M1: Compiler Phase Pipeline (0044) — Weeks 1-2

**Proposal:** [0044_compiler_phase_pipeline_refactor.md](../proposals/0044_compiler_phase_pipeline_refactor.md)

**Problem:** The bytecode compiler is concentrated in three large files:
- `mod.rs` — 3033 lines (state, inference, validation, codegen)
- `expression.rs` — 4628 lines (all expression compilation)
- `statement.rs` — 1014 lines

**Approach:** Follow the same pattern used for Core IR (0102 Phase 0):

```
src/bytecode/compiler/
├── mod.rs              → Compiler struct + public API (~500 lines)
├── pipeline.rs         → Phase runner, phase ordering (~200 lines)
├── passes/
│   ├── prepare.rs      → State reset, imports, module setup
│   ├── collect.rs      → Contract/ADT/effect declaration collection
│   ├── infer.rs        → HM inference orchestration (1-phase and 2-phase)
│   ├── validate.rs     → Entrypoint, purity, effect validation
│   └── codegen.rs      → PASS 2 bytecode emission dispatch
├── expression/
│   ├── mod.rs          → Expression compilation dispatch
│   ├── literals.rs     → Literal/identifier/string compilation
│   ├── operators.rs    → Infix/prefix/pipe compilation
│   ├── control.rs      → If/match/case compilation
│   ├── functions.rs    → Function/lambda/call compilation
│   ├── collections.rs  → Array/list/hash/tuple compilation
│   ├── effects.rs      → Handle/perform compilation + evidence
│   └── access.rs       → Index/member/tuple-field compilation
├── statement.rs        → Statement compilation (keep, ~1000 lines is OK)
├── ... (existing helper files unchanged)
```

**Validation:**
```bash
cargo test --all --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo run -- parity-check tests/parity
```

---

### M2: Traits Phase A (0053) — Weeks 3-5

**Proposal:** [0053_traits_and_typeclasses.md](../proposals/0053_traits_and_typeclasses.md)

**Scope (Phase A only):**

**Syntax:**
```flux
trait Show {
    show: Self -> String
}

trait Eq {
    eq: (Self, Self) -> Bool
}

trait Ord : Eq {
    compare: (Self, Self) -> Int
}

impl Show for Int {
    fn show(x) { to_string(x) }
}
```

**Implementation steps:**
1. Parser: `trait` and `impl` declarations
2. Trait environment: table of trait definitions + implementations
3. Coherence: orphan rule enforcement (impl must be in same module as trait or type)
4. HM integration: constrained type schemes `fn print<T: Show>(x: T) -> Unit`
5. Base function migration: `print(Any)` → `print<T: Show>(T)`
6. Diagnostics: trait-not-satisfied error messages

**Built-in trait implementations:**
- `Show` for Int, Float, Bool, String, Unit, List, Array, Option, Either
- `Eq` for Int, Float, Bool, String, Unit
- `Ord` for Int, Float, String

**Deferred to Phase B:**
- `Functor`, `Foldable` (higher-kinded)
- Deriving mechanism
- Associated types

---

### M3: Typed Records (0048) — Week 6

**Proposal:** [0048_typed_record_types.md](../proposals/0048_typed_record_types.md)

**Syntax:**
```flux
type Point = { x: Float, y: Float }
type Person = { name: String, age: Int }

let p = Point { x: 1.0, y: 2.0 }
let n = p.name   // type error: Point has no field 'name'

match person {
    { name, age } if age >= 18 -> name + " is an adult"
    { name, .. } -> name + " is a minor"
}
```

**Implementation:**
- Parser: record type syntax in `type` declarations
- Record expression: `TypeName { field: value, ... }`
- Field access: `expr.field` with type-checked resolution
- Pattern matching: `{ field1, field2, .. }` destructuring
- HM integration: record types as named product types

---

### M4: Selective Imports + Builtins — Week 7

**Selective imports (0011):**
```flux
import Math { square, cube }
square(5)      // OK — imported directly
Math.add(1,2)  // Error — not selectively imported
```

**Numeric conversion builtins:**
```flux
fn to_float(x: Int) -> Float      // Int → Float widening
fn to_int(x: Float) -> Int        // Float → Int truncation
```

These are simple base function additions in `src/runtime/base/`.

---

### M5: Polish + Release — Week 8

- Fix remaining pre-HM example files with mixed-type arithmetic
- Update documentation: `docs/versions/v0.0.5.md`
- Expand examples for traits, records, selective imports
- Final VM/JIT parity lock
- Release sign-off via `scripts/release_check.sh`

---

## Dependency Graph

```
M1 (Compiler Split) ─────────→ M2 (Traits)
                                    │
                                    ├──→ M3 (Records) ← uses Show/Eq for printing
                                    │
                                    └──→ M4 (Imports + Builtins)
                                              │
                                              └──→ M5 (Polish + Release)
```

M1 is the prerequisite — splitting the compiler first makes M2-M4 cleaner to implement.

---

## Risk Management

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Traits add complexity to HM | Medium | High | Phase A only (Show/Eq/Ord); no higher-kinded types |
| Record types interact with hash syntax | Medium | Medium | Records use `TypeName { }`, hashes use `{ }` — distinct |
| Compiler split introduces subtle bugs | Low | Medium | Phase-by-phase extraction + full test suite after each |
| Scope creep from traits Phase B | Medium | High | Hard boundary: no Functor/deriving in v0.0.5 |

---

## Release Gate Checklist (v0.0.5)

1. Bytecode compiler split into focused modules (no file >1500 lines)
2. `trait Show/Eq/Ord` with `impl` blocks working
3. `print` uses `Show` constraint (not `Any`)
4. Record types with construction, access, and pattern matching
5. `import Foo { bar }` selective imports working
6. `to_float`, `to_int` conversion builtins
7. All existing tests green
8. 102/102 VM/JIT parity (or more, with new examples)
9. `cargo clippy` + `cargo fmt` clean
10. Documentation updated

---

## Post-v0.0.5 Horizon

| Feature | Proposal | Depends On |
|---------|----------|-----------|
| Traits Phase B (Functor, deriving) | 0053 | v0.0.5 Phase A |
| Any elimination from user code | 0099 Part 2 | Traits Phase A |
| Monomorphization | 0099 Part 3 | Any elimination |
| Auto-currying | 0052 | Traits (for ergonomics) |
| Package manager MVP | 0015 | Selective imports |
| IO as algebraic effect | 0099 Part 1 | Independent |
