- Feature Name: Full Static Typing — From Gradual to Haskell-Like Type Safety
- Start Date: 2026-03-25
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0119 (Typed LLVM Codegen), Proposal 0120 (Unified Base Library)

## Summary

Transition Flux from a gradually-typed language (where unannotated code infers as `Any`) to a fully statically-typed language where every expression has a concrete type at compile time and ill-typed programs are rejected before execution. This follows the Haskell model: the compiler proves type safety, runtime type errors become impossible.

The transition is incremental — each phase adds type system features while maintaining backward compatibility through a `--strict-types` flag that becomes the default in a future release.

## Motivation

### The problem with gradual typing

Today, Flux uses `Any` as a universal escape hatch. When HM inference can't resolve a type, it falls back to `Any`:

```flux
fn add(x, y) { x + y }     // inferred as: Any -> Any -> Any
fn double(x) { x * 2 }     // inferred as: Any -> Any

fn main() with IO {
    print(add(1, "hello"))  // compiles, crashes at runtime
    print(double([1, 2]))   // compiles, crashes at runtime
}
```

In Haskell, both calls are rejected at compile time. In Flux, they compile and fail at runtime with "type mismatch" errors. This defeats the purpose of having a type system.

### Why gradual typing exists in Flux

The `Any` fallback was introduced for pragmatic reasons:

1. **Polymorphic base functions**: `len` works on strings, arrays, and lists. Without type classes, the only way to express this is `len : Any -> Int`.
2. **Missing type classes**: Operators like `+`, `==`, `>` need to work on multiple types. Without `Num`, `Eq`, `Ord` type classes, they're typed as `Any -> Any -> Any`.
3. **Untyped legacy code**: Early Flux code has no type annotations. Gradual typing lets it compile.

### The GHC model

Haskell has no `Any`. Every expression has a concrete type. Polymorphism is explicit through type variables and type classes:

```haskell
add :: Num a => a -> a -> a
add x y = x + y

len :: Foldable t => t a -> Int
len = length

double :: Num a => a -> a
double x = x * 2
```

When `add(1, "hello")` is written, GHC reports:
```
No instance for (Num String) arising from a use of 'add'
```

### What this proposal achieves

1. **Compile-time type safety**: Ill-typed programs are rejected before execution
2. **Better error messages**: Type errors point to the exact mismatch, not "runtime type error"
3. **Enables 0119 (typed codegen)**: Once every expression has a known type, unboxed code generation becomes straightforward
4. **Enables optimizations**: Dead branch elimination, specialization, unboxed ADT fields — all require static types
5. **Developer confidence**: "If it compiles, it works" — the Haskell promise

---

## Guide-level explanation

### For Flux users

After full static typing, the compiler catches type errors:

```flux
fn add(x: Int, y: Int): Int { x + y }

fn main() with IO {
    print(add(1, "hello"))
    //         ^^^^^^^ error: expected Int, found String
}
```

Polymorphic functions use type variables:

```flux
fn identity<a>(x: a): a { x }

fn first<a>(arr: Array<a>): Option<a> {
    arr[0]
}
```

Type classes enable overloaded operations:

```flux
class Eq<a> {
    fn eq(x: a, y: a): Bool
    fn neq(x: a, y: a): Bool { !eq(x, y) }
}

instance Eq<Int> {
    fn eq(x, y) { x == y }
}

instance Eq<String> {
    fn eq(x, y) { x == y }
}

// Now == works on Int and String but is rejected for unknown types
fn contains<a: Eq>(arr: Array<a>, elem: a): Bool {
    any(arr, \x -> eq(x, elem))
}
```

### For compiler contributors

The type system pipeline becomes:

```
Source (.flx)
  → Parser (syntax/)              AST with optional type annotations
  → HM Inference (ast/type_infer/) Algorithm W, NO Any fallback
  → Type Class Resolution          resolve constraints, find instances
  → Typed Core IR (core/)          every binder and expression has a FluxRep
  → Aether (aether/)              type-aware dup/drop (skip for IntRep/FloatRep)
  → core_to_llvm / VM             type-directed code generation
```

---

## Reference-level explanation

### Phase 1 — Eliminate Any from HM inference

**Goal**: When HM inference can't resolve a type, emit an error instead of falling back to `Any`.

**Changes**:

1. Add `--strict-types` compiler flag (off by default initially)
2. When `--strict-types` is active, `InferType::Con(TypeConstructor::Any)` is never produced by fresh variable resolution
3. Unresolved type variables at generalization time produce an error:
   ```
   error[E500]: Could not infer type
     |
   3 | fn add(x, y) { x + y }
     |        ^ type of `x` could not be determined
     |
     help: add a type annotation: fn add(x: Int, y: Int)
   ```
4. Maintain backward compatibility: without `--strict-types`, existing behavior is preserved

**Files**: `src/ast/type_infer/mod.rs`, `src/types/infer_type.rs`, `src/main.rs`

**Prerequisite**: Proposal 0120 Phase 4 (done — primops have type schemes that HM can use)

### Phase 2 — Type annotations on all public APIs

**Goal**: Require type annotations on all public function signatures.

```flux
module Base.List {
    // Required: public functions must be annotated
    public fn map<a, b>(arr: Array<a>, f: (a) -> b): Array<b> { ... }
    public fn filter<a>(arr: Array<a>, pred: (a) -> Bool): Array<a> { ... }

    // Private functions can still be inferred
    fn go(i, acc) { ... }
}
```

**Changes**:

1. `--strict-types` requires type annotations on `public fn` signatures
2. Private/local functions can still be inferred
3. Update `lib/Base/*.flx` with full type annotations
4. Module interface files (`.flxi`) always contain complete type information

**Files**: `lib/Base/*.flx`, `src/bytecode/compiler/statement.rs`

### Phase 3 — Type classes (core feature)

**Goal**: Replace `Any`-typed operators with type class constraints.

**Design** (following Haskell):

```rust
// New AST node
pub enum Statement {
    // ...
    Class {
        name: Identifier,
        type_params: Vec<Identifier>,
        superclasses: Vec<Constraint>,
        methods: Vec<ClassMethod>,
        span: Span,
    },
    Instance {
        class_name: Identifier,
        type_args: Vec<TypeExpr>,
        methods: Vec<InstanceMethod>,
        span: Span,
    },
}

// New InferType variant for constrained polymorphism
pub enum InferType {
    // ... existing variants ...
    /// Constrained type variable: `a` where `Eq a`
    Constrained(TypeVarId, Vec<Constraint>),
}
```

**Built-in type classes**:

| Class | Methods | Instances |
|-------|---------|-----------|
| `Eq<a>` | `eq : (a, a) -> Bool` | Int, Float, String, Bool, Array<a: Eq>, Option<a: Eq> |
| `Ord<a>` | `lt, gt, lte, gte : (a, a) -> Bool` | Int, Float, String |
| `Num<a>` | `add, sub, mul : (a, a) -> a` | Int, Float |
| `Show<a>` | `show : a -> String` | Int, Float, String, Bool, Array<a: Show> |
| `Functor<f>` | `fmap : ((a) -> b, f<a>) -> f<b>` | Array, Option, List |

**Impact on primops**:

```
// Before (gradual):
add : Any -> Any -> Any

// After (type classes):
add : Num a => a -> a -> a
```

**Files**: `src/syntax/statement.rs`, `src/ast/type_infer/`, new `src/types/type_class.rs`

### Phase 4 — Type class resolution (constraint solver)

**Goal**: Resolve type class constraints during inference.

When HM inference encounters `x + y`, it generates a constraint `Num a` on the type variable. The constraint solver:

1. Collects all constraints during inference
2. After inference, resolves each constraint by finding an instance
3. If no instance exists, reports a type error:
   ```
   error[E510]: No instance for Num String
     |
   5 | let x = "hello" + "world"
     |                 ^ String does not implement Num
     |
     help: use string concatenation: "hello" ++ "world"
   ```

**Implementation** (following GHC's approach):

1. **Constraint generation**: Each overloaded operation generates a constraint
2. **Constraint simplification**: Reduce constraints using superclass relationships
3. **Instance resolution**: Match constraints to declared instances
4. **Dictionary passing**: Translate type class calls to explicit dictionary arguments in Core IR

**Files**: new `src/types/constraint.rs`, `src/types/instance.rs`, `src/core/dictionary.rs`

### Phase 5 — Higher-kinded types

**Goal**: Support type constructors as type parameters.

Required for `Functor`, `Monad`, and other higher-kinded type classes:

```flux
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b): f<b>
}

instance Functor<Array> {
    fn fmap(arr, f) { map(arr, f) }
}

instance Functor<Option> {
    fn fmap(opt, f) { map_option(opt, f) }
}
```

**Changes**:

1. Add kind system: `*`, `* -> *`, `* -> * -> *`
2. Type constructors (`Array`, `Option`, `List`) have kinds
3. Type class parameters can be higher-kinded
4. Kind inference during type checking

**Files**: new `src/types/kind.rs`, updates to `src/types/type_constructor.rs`

### Phase 6 — Typed Core IR (Proposal 0119)

**Goal**: Carry type information through Core IR for type-directed codegen.

With full static types, every Core IR expression has a known `FluxRep`. This enables:
- Unboxed arithmetic (Proposal 0119)
- Type-directed Aether (skip RC for primitives)
- Optimized ADT layouts (unboxed fields)

This phase is exactly Proposal 0119 — it becomes straightforward once Phases 1-5 establish complete type information.

---

## Migration path

### Backward compatibility

The transition is opt-in per compilation unit:

1. **Current behavior** (default): Gradual typing with `Any` fallback
2. **`--strict-types`**: Full static typing, errors on unresolved types
3. **Future default**: `--strict-types` becomes the default; `--gradual` flag for legacy code

### lib/Base/ migration

`lib/Base/*.flx` must be fully annotated first (Phase 2). This is the forcing function — once the stdlib is typed, user code that calls it inherits types naturally.

### Timeline

| Phase | Feature | Depends on | Effort |
|-------|---------|------------|--------|
| 1 | Eliminate Any fallback | 0120 Phase 4 (done) | 1 week |
| 2 | Public API annotations | Phase 1 | 1 week |
| 3 | Type classes | Phase 1 | 3 weeks |
| 4 | Constraint solver | Phase 3 | 2 weeks |
| 5 | Higher-kinded types | Phase 4 | 2 weeks |
| 6 | Typed Core IR (0119) | Phase 1-2 | 2 weeks |

Phases 1-2 give immediate value (catch type errors). Phases 3-5 are the big investment (type classes). Phase 6 unlocks performance.

---

## Drawbacks

- **Breaking change**: Existing untyped Flux code won't compile under `--strict-types`. Mitigated by opt-in flag and gradual migration.

- **Annotation burden**: Users must write type annotations on public APIs. Mitigated by HM inference handling local/private code.

- **Type class complexity**: Type classes add significant compiler complexity (constraint solver, instance resolution, dictionary passing). This is the hardest part of the proposal — GHC's constraint solver is ~30K lines.

- **Higher-kinded types**: HKTs make the type system significantly more complex. Can be deferred — basic type classes work without HKTs (just no `Functor`/`Monad`).

- **Slower compilation**: Constraint resolution adds time to the type checking phase. Mitigated by caching instance dictionaries.

---

## Rationale and alternatives

### Why follow Haskell rather than TypeScript/Python?

TypeScript and Python use gradual typing — mixing typed and untyped code. This is pragmatic but provides weaker guarantees. Flux already has HM inference and algebraic effects — it's closer to Haskell/Koka than TypeScript. Full static typing is the natural evolution.

### Alternative: Rust-style traits instead of type classes

Rust traits and Haskell type classes are similar. The key difference: Rust traits use monomorphization (code duplication); Haskell uses dictionary passing (single code, runtime dispatch). For Flux, dictionary passing is simpler to implement and doesn't cause code bloat. Future optimization: monomorphize hot paths.

### Alternative: Row polymorphism for overloading

Instead of type classes, use row types to express "any type with an `add` method." This is how OCaml handles some overloading. Flux already has effect rows — extending to value rows is possible but less established than type classes.

### Alternative: Keep gradual typing, improve error messages

Don't make the type system stricter; instead, improve runtime error messages with source locations and expected types. This is the minimal approach but doesn't achieve compile-time safety.

---

## Prior art

### GHC (Haskell)

The gold standard for type classes. Key papers:
- "How to make ad-hoc polymorphism less ad hoc" (Wadler & Blott, 1989) — type classes
- "Type classes in Haskell" (Hall et al., 1996) — implementation
- "A theory of overloading" (Stuckey & Sulzmann, 2005) — constraint solving
- "OutsideIn(X)" (Vytiniotis et al., 2011) — GHC's current constraint solver

GHC compiles type classes to dictionaries in Core:
```haskell
-- Source:     (+) :: Num a => a -> a -> a
-- Core:       plus :: NumDict a -> a -> a -> a
-- Dictionary: data NumDict a = NumDict { plus :: a -> a -> a, ... }
```

### Koka

Koka uses effect types and row polymorphism. It doesn't have type classes but uses overloaded names with explicit type dispatch. This is simpler but less expressive.

### Lean 4

Lean uses type classes extensively (called "instances"). Its elaborator handles constraint resolution and instance search, similar to GHC but in a dependently-typed setting.

### PureScript

PureScript is Haskell-like with type classes, compiling to JavaScript. Its type class implementation is a good reference for a simpler-than-GHC approach — no superclass defaults, no overlapping instances.

---

## Unresolved questions

- **Should `Int` and `Float` be separate types or unified as `Num`?** Currently, `1 + 2.0` works via runtime coercion. With type classes, this either requires `Num` instances for both or explicit conversion (`toFloat(1) + 2.0`). Haskell uses `fromInteger` and `Num` to handle literals polymorphically.

- **How to handle `len`?** `len` works on String, Array, List, Map. With type classes, it needs a `HasLength` class. Alternatively, make it a method on `Foldable`. Or keep it as a compiler-recognized function with special typing rules.

- **How to handle `==`?** Currently `==` works on any two values via runtime dispatch. With type classes, it needs `Eq`. Should `==` be syntactic sugar for `Eq.eq`? What about heterogeneous equality (`1 == "1"` — currently false, should it be a type error)?

- **Dictionary passing vs monomorphization?** Dictionary passing is simpler to implement but has runtime overhead (indirect calls). Monomorphization eliminates overhead but causes code bloat. GHC defaults to dictionaries with opportunistic specialization via `SPECIALIZE` pragmas. Flux could start with dictionaries and add specialization later.

- **How much of Haskell's type system to adopt?** Full GHC has: type families, GADTs, existential types, rank-N types, kind polymorphism, type-level computation. Each adds complexity. The minimal viable set for "Haskell-like" is: HM + type classes + basic HKTs. Everything else is optional.

---

## Future possibilities

- **Deriving**: Auto-derive `Eq`, `Ord`, `Show` for ADTs (like Haskell's `deriving`)
- **Type families**: Type-level functions for advanced type-level programming
- **GADTs**: Generalized algebraic data types for indexed types
- **Existential types**: `exists a. (a, a -> String)` for type erasure
- **Type inference for type classes**: Infer which type class instances are needed without explicit annotations (GHC already does this)
- **Monomorphization pass**: Specialize frequently-used polymorphic functions for known types
- **Typed effects**: Combine type classes with algebraic effects for typed handlers
