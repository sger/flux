- Feature Name: Static Purity Completion
- Start Date: 2026-03-11
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0053 (traits and typeclasses), 0086 (backend-neutral core IR), 0098 (Cranelift JIT improvements / Flux IR)

# Proposal 0099: Static Purity Completion

## Summary

Three interdependent changes that together complete Flux as a statically pure functional language:

1. **IO as a first-class algebraic effect** — remove the hardcoded `IO` privilege from 77 base functions and make `IO` a user-programmable effect like any other.
2. **`Any` elimination from user-facing code** — once trait constraints (0053) cover all ad-hoc polymorphism use cases, make unresolved type variables a compile error rather than a silent fallback to `Any`.
3. **Monomorphization at the Flux IR layer** — once the type system is closed (no `Any` in user code, no unresolved vars escaping to runtime), specialize generic functions per concrete type in the IR and eliminate runtime `Value` tags from typed code paths.

Together these changes move Flux from a *pragmatic interpreter model with good FP primitives* to a language where purity is a structural property of the type system, not a convention.

## Motivation

Flux has HM type inference, algebraic effects with effect rows, ADTs, pattern matching, and persistent collections. The foundations are correct. But three gaps prevent the language from being fully statically pure:

### Gap 1: IO is not an algebraic effect

`IO` is hardcoded into 77 base functions in `src/runtime/base/`. The `with IO` annotation is a *capability grant* — once a function has it, unrestricted side effects are permitted with no sequencing requirements and no ability for user code to intercept, mock, or handle them.

This contradicts the algebraic effects model that Flux already implements for custom effects. A function can write a handler for a user-defined `Console` effect and intercept every `perform Console.print(...)` call. But it cannot do the same for the built-in `print` function. IO is a second-class citizen in the effect system.

Consequence: IO-heavy code is untestable without actually performing IO. There is no way to write a pure test that exercises a function using `print` or file I/O.

### Gap 2: `Any` silently undermines static typing

`InferType::Con(TypeConstructor::Any)` unifies with everything. At the VM boundary, unresolved type variables silently become `RuntimeType::Any`, which accepts any `Value` at runtime. This means type errors that should be caught at compile time are deferred to runtime, or silently accepted.

The `Any` type exists primarily because Flux lacks the ad-hoc polymorphism needed to express base functions like `print` (which must work for any `Show`-able type) without a dynamic escape. Once proposal 0053 (traits/typeclasses) delivers `Show`, `Eq`, `Ord`, and `Hash`, the legitimate uses of `Any` in user-facing code are covered by bounded polymorphism, and `Any` can be restricted to compiler-internal use.

Consequence: The type system gives a false guarantee. A program that type-checks may still fail with a runtime type mismatch if `Any` was involved in the inference.

### Gap 3: The `Value` enum is the dynamism

`src/runtime/value.rs` defines a 25-variant tagged union that is the universal runtime representation. Every value carries a runtime type tag. In a fully static language, you know the type of every value at compile time — the tag is redundant for typed code paths and exists only because the type might not be resolved.

This is the performance consequence of gaps 1 and 2. The Flux JIT (proposal 0098) is partially working around this with type-directed unboxing passes, but this is treating the symptom. The root cause is that as long as `Any` can appear anywhere and IO is untyped at the effect level, the compiler cannot trust type information enough to eliminate tags.

Consequence: The JIT is 17–18× slower than Rust on arithmetic-heavy code. The `Value` enum forces boxing and tag checks that native FP compilers (GHC, MLton, OCaml) eliminate via monomorphization or type erasure.

## Guide-level explanation

### Part 1: IO as an algebraic effect

After this change, `IO` is declared as a built-in effect with a fixed set of operations. User code interacts with it the same way as any other algebraic effect:

```flux
-- IO is now a built-in algebraic effect
effect IO {
    print:     String -> Unit
    print_int: Int -> Unit
    read_line: Unit -> String
    read_file: String -> String
    write_file: String -> String -> Unit
}
```

Base functions that previously had `with IO` baked in now `perform` into the `IO` effect:

```flux
-- Before (hardcoded, uninterceptable)
fn print(x: String) with IO { ... }  -- calls C function directly

-- After (algebraic, interceptable)
fn print(x: String) with IO {
    perform IO.print(x)
}
```

The `main` function provides the canonical IO handler that connects to real system calls:

```flux
fn main() with IO {
    print("hello world")
}
-- The runtime wraps main with the default IO handler automatically
```

The payoff is testability. User code can intercept IO in tests:

```flux
fn test_greet() {
    let output = ref([||])
    greet("Alice") handle IO {
        print(resume, msg) -> {
            output := array_push(*output, msg)
            resume(())
        }
    }
    assert(*output == [|"Hello, Alice!"|])
}
```

Functions that do not perform IO are verified pure by the type system — `with IO` is absent from their inferred type. This was already true in principle; it becomes enforceable in tests without mocking.

### Part 2: `Any` elimination

After proposal 0053 lands with `Show`, `Eq`, `Ord`, and `Hash`, the base function signatures change:

```flux
-- Before
fn print(x: Any) with IO { ... }
fn equal(a: Any, b: Any) -> Bool { ... }
fn to_string(x: Any) -> String { ... }

-- After
fn print<T: Show>(x: T) with IO { perform IO.print(show(x)) }
fn equal<T: Eq>(a: T, b: T) -> Bool { T.eq(a, b) }
fn to_string<T: Show>(x: T) -> String { show(x) }
```

With this in place, `Any` in user-facing types becomes a compile error:

```flux
-- Error: type variable 'a' is not resolved; add a type annotation or constraint
fn mystery(x) { x }
```

Unresolved type variables that previously produced `RuntimeType::Any` now produce a diagnostic:

```
error[E0201]: unresolved type variable in public function signature
  --> src/lib.flx:4:1
   |
 4 | fn mystery(x) { x }
   | ^^^^^^^^^^^^^^^^^^^
   |
   = hint: add a type annotation: fn mystery<T>(x: T) -> T
   = hint: or add a constraint: fn mystery<T: Show>(x: T) -> T
```

`Any` is retained as a compiler-internal concept for FFI boundaries and the bytecode VM's dynamic dispatch path, but it never appears in inferred types shown to users or in positions that affect type checking of user code.

### Part 3: Monomorphization

Once the IR layer (proposal 0098) exists and `Any` is eliminated from user code, the IR lowering pass can specialize generic functions. Given:

```flux
fn identity<T>(x: T) -> T { x }

let a = identity(42)      -- T = Int
let b = identity("hello") -- T = String
```

The IR lowering produces two functions:

```
ir.fn identity_Int(x: Int) -> Int { return x }
ir.fn identity_String(x: String) -> String { return x }
```

Each specialized function carries concrete `FluxIRType` on every value. The Cranelift backend emits code with no runtime tags, no boxing, and no `Value` enum for these paths. Arithmetic on `Int` compiles to plain `iadd`/`imul` without any helper calls.

The bytecode VM retains the `Value` enum as its execution model — monomorphization is a JIT-only optimization in the first phase.

## Reference-level explanation

### Part 1: IO effect implementation

**Effect declaration:** Add `IO` to the built-in effect environment in `src/ast/type_infer/mod.rs`. IO is pre-declared before user modules are processed, the same way `Option` and `Either` are pre-declared.

```rust
// src/ast/type_infer/builtin_effects.rs (new file)
pub fn register_builtin_effects(env: &mut TypeEnv) {
    // effect IO { print: String -> Unit, read_line: Unit -> String, ... }
    env.define_builtin_effect("IO", vec![
        ("print",      fun_type(vec![string_type()], unit_type())),
        ("print_int",  fun_type(vec![int_type()],    unit_type())),
        ("read_line",  fun_type(vec![unit_type()],   string_type())),
        ("read_file",  fun_type(vec![string_type()], string_type())),
        ("write_file", fun_type(vec![string_type(), string_type()], unit_type())),
    ]);
}
```

**Base function migration:** Each of the 77 base functions with `with IO` is rewritten to `perform IO.<op>(...)`. The runtime provides a default IO handler (`DefaultIOHandler`) that dispatches to the existing C-level implementations. This handler is installed by the VM/JIT before `main` is called.

**Default handler installation:** `src/runtime/vm/mod.rs` and `src/jit/mod.rs` wrap `main` invocation with the default handler. User code never sees this — it is transparent unless they install their own handler.

**Effect row impact:** `IO` continues to appear in function types as `with IO`. The row solver already handles this correctly. No changes to row normalization or constraint solving are needed.

**Backward compatibility:** All existing `.flx` files that use `print`, `read_line`, etc. continue to work. The only breaking change is that `IO` operations can now be intercepted by handlers, which is additive.

### Part 2: `Any` elimination

**Phase A — Audit and enumerate legitimate `Any` uses.** Grep `TypeConstructor::Any` across `src/types/` and `src/runtime/`. Categorize each use as:
- (a) Base function ad-hoc polymorphism → replace with trait constraint after 0053 lands
- (b) Unresolved type variable fallback → replace with compile error
- (c) FFI/VM internal → keep, mark with `#[allow(any_type)]` annotation

**Phase B — Replace (a) uses.** For each base function currently typed `Any -> ...`, update the signature to use the appropriate trait constraint. This requires 0053 to be complete and the trait environment to be available during base function type checking.

**Phase C — Make (b) a diagnostic.** In `src/types/type_env.rs`, the `to_runtime` function currently maps `InferType::Var(_)` to `RuntimeType::Any`. Change this to emit a `E0201` diagnostic (new error code) and abort compilation. In `--strict` mode this is already the behavior; this change makes it the default.

```rust
// src/types/type_env.rs — to_runtime
InferType::Var(id) => {
    // Previously: RuntimeType::Any (silent fallback)
    // Now: compile error
    diagnostics.emit(diag_enhanced(E0201_UNRESOLVED_TYPE_VAR)
        .with_span(span)
        .with_message(format!("type variable ${} is not resolved", id)));
    RuntimeType::Any // placeholder; compilation aborts before codegen
}
```

**Phase D — Hide `Any` from user-facing output.** Diagnostic rendering never displays `Any` in inferred types shown to users. Instead it displays `_` (unknown) with a hint to add an annotation.

### Part 3: Monomorphization

This phase depends on Flux IR (proposal 0098, step S7–S9) being complete.

**Monomorphization pass** (`src/ir/passes/monomorphize.rs`):

1. Collect all call sites of generic functions in the IR module.
2. For each call site, record the concrete `FluxIRType` substitution for each type parameter.
3. For each unique substitution, clone the generic IR function and substitute concrete types throughout.
4. Replace the polymorphic call site with a call to the specialized clone.
5. Remove the original generic function from the module (it is now dead).

**Type representation after monomorphization:**

| Before | After |
|--------|-------|
| `IrVar` typed as `FluxIRType::Var(T)` | `IrVar` typed as `FluxIRType::Int` / `FluxIRType::String` / etc. |
| `Value` tag checked at runtime | Tag eliminated; Cranelift emits raw `i64` / pointer |
| `rt_add` helper call | Cranelift `iadd` directly |

**Scope limitation:** Monomorphization applies to functions whose type parameters are fully resolved at all call sites. Functions that are called with `Any` (FFI, dynamic dispatch through base layer) remain polymorphic and use the existing `Value` boxing path.

**Code size trade-off:** Monomorphization increases binary size. A threshold (configurable, default: 4 specializations per generic function) triggers a fallback to dictionary passing for functions with many distinct instantiations. This mirrors Rust's `#[inline]` / `#[cold]` trade-off.

### Interaction with existing proposals

| Proposal | Interaction |
|----------|-------------|
| 0053 (traits) | Required before Part 2. Trait constraints replace `Any` in base function signatures. |
| 0086 (backend-neutral IR) | Part 3 requires a typed IR layer. 0086 defines the IR structure; 0098 implements it. |
| 0098 (Flux IR) | Part 3 adds the monomorphization pass as `src/ir/passes/monomorphize.rs`. Runs after existing passes (constant folding, dead block elimination) and before Cranelift lowering. |
| 0038 (deterministic effect replay) | IO as algebraic effect makes IO operations replayable via the existing continuation capture mechanism — a prerequisite for deterministic replay. |
| 0068 (Perceus uniqueness analysis) | Monomorphized IR exposes more opportunities for Perceus in-place update because field types are concrete, enabling precise alias analysis. |

## Drawbacks

**Part 1 (IO as algebraic effect):**
- The default IO handler adds one indirection layer on every IO call. This is negligible for IO-bound programs but measurable in microbenchmarks that call `print` in a tight loop.
- Existing code that captures the `IO` effect for mocking may need adjustment if the default handler installation point changes.

**Part 2 (`Any` elimination):**
- Breaking change for any user code that relied on `Any` implicitly (e.g. functions with no type annotation that happened to pass type checking via `Any` fallback). The migration path is clear (add annotation or constraint) but it requires action.
- The transition period where 0053 is partially complete but `Any` is not yet eliminated creates a gap where some `Any` uses have trait replacements and others do not.

**Part 3 (monomorphization):**
- Binary size growth for programs with many generic functions and many distinct instantiations. The threshold-based fallback mitigates this but adds complexity.
- The monomorphization pass must be kept in sync with the IR definition. Any new `IrInstr` variant that carries type information must be updated in the substitution logic.
- Monomorphization interacts subtly with algebraic effects: a generic function with an effect row `with e` must be specialized for both the effect type variable `e` and the value type variables. This requires effect monomorphization in addition to type monomorphization.

## Rationale and alternatives

### Why IO as algebraic effect rather than IO monad?

Flux already has algebraic effects with delimited continuations. Introducing an IO monad would create two parallel effect models — one algebraic (for user-defined effects) and one monadic (for IO). This is worse than either model alone. Making IO algebraic gives a uniform programming model: one mechanism, all effects.

The cost of the IO monad (explicit `bind`/`>>=` sequencing, `do`-notation) is the main adoption barrier for Haskell. Flux's algebraic effect model provides the same sequencing guarantees without requiring monadic notation.

### Why not keep `Any` and improve inference instead?

Better inference could reduce the *frequency* of `Any` appearing in practice but cannot eliminate it structurally. `Any` unifies with everything — as long as it exists in the type system, a single unconstrained expression can silently propagate `Any` to its context. The only fix is to remove it from positions where it affects type checking.

### Why monomorphization instead of NaN-boxing (proposal 0041)?

NaN-boxing reduces the *size* of the `Value` representation (from 16–24 bytes to 8 bytes) and eliminates the pointer indirection for small scalars. It does not eliminate the runtime tag check — every operation must still inspect the tag to determine the value kind.

Monomorphization eliminates the tag entirely for typed code paths. The two are complementary: NaN-boxing is a better representation for the bytecode VM and for polymorphic code paths that remain; monomorphization is better for the JIT's typed fast paths.

The right long-term order is: monomorphization first (eliminates tags for typed paths), NaN-boxing second (improves the fallback representation for remaining dynamic paths).

### Why not dictionary passing for generics?

Dictionary passing (the GHC/Haskell approach for type class resolution) has zero code size cost — one generic function, one dictionary argument per constraint. It has a runtime cost: every constrained call indirects through the dictionary. For a JIT compiler, this is a missed optimization: if the concrete type is known at the call site (which it always is after inlining), the dictionary call is an unnecessary indirect branch.

Monomorphization eliminates this indirection at the cost of code size. Flux's threshold-based fallback (dictionary passing for heavily-instantiated generics, monomorphization for the common case) gives the best of both.

## Prior art

- **GHC (Haskell):** Uses a mixed model — dictionary passing for type class constraints, worker/wrapper for unboxing, and a typed intermediate language (Core/Cmm) for optimization. GHC's `IO` monad is the canonical monadic IO model; Flux's algebraic approach is closer to Frank or Koka.
- **Koka:** IO is an algebraic effect named `io`. All built-in operations that perform IO are typed `io` and can be handled by user code. Koka's evidence-passing handler compilation is the state of the art for efficient algebraic effect dispatch.
- **MLton:** Whole-program monomorphization. Every polymorphic function is specialized; no runtime tags in generated code. MLton achieves near-C performance for ML programs.
- **OCaml 5:** Algebraic effects are now a first-class language feature. The OCaml stdlib is gradually being annotated with effect types. The transition from untyped effects to typed effect rows is analogous to Flux's IO migration.
- **Lean 4:** IO is an algebraic effect backed by a state monad over the real world. The `do`-notation desugars to monadic bind but the runtime representation is efficient (no heap allocation for pure bind chains).

## Unresolved questions

1. **Effect monomorphization:** How should generic functions with effect type variables (`fn f<e>(x: Int) -> Int with e`) be monomorphized? Should `e` be specialized per handler, or is effect polymorphism always resolved by dictionary passing?

2. **IO handler granularity:** Should the default IO handler be one handler for all IO operations, or should each operation (`print`, `read_line`, etc.) have an independently overridable default? Finer granularity enables partial mocking but complicates the handler installation protocol.

3. **`Any` in FFI:** External functions (C FFI, future WASM imports) have no Flux types. Should they be typed `Any -> Any` (current implicit behavior) or should they require explicit `extern` type signatures? The latter is safer but requires a foreign type annotation syntax that does not yet exist.

4. **Monomorphization threshold:** What is the right default threshold (number of specializations) before falling back to dictionary passing? This needs benchmark data across real Flux programs.

5. **Diagnostic for `Any` in user types:** When `Any` appears in a type inferred for a public function (e.g. because a called base function has not yet been migrated to traits), should the error be on the call site or the definition? The call site is more actionable; the definition is more correct.

## Future possibilities

- **WASM backend:** With a typed Flux IR and monomorphized functions, a WASM lowerer (`src/wasm/`) can target WASM's typed value stack directly. Generic `Value` boxing is the main blocker for efficient WASM output.
- **Escape analysis:** Monomorphized IR enables field-level escape analysis. A `List<Int>` known to be local can be stack-allocated, eliminating GC pressure for short-lived accumulators.
- **Effect sealing (proposal 0075):** IO as algebraic effect is a prerequisite for effect sealing — the ability to close a handler scope and guarantee no further effects escape it. Sealing cannot work for IO when IO bypasses the handler mechanism.
- **Profile-guided specialization:** The monomorphization pass could be guided by runtime type frequency data (from JIT traces) to specialize only the hot instantiations and use dictionary passing for cold ones.
- **`Show` derivation:** Once `Show` is a real trait, the compiler can auto-derive `Show` implementations for ADTs, records, and tuples — the same derive mechanism Rust uses for `Debug`/`Display`.

## Implementation sequence

| Step | Part | Description |
|------|------|-------------|
| I1 | 1 | Declare `IO` as a built-in algebraic effect in the type environment |
| I2 | 1 | Implement `DefaultIOHandler` in `src/runtime/vm/` and `src/jit/` |
| I3 | 1 | Migrate `print`, `print_int`, `print_float` to `perform IO.*` (proof of concept) |
| I4 | 1 | Migrate remaining 74 base functions; remove `with IO` hardcoding from base registry |
| I5 | 2 | Wait for 0053 Phase A (`Eq`, `Ord`, `Show`) to land |
| I6 | 2 | Migrate base function signatures from `Any` to trait constraints |
| I7 | 2 | Add `E0201` diagnostic for unresolved type variables escaping to runtime |
| I8 | 2 | Enable E0201 in `--strict` mode first; make default after one release cycle |
| I9 | 3 | Wait for 0098 steps S7–S9 (Flux IR scaffold + JIT consumes IR) |
| I10 | 3 | Implement `src/ir/passes/monomorphize.rs` |
| I11 | 3 | Wire monomorphization pass into the IR optimization pipeline |
| I12 | 3 | Benchmark and tune specialization threshold |

Steps I1–I4 are independent of 0053 and can begin immediately. Steps I5–I8 are gated on 0053. Steps I9–I12 are gated on 0098.
