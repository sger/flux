- Feature Name: Full Static Typing — From Gradual to Haskell-Like Type Safety
- Start Date: 2026-03-25
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0119 (Typed LLVM Codegen), Proposal 0120 (Unified Base Library)

## Summary

Transition Flux from a gradually-typed language (where unannotated code infers as `Any`) to a fully statically-typed language where every expression has a concrete type at compile time and ill-typed programs are rejected before execution. This follows the Haskell model: the compiler proves type safety, runtime type errors become impossible.

The transition is incremental — each phase adds type system features while maintaining backward compatibility through a `--strict-types` flag that becomes the default in a future release.

---

## Implementation status

Last updated: 2026-04-07 (Phase 7 complete)

### Completed

| Phase | Feature | Status | Notes |
|-------|---------|--------|-------|
| **1** | Eliminate `Any` fallback (`--strict-types`) | **Done** | New flag, post-inference validation pass (`strict_types.rs`), error code E430. Rejects any binding whose inferred type `contains_any()`. Disabled for Flow library. |
| **2** | Public API annotations (`--strict`) | **Already existed** | E416 (params), E417 (return type), E418 (effects), E423 (Any in annotations). |
| **—** | Typed primop returns | **Done** | `print`/`println` return `Unit` (was `Any`). All primop params polymorphic with type variables (was `Any`). Operators preserve type vars instead of collapsing to `Any`. |
| **3** | Type classes (syntax + AST) | **Done (MVP)** | See Proposal 0145. Parser, ClassEnv, runtime dispatch for single-instance. Constraint solver + dictionaries remain. |
| **6** | Deriving (`Eq`, `Ord`, `Show`, `Semigroup`) | **Done** | Auto-derive type class instances for ADTs. |
| **7a** | Typed Core IR — binder infrastructure | **Done** | `FluxRep` enum on `CoreBinder`, `CoreType`, `TypeEnv` threading to AST lowerer, typed function params via `bind_fn_params()`. |
| **7b** | Typed Core IR — lambda param typing | **Done** | Lambda parameters get `FluxRep` from HM-inferred function type via `bind_lambda_params()`. |
| **7c** | Typed Core IR — LIR/LLVM type extraction | **Done** | LIR extracts `param_reps`/`result_rep` from Core binders; LLVM worker/wrapper uses them for unboxed specialization. |
| **7d** | Aether type-directed RC elision | **Done** | `wrap_drop`/`wrap_dups` skip Dup/Drop for binders with `IntRep`/`FloatRep`/`BoolRep` (`!needs_rc()`). |
| **7e** | Typed Core IR — pattern binders | **Done** | `lower_pattern_typed()` threads scrutinee `InferType` from `hm_expr_types` through pattern decomposition. Built-in patterns (Option, Either, List, Tuple) extract inner types; `CorePat::Var` binders get correct `FluxRep`. Eliminates unnecessary Dup/Drop for unboxed pattern vars (e.g. `h` in `[h\|t]` for `List<Int>` is `IntRep`). |
| **7f** | Typed Core IR — effect handler binders | **Done** | `lower_handle_arm_typed()` uses `EffectOpSigs` map threaded into `AstLowerer` to type handler param binders from effect op signatures. Resume binder is always `BoxedRep` (closure). New `lower_program_ast_complete()` entry point accepts `EffectOpSigs`. |
| **7g** | Typed Core IR — ADT field layout metadata | **Done** | `FluxRep::from_type_expr()` converts syntactic field types to reps. `AdtDefinition` and `ConstructorInfo` now store per-constructor `field_reps: Vec<FluxRep>`. LIR `MakeCtor` carries `field_reps`. Infrastructure for future unboxed field storage. |

### Remaining

| Phase | Feature | Status | Blocker |
|-------|---------|--------|---------|
| **3** | Type classes (full) | In progress | Proposal 0145: Steps 1-4, 6 done. Step 5 monomorphic resolution done; polymorphic dictionary threading remaining. |
| **4** | Constraint solver + dictionaries | **Mostly done** | Constraint generation + solving done. Monomorphic compile-time instance resolution done (0145 Step 5). Polymorphic dictionary parameters deferred. |
| **5** | Higher-kinded types | Not started | Phase 4 complete; requires kind system |

### Key files

| File | Purpose |
|------|---------|
| `src/ast/type_infer/strict_types.rs` | Phase 1: `--strict-types` validation pass |
| `src/types/class_env.rs` | Phase 3: ClassEnv — class/instance registry + validation |
| `src/types/class_dispatch.rs` | Phase 3: MVP runtime dispatch — instance method compilation |
| `src/syntax/type_class.rs` | Phase 3: AST types for `ClassConstraint`, `ClassMethod`, `InstanceMethod` |
| `src/diagnostics/compiler_errors.rs` | E430 (strict-types), E440–E443 (type class validation) |
| `docs/proposals/0145_type_classes.md` | Detailed type class proposal with step-by-step tracking |
| `src/core/lower_ast/pattern.rs` | Phase 7e: `lower_pattern_typed()` — typed pattern binder lowering |
| `src/core/lower_ast/mod.rs` | Phase 7f: `EffectOpSigs` type, `lower_program_ast_complete()` entry point |
| `src/bytecode/compiler/adt_registry.rs` | Phase 7g: `register_adt()` populates `field_reps` from `DataVariant` |
| `src/bytecode/compiler/adt_definition.rs` | Phase 7g: `AdtDefinition` with per-constructor `field_reps` |
| `src/bytecode/compiler/constructor_info.rs` | Phase 7g: `ConstructorInfo` with `field_reps: Vec<FluxRep>` |

### What `--strict-types` catches today

```flux
fn add(x, y) { x + y }       // ✓ passes — infers as a -> a -> a (polymorphic, no Any)
fn identity(x) { x }         // ✓ passes — infers as a -> a (polymorphic)
fn bad() { x + "hello" }     // ✗ E300 type mismatch (caught by normal inference)

fn main() with IO {
    print(add(1, 2))          // ✓ passes — print returns Unit, add returns Int
}
```

### What `--strict-types` cannot catch yet

```flux
fn add(x, y) { x + y }       // infers a -> a -> a — but + should require Num<a>
                               // Without type classes, any type is accepted for +
                               // Needs: Phase 3-4 (constraint solver)

fn show_it(x) { show(x) }    // Would need Show<a> constraint
                               // Needs: Phase 3-4 (dictionary passing for polymorphic dispatch)
```

---

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
    fn neq(x: a, y: a): Bool { !eq(x, y) }  // default method
}

instance Eq<Int> {
    fn eq(x, y) { prim_int_eq(x, y) }
}

instance Eq<String> {
    fn eq(x, y) { prim_str_eq(x, y) }
}

// Derived instance for ADTs
data Color { Red, Green, Blue } deriving (Eq, Show)

// Constrained polymorphism
fn contains<a: Eq>(xs: List<a>, elem: a): Bool {
    any(xs, \x -> eq(x, elem))
}
```

### For compiler contributors

The type system pipeline becomes:

```
Source (.flx)
  -> Parser (syntax/)              AST with optional type annotations
  -> HM Inference (ast/type_infer/) Algorithm W + constraint generation
  -> Constraint Solver              resolve class constraints, find instances
  -> Dictionary Elaboration         class calls -> explicit dictionary args in Core
  -> Typed Core IR (core/)          every binder has a FluxRep
  -> Aether (aether/)              type-aware dup/drop (skip for IntRep/FloatRep)
  -> core_to_llvm / VM             type-directed code generation
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

**GHC comparison**: GHC's `TcLevel` tracks nesting depth of type variables. At generalization time (`chooseInferredQuantifiers` in `GHC.Tc.Gen.Bind`), unresolved variables are either generalized (if they escape no constraints) or trigger an "ambiguous type variable" error. Flux should follow the same approach but without GHC's monomorphism restriction (which Flux doesn't need since all bindings are function definitions).

**Files**: `src/ast/type_infer/mod.rs`, `src/types/infer_type.rs`, `src/main.rs`

### Phase 2 — Type annotations on all public APIs

**Goal**: Require type annotations on all public function signatures.

```flux
module Flow.List {
    // Required: public functions must be annotated
    public fn map<a, b>(xs: List<a>, f: (a) -> b): List<b> { ... }
    public fn filter<a>(xs: List<a>, pred: (a) -> Bool): List<a> { ... }

    // Private functions can still be inferred
    fn go(i, acc) { ... }
}
```

**GHC comparison**: Haskell does not require type annotations — GHC's inference is powerful enough to infer nearly everything. But the community convention is to annotate all top-level bindings. GHC warns with `-Wmissing-signatures`. Flux should follow GHC's approach: require annotations on public APIs under `--strict-types`, warn otherwise. Private/local functions are always inferred.

**Files**: `lib/Flow/*.flx`, `src/bytecode/compiler/statement.rs`

### Phase 3 — Type classes (core feature)

**Goal**: Replace `Any`-typed operators with type class constraints.

#### 3a. Syntax and AST

```rust
// New AST nodes
pub enum Statement {
    // ...
    Class {
        name: Identifier,
        type_params: Vec<Identifier>,
        superclasses: Vec<Constraint>,  // e.g., [Eq a] for Ord
        methods: Vec<ClassMethod>,      // method signatures + optional defaults
        span: Span,
    },
    Instance {
        class_name: Identifier,
        type_args: Vec<TypeExpr>,
        context: Vec<Constraint>,       // instance context, e.g., Eq a => Eq (List a)
        methods: Vec<InstanceMethod>,
        span: Span,
    },
    Deriving {
        type_name: Identifier,
        classes: Vec<Identifier>,       // e.g., [Eq, Ord, Show]
        span: Span,
    },
}
```

**GHC comparison**: GHC's `Class` type (in `GHC.Core.Class`) stores:
- `classTyCon` — the dictionary type constructor
- `classTyVars` — class type parameters
- `classFunDeps` — functional dependencies
- `classBody` — superclasses, associated types, method selectors

Flux should start simpler: no associated types, no functional dependencies. Single-parameter type classes first (multi-parameter is Phase 5b).

#### 3b. Built-in type class hierarchy

Following GHC's proven hierarchy, adapted for Flux:

```
Eq
  └── Ord

Num
  └── Fractional

Semigroup
  └── Monoid

Show

Foldable (Phase 5, requires HKTs)
  └── Traversable (Phase 5)

Functor (Phase 5, requires HKTs)
  └── Applicative (Phase 5)
```

**Detailed class definitions following GHC:**

| Class | Superclass | Methods | Minimal Definition | Flux Instances |
|-------|------------|---------|-------------------|----------------|
| `Eq<a>` | — | `eq(a, a) -> Bool`, `neq(a, a) -> Bool` | `eq` | Int, Float, String, Bool, Char, Option<a: Eq>, List<a: Eq>, Array<a: Eq>, Tuple |
| `Ord<a>` | `Eq<a>` | `compare(a, a) -> Ordering`, `lt`, `gt`, `lte`, `gte`, `min`, `max` | `compare \| lte` | Int, Float, String, Char |
| `Num<a>` | — | `add(a, a) -> a`, `sub(a, a) -> a`, `mul(a, a) -> a`, `neg(a) -> a`, `abs(a) -> a`, `from_int(Int) -> a` | `add, mul, neg, abs, from_int` | Int, Float |
| `Fractional<a>` | `Num<a>` | `div(a, a) -> a`, `recip(a) -> a`, `from_float(Float) -> a` | `div, from_float` | Float |
| `Show<a>` | — | `show(a) -> String` | `show` | Int, Float, String, Bool, Char, Option<a: Show>, List<a: Show>, Array<a: Show> |
| `Semigroup<a>` | — | `append(a, a) -> a` | `append` | String, List<a>, Array<a> |
| `Monoid<a>` | `Semigroup<a>` | `empty() -> a` | `empty` | String, List<a>, Array<a> |

**GHC comparison**: GHC's numeric tower is deeper: `Num -> Real -> Integral` and `Num -> Fractional -> Floating -> RealFloat`. Flux should keep it simpler — just `Num` and `Fractional` — since NaN-boxing means Int and Float are the only numeric representations.

**GHC comparison on `Eq`**: GHC's `Eq` has minimal definition `(==) | (/=)`, where each can be defined via the other. GHC derives `Eq` by comparing constructor tags then fields structurally. Flux should follow this for ADTs.

**Operator desugaring**:

| Operator | Desugars to | Class |
|----------|-------------|-------|
| `x + y` | `Num.add(x, y)` | `Num` |
| `x - y` | `Num.sub(x, y)` | `Num` |
| `x * y` | `Num.mul(x, y)` | `Num` |
| `x / y` | `Fractional.div(x, y)` | `Fractional` |
| `x == y` | `Eq.eq(x, y)` | `Eq` |
| `x != y` | `Eq.neq(x, y)` | `Eq` |
| `x < y` | `Ord.lt(x, y)` | `Ord` |
| `x > y` | `Ord.gt(x, y)` | `Ord` |
| `x <= y` | `Ord.lte(x, y)` | `Ord` |
| `x >= y` | `Ord.gte(x, y)` | `Ord` |
| `x ++ y` | `Semigroup.append(x, y)` | `Semigroup` |

**Files**: `src/syntax/statement.rs`, `src/syntax/parser/statement.rs`, `src/ast/type_infer/`

### Phase 4 — Constraint solver and dictionary elaboration

**Goal**: Resolve type class constraints during inference and translate to dictionaries in Core IR.

#### 4a. Constraint generation

When HM inference encounters an overloaded operation, it generates a constraint:

```
infer(x + y):
  fresh a
  unify(typeof(x), a)
  unify(typeof(y), a)
  emit constraint: Num a
  return type: a
```

Constraints are represented as:

```rust
pub enum Constraint {
    ClassConstraint {
        class_name: Identifier,
        type_args: Vec<InferType>,
        span: Span,
    },
}

pub struct WantedConstraints {
    pub simple: Vec<Constraint>,        // direct constraints
    pub implications: Vec<Implication>,  // nested scopes (e.g., inside lambdas)
}
```

**GHC comparison**: GHC's constraint types (`GHC.Tc.Types.Constraint`) are richer:
- `CDictCan` — class dictionary constraints (canonical)
- `CEqCan` — type equality constraints
- `CIrredCan` — irreducible/stuck constraints
- `CQuantCan` — quantified constraints

Flux should start with just `CDictCan` equivalent. Type equalities are already handled by HM unification. Irreducible constraints can be deferred.

#### 4b. Constraint solving

The solver follows GHC's **OutsideIn(X)** approach, simplified:

1. **Canonicalize**: Decompose constraints to head-normal form
2. **Instance matching**: For each `ClassConstraint(C, [tau])`, search for a matching instance
3. **Superclass expansion**: If `Ord a` is wanted and `Eq a` is a superclass, also generate `Eq a`
4. **Simplify**: Remove constraints satisfied by instances
5. **Generalize**: Remaining unsolved constraints become part of the function's type scheme

```
// Example: inferring  fn add(x, y) { x + y }
//
// Step 1: Generate constraints
//   typeof(x) = a, typeof(y) = a, Num a
//
// Step 2: No specific instance yet (a is a variable)
//
// Step 3: Generalize
//   add : forall a. Num a => a -> a -> a
//
// Step 4: At call site  add(1, 2)
//   Instantiate a = Int
//   Solve Num Int -> found instance -> emit NumIntDict
```

**GHC comparison**: GHC's solver (`GHC.Tc.Solver`) is ~16K lines across 10 files. The core loop in `simplify_loop` iterates until a fixed point. GHC tracks "Given" constraints (from context) and "Wanted" constraints (to solve). For Flux, a single-pass solver is sufficient initially — iterate only if superclass expansion adds new constraints.

**GHC comparison on evidence**: GHC compiles type classes to explicit dictionaries in Core:
```haskell
-- Source:     (+) :: Num a => a -> a -> a
-- Core:       plus :: NumDict a -> a -> a -> a
-- Dictionary: data NumDict a = NumDict { plus :: a -> a -> a, ... }
```

Flux should follow the same dictionary-passing translation. This maps cleanly to Core IR:

```rust
// Before (source):
//   fn add<a: Num>(x: a, y: a): a { x + y }
//
// After (Core IR with dictionary passing):
//   fn add(dict_num: NumDict, x: Value, y: Value): Value {
//     dict_num.add(x, y)
//   }
```

**Dictionaries as ADTs**: Each type class becomes an ADT in Core IR:
```rust
// Num dictionary for Int:
let numIntDict = Adt("NumDict", [
    closure(prim_int_add),    // add
    closure(prim_int_sub),    // sub
    closure(prim_int_mul),    // mul
    closure(prim_int_neg),    // neg
    closure(prim_int_abs),    // abs
    closure(int_from_int),    // from_int (identity for Int)
])
```

**Files**: new `src/types/constraint.rs`, `src/types/solver.rs`, `src/core/passes/dictionary.rs`

#### 4c. Instance resolution rules

Following GHC's approach:

1. **Exact match**: `Num Int` matches `instance Num<Int>`
2. **Parametric match**: `Eq (List a)` matches `instance Eq<a> => Eq<List<a>>`
3. **No overlapping instances** (initially): Two instances with overlapping heads are rejected at declaration time
4. **Orphan instance warning**: Instance defined in a module that owns neither the class nor the type

**GHC comparison**: GHC supports `OVERLAPPING`, `OVERLAPPABLE`, `OVERLAPS`, and `INCOHERENT` pragmas for controlling overlap resolution. Flux should NOT support these initially — they are a source of confusing behavior. PureScript also prohibits overlapping instances and is better for it.

#### 4d. Defaulting

When a type variable is constrained but never instantiated to a concrete type:

```flux
fn main() with IO {
    print(1 + 2)  // Num a => a, but which a?
}
```

**GHC's defaulting rules**:
1. The constraint involves only standard classes (Num, Eq, Ord, Show)
2. At least one is a numeric class
3. All classes have an instance for the default type
4. Default type list: `Integer`, then `Double`

**Flux's defaulting rules** (simpler):
1. If all constraints on a variable have instances for `Int`, default to `Int`
2. If all constraints have instances for `Float` but not `Int`, default to `Float`
3. Otherwise, report an "ambiguous type variable" error

**Files**: `src/types/solver.rs` (defaulting pass after main solving)

### Phase 5 — Higher-kinded types and advanced classes

**Goal**: Support type constructors as type parameters (kind `* -> *`).

Required for `Functor`, `Foldable`, and collection-generic functions like `len`.

```flux
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b): f<b>
}

instance Functor<List> {
    fn fmap(xs, f) { map(xs, f) }
}

instance Functor<Array> {
    fn fmap(arr, f) { Array.map(arr, f) }
}

instance Functor<Option> {
    fn fmap(opt, f) { map_option(opt, f) }
}
```

#### 5a. Kind system

```
Kind ::= Type              -- the kind of value types (GHC's `*` or `Type`)
       | Kind -> Kind       -- type constructor kinds
```

| Type constructor | Kind |
|-----------------|------|
| `Int` | `Type` |
| `String` | `Type` |
| `Bool` | `Type` |
| `List` | `Type -> Type` |
| `Array` | `Type -> Type` |
| `Option` | `Type -> Type` |
| `Map` | `Type -> Type -> Type` |
| `(->)` | `Type -> Type -> Type` |

**GHC comparison**: GHC unifies kinds and types — kinds ARE types (via `TypeInType`). The kind of `Type` is `Type` (self-referential). This enables kind polymorphism. Flux should NOT adopt this — keep kinds as a separate, simpler system. Just `Type` and `->` between kinds.

**GHC comparison**: GHC tracks `RuntimeRep` kinds for representation polymorphism (`IntRep`, `FloatRep`, `BoxedRep`, etc.). This is analogous to Flux's `FluxRep`. When Flux adds HKTs, it should ensure that type constructors preserve rep information.

**Files**: new `src/types/kind.rs`, updates to `src/types/type_constructor.rs`

#### 5b. Multi-parameter type classes (optional)

```flux
class Convertible<a, b> {
    fn convert(x: a): b
}

instance Convertible<Int, Float> {
    fn convert(x) { to_float(x) }
}
```

**GHC comparison**: GHC supports multi-parameter type classes with functional dependencies (`class C a b | a -> b`) to resolve ambiguity. Without fundeps, multi-param classes often lead to ambiguous types. Flux should defer this to a later proposal.

#### 5c. Foldable and len

With HKTs, `len` becomes a method on `Foldable`:

```flux
class Foldable<t> {
    fn fold_right<a, b>(xs: t<a>, init: b, f: (a, b) -> b): b
    fn length<a>(xs: t<a>): Int { fold_right(xs, 0, \(_, acc) -> acc + 1) }
    fn to_list<a>(xs: t<a>): List<a> { fold_right(xs, [], \(x, acc) -> [x | acc]) }
}

instance Foldable<List> {
    fn fold_right(xs, init, f) { fold(xs, init, \(acc, x) -> f(x, acc)) }
}

instance Foldable<Array> {
    fn fold_right(arr, init, f) { Array.fold(arr, init, \(acc, x) -> f(x, acc)) }
}

// Now len works on any Foldable:
fn len<t: Foldable, a>(xs: t<a>): Int { Foldable.length(xs) }
```

**GHC comparison**: GHC's `Foldable` (in `GHC.Internal.Data.Foldable`) has 14 methods with defaults, all derivable from `foldMap | foldr`. The `length` function is defined as `foldl' (\c _ -> c + 1) 0`. Flux should follow this but with fewer methods initially.

### Phase 6 — Deriving

**Goal**: Auto-derive type class instances for ADTs.

```flux
data Color { Red, Green, Blue } deriving (Eq, Ord, Show)

data Tree<a> { Leaf(a), Node(Tree<a>, Tree<a>) } deriving (Eq, Show)
```

**GHC comparison**: GHC supports deriving for: `Eq`, `Ord`, `Enum`, `Bounded`, `Show`, `Read`, `Functor`, `Foldable`, `Traversable`, `Generic`, `Data`, `Lift`. GHC also supports `deriving via` (derive using a newtype wrapper) and `anyclass` deriving (use default methods).

**Flux deriving** (initial set):

| Class | Strategy |
|-------|----------|
| `Eq` | Compare constructor tags, then structural equality on fields |
| `Ord` | Constructor order (declaration order), then structural comparison on fields |
| `Show` | `ConstructorName` for unit, `ConstructorName(field1, field2)` for others |

**Generated code for `Eq` on an ADT**:
```flux
// data Color { Red, Green, Blue } deriving (Eq)
// generates:
instance Eq<Color> {
    fn eq(x, y) {
        match (x, y) {
            (Red, Red) -> true,
            (Green, Green) -> true,
            (Blue, Blue) -> true,
            _ -> false
        }
    }
}
```

**Generated code for `Eq` on a parametric ADT**:
```flux
// data Tree<a> { Leaf(a), Node(Tree<a>, Tree<a>) } deriving (Eq)
// generates (requires Eq<a> constraint):
instance Eq<a> => Eq<Tree<a>> {
    fn eq(x, y) {
        match (x, y) {
            (Leaf(a), Leaf(b)) -> eq(a, b),
            (Node(l1, r1), Node(l2, r2)) -> eq(l1, l2) && eq(r1, r2),
            _ -> false
        }
    }
}
```

**GHC comparison**: GHC's derive generator (`GHC.Tc.Deriv.Generate`) infers constraints from field types. For `data T a = MkT a [a]`, deriving `Eq` infers `(Eq a)` from the `a` and `[a]` fields. Flux should follow the same field-type-based inference.

**Files**: new `src/syntax/deriving.rs`, `src/core/passes/deriving.rs`

### Phase 7 — Typed Core IR (Proposal 0119)

**Goal**: Carry type information through Core IR for type-directed codegen.

With full static types, every Core IR expression has a known `FluxRep`. This enables:
- Unboxed arithmetic (Proposal 0119)
- Type-directed Aether (skip RC for primitives)
- Optimized ADT layouts (unboxed fields)

This phase is exactly Proposal 0119 — it becomes straightforward once Phases 1-6 establish complete type information.

---

## GHC architecture comparison

### What Flux should adopt from GHC

| GHC Feature | Flux Adoption | Rationale |
|-------------|--------------|-----------|
| Algorithm W + bidirectional checking | Extend existing HM with constraint generation | Flux already has Algorithm W; add constraints for class dispatch |
| Dictionary-passing translation | Yes, in Core IR | Maps cleanly to Flux's existing closure/ADT representation |
| Constraint-based inference | Yes, simplified | Generate constraints during HM, solve after inference |
| Superclass hierarchy | Yes (Eq < Ord, Num < Fractional, Semigroup < Monoid) | Proven design; enables default methods |
| Deriving mechanism | Yes, for Eq/Ord/Show initially | Huge ergonomic win for ADTs |
| Defaulting rules | Yes, simplified (Int then Float) | Prevents ambiguous type errors for numeric literals |
| Type class coherence (no overlapping) | Yes, strict coherence | Following PureScript's design; simpler and safer |

### What Flux should NOT adopt from GHC

| GHC Feature | Skip | Rationale |
|-------------|------|-----------|
| Type families | Skip | Huge complexity; not needed for core use cases |
| GADTs | Skip initially | Useful but adds constraint solving complexity |
| Rank-N types | Skip | Quick Look impredicativity is ~3K lines in GHC; not worth the complexity initially |
| Kind polymorphism / TypeInType | Skip | Keep kinds as a separate simple system |
| Functional dependencies | Skip | Multi-param type classes are deferred |
| Overlapping instances | Skip | Source of confusion; PureScript proves they're not needed |
| Deferred type errors | Skip | Flux aims for strict type safety, not gradual |
| Coercion evidence | Skip | GHC's coercion system is ~5K lines; Flux doesn't need newtype coercions |
| Monomorphism restriction | Skip | Historical artifact; modern Haskell disables it |
| Rebindable syntax | Skip | Not needed |

### Key GHC data structures to replicate in Flux

**GHC's `Type` (in `GHC.Core.TyCo.Rep`)**:
```
Type = TyVarTy Var | AppTy Type Type | TyConApp TyCon [Type]
     | ForAllTy Binder Type | FunTy Flag Mult Type Type | LitTy TyLit
```

**Flux equivalent** (extend existing `InferType`):
```rust
pub enum FluxType {
    Var(TypeVarId),
    Con(TypeConstructor),
    App(Box<FluxType>, Box<FluxType>),
    Fun(Box<FluxType>, Box<FluxType>, EffectRow),  // includes effect
    ForAll(TypeVarId, Box<FluxType>),
    Constrained(Vec<Constraint>, Box<FluxType>),    // NEW: qualified type
}
```

**GHC's `Class` (in `GHC.Core.Class`)**:
```
Class = Class { className, classTyCon, classTyVars, classSCTheta,
                classATItems, classOpItems, classMinimalDef }
```

**Flux equivalent**:
```rust
pub struct FluxClass {
    pub name: Identifier,
    pub type_params: Vec<TypeVarId>,
    pub superclasses: Vec<Constraint>,
    pub methods: Vec<ClassMethod>,
    pub minimal_def: Vec<Identifier>,       // methods that must be implemented
    pub default_methods: Vec<DefaultMethod>, // methods with default implementations
}
```

---

## Migration path

### Backward compatibility

The transition is opt-in per compilation unit:

1. **Current behavior** (default): Gradual typing with `Any` fallback
2. **`--strict-types`**: Full static typing, errors on unresolved types
3. **Future default**: `--strict-types` becomes the default; `--gradual` flag for legacy code

### lib/Flow/ migration

`lib/Flow/*.flx` must be fully annotated first (Phase 2). This is the forcing function — once the stdlib is typed, user code that calls it inherits types naturally.

### Timeline

| Phase | Feature | Depends on | GHC Reference | Status |
|-------|---------|------------|---------------|--------|
| 1 | Eliminate Any fallback | 0120 Phase 4 (done) | `GHC.Tc.Gen.Bind` generalization | **Done** |
| 2 | Public API annotations | Phase 1 | `-Wmissing-signatures` | **Done** (pre-existing) |
| — | Typed primop returns | Phase 1 | — | **Done** |
| 3 | Type classes (syntax + AST) | Phase 1 | `GHC.Core.Class`, `GHC.Tc.TyCl` | **MVP done** (Proposal 0145) |
| 4 | Constraint solver + dictionaries | Phase 3 + **Proposal 0145 Steps 3–5** | `GHC.Tc.Solver` (simplified) | Not started |
| 5 | Higher-kinded types | Phase 4 | `GHC.Tc.Gen.HsType` kind checking | Not started |
| 6 | Deriving | Phase 3-4 | `GHC.Tc.Deriv.Generate` | Not started |
| 7 | Typed Core IR (0119) | Phase 1-2 | `GHC.Core` typed expressions | Not started |

Phases 1-2 give immediate value (catch type errors). Phases 3-4 are the big investment (type classes + solver). Phase 5-6 unlock expressiveness. Phase 7 unlocks performance.

---

## Drawbacks

- **Breaking change**: Existing untyped Flux code won't compile under `--strict-types`. Mitigated by opt-in flag and gradual migration.

- **Annotation burden**: Users must write type annotations on public APIs. Mitigated by HM inference handling local/private code.

- **Type class complexity**: Type classes add significant compiler complexity. GHC's constraint solver (`GHC.Tc.Solver`) is ~16K lines across 10 files. However, Flux's simplified version (no type families, no GADTs, no overlapping instances) should be ~2-3K lines — closer to PureScript's implementation.

- **Higher-kinded types**: HKTs make the type system significantly more complex. Can be deferred — basic type classes work without HKTs (just no `Functor`/`Foldable`).

- **Slower compilation**: Constraint resolution adds time to the type checking phase. Mitigated by: (1) Flux programs are small compared to Haskell, (2) no type family reduction, (3) no coercion generation.

---

## Rationale and alternatives

### Why follow Haskell rather than TypeScript/Python?

TypeScript and Python use gradual typing — mixing typed and untyped code. This is pragmatic but provides weaker guarantees. Flux already has HM inference and algebraic effects — it's closer to Haskell/Koka than TypeScript. Full static typing is the natural evolution.

### Why dictionary passing rather than monomorphization?

Rust traits use monomorphization (code duplication at each call site). GHC uses dictionary passing (single polymorphic code, dictionary argument at runtime). For Flux:

- **Dictionary passing is simpler**: One compiled function, one closure. No need for a specialization pass.
- **Dictionary passing works with separate compilation**: Modules can be compiled independently.
- **Code size**: No duplication for each type instantiation.
- **Runtime cost**: One extra argument per polymorphic call. GHC's `SPECIALIZE` pragma can eliminate this for hot paths.

Flux should start with dictionaries. A future monomorphization/specialization pass can eliminate overhead for frequently-used instances (similar to GHC's `SpecConstr` and `SPECIALIZE` optimizations).

### Alternative: Row polymorphism for overloading

Instead of type classes, use row types to express "any type with an `add` method." This is how OCaml handles some overloading. Flux already has effect rows — extending to value rows is possible but less established than type classes. Type classes have 35+ years of research and tooling.

### Alternative: Keep gradual typing, improve error messages

Don't make the type system stricter; instead, improve runtime error messages with source locations and expected types. This is the minimal approach but doesn't achieve compile-time safety.

---

## Prior art

### GHC (Haskell)

The gold standard for type classes. Key papers:
- "How to make ad-hoc polymorphism less ad hoc" (Wadler & Blott, 1989) — type classes
- "Type classes in Haskell" (Hall et al., 1996) — implementation
- "OutsideIn(X)" (Vytiniotis et al., 2011) — GHC's current constraint solver

GHC's type system implementation:
- **Type checker**: `compiler/GHC/Tc/` (~2M lines total). Main inference in `GHC.Tc.Gen.App` (bidirectional + Quick Look).
- **Constraint solver**: `compiler/GHC/Tc/Solver/` (10 files, ~850K lines). Inert-set-based iterative solver.
- **Class representation**: `compiler/GHC/Core/Class.hs`. Classes become TyCons (dictionary constructors).
- **Instance resolution**: `compiler/GHC/Tc/Solver/Dict.hs`. `tryInstances` matches top-level instances.
- **Evidence**: Type class usage compiles to explicit dictionary arguments in Core (`compiler/GHC/Core/`).
- **Deriving**: `compiler/GHC/Tc/Deriv/Generate.hs`. Stock deriving for Eq, Ord, Show, Read, Enum, Bounded, Functor, Foldable, Traversable.
- **Numeric tower**: `Num -> Real -> Integral`, `Num -> Fractional -> Floating -> RealFloat`. Seven methods for `Num` alone.
- **Prelude type classes**: 15+ classes auto-imported. Core hierarchy: Eq/Ord, Num hierarchy, Functor/Applicative/Monad, Foldable/Traversable, Semigroup/Monoid, Show/Read.

### PureScript

PureScript is Haskell-like with type classes, compiling to JavaScript. Its type class implementation is a good reference for a simpler-than-GHC approach:
- No overlapping instances (strict coherence)
- No superclass defaults
- Explicit dictionary passing in generated code
- Simpler constraint solver (~3K lines vs GHC's ~850K)

### Koka

Koka uses effect types and row polymorphism. It doesn't have type classes but uses overloaded names with explicit type dispatch. This is simpler but less expressive.

### Lean 4

Lean uses type classes extensively (called "instances"). Its elaborator handles constraint resolution and instance search, similar to GHC but in a dependently-typed setting.

---

## Unresolved questions

- **Should `Int` and `Float` be separate types or unified as `Num`?** Currently, `1 + 2.0` works via runtime coercion. With type classes, this either requires `Num` instances for both or explicit conversion (`to_float(1) + 2.0`). GHC uses `fromInteger :: Num a => Integer -> a` to handle numeric literals polymorphically — the literal `1` has type `Num a => a`, not `Int`. Flux should follow GHC: numeric literals are polymorphic, resolved by context or defaulting.

- **How to handle `len`?** GHC puts `length` on `Foldable` (requires HKTs). Before Phase 5, Flux could use a simpler `HasLength` class: `class HasLength<a> { fn len(x: a): Int }` with instances for String, Array, List, Map. After Phase 5, migrate to `Foldable`.

- **How to handle `==`?** GHC desugars `==` to `Eq.eq`. Heterogeneous equality (`1 == "1"`) becomes a type error. Flux should follow: `==` desugars to `Eq.eq`, and `1 == "hello"` is rejected with `No instance for Eq where Int ~ String`.

- **How to handle `print`/`to_string`?** GHC's `show` is a method on `Show`. Flux's `to_string` should become `Show.show`. `print` should require `Show a => a -> () with IO`.

- **Dictionary passing vs monomorphization?** Start with dictionaries (simpler, no code bloat). Add `@specialize` pragma later for hot paths. GHC's experience shows this is the right order.

- **How much of GHC's kind system?** Just `Type` and `Type -> Type` (and `Type -> Type -> Type`). No kind polymorphism, no promoted types, no `TypeInType`. This is sufficient for Functor/Foldable.

---

## Future possibilities

- **Deriving via**: `data Wrapper = Wrapper Int deriving (Show) via Int` — derive using an existing instance through a newtype
- **Type families**: Type-level functions for advanced type-level programming
- **GADTs**: Generalized algebraic data types for indexed types
- **Existential types**: `exists a. (a, a -> String)` for type erasure
- **Specialization pragma**: `@specialize fn sort<Int>` to monomorphize hot polymorphic functions
- **Typed effects**: Combine type classes with algebraic effects — `class MonadIO<m> { fn liftIO<a>(io: IO<a>): m<a> }` becomes `perform IO.liftIO(action)` with effect handlers
- **Numeric literal polymorphism**: `1` has type `Num a => a` (like Haskell), enabling `1 :: Float` without explicit conversion
- **Default implementations with superclasses**: `class Ord<a> extends Eq<a> { ... }` where `neq` gets a default from `not(eq(x, y))`
