- Feature Name: Type Inference — GHC Parity Roadmap
- Start Date: 2026-03-26
- Status: Mostly complete (5 of 7 phases done)
- Last Updated: 2026-04-08
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0123 (Full Static Typing) ✅

## Implementation status

| Phase | Feature | Status | Implemented via |
|-------|---------|--------|-----------------|
| **1** | Constraint generation | **Done** | `emit_class_constraint`, `WantedClassConstraint`, `Constraint::Class` in `ast/type_infer/` (Proposal 0145 Step 3) |
| **2** | Constraint solver | **Done** | `class_solver.rs`, E444 for unsatisfied constraints (Proposal 0145 Step 4) |
| **3** | Evidence elaboration (dictionaries) | **Done** | `dict_elaborate.rs` Core-to-Core pass, `__dict_*` CoreDefs, body rewriting, call-site resolution (Proposal 0145 Step 5b) |
| **4** | Defaulting | **Open** | Num defaulting to Int not implemented. Low priority. |
| **5** | Generalization with constraints | **Done** | `Scheme.constraints: Vec<SchemeConstraint>`, `collect_scheme_constraints`, `generalize_with_constraints` (Proposal 0145 Step 5b) |
| **6** | Bidirectional type checking | **Open** | Optional/deferred. Pure inference sufficient for current use cases. |
| **7** | Kind system | **Done** | `src/types/kind.rs`, `Kind::Type`/`Kind::Arrow`, HKT unification, `Functor<List>` works end-to-end (Proposal 0145 HKTs) |

**What was listed as missing in the original proposal and is now done:**
- Type classes: `class`/`instance` with full dispatch (0145)
- Constraint solver: concrete constraints checked, variable constraints promoted to schemes
- Dictionary elaboration: Core-to-Core pass with MakeTuple dictionaries and TupleField extraction
- Kind system: `Type` and `->` kinds with HKT application
- `Any` fallback: eliminated by `--strict-types` (0123)
- Superclass expansion: parsing + enforcement (E445)
- Constrained schemes: `Scheme.constraints` field with generalization

## Summary

Close the gap between Flux's current Algorithm W type inference and GHC's production-grade inference engine. This proposal identifies the specific architectural differences, prioritizes which GHC features to adopt, and defines concrete implementation phases. The goal is not to replicate GHC's 2M-line type checker but to adopt the minimal set of features that unlock Haskell-grade type safety for Flux.

## Motivation

### Current state of Flux inference

Flux implements classic Algorithm W (Hindley-Milner) with extensions for effect rows and gradual typing. The implementation lives in `src/ast/type_infer/` (~2.5K lines) and `src/types/` (~2K lines).

**What works well:**
- Let-polymorphism with level-based generalization
- Row-polymorphic effect types (unique to Flux, not in GHC)
- ADT constructor inference with pattern matching refinement
- Lazy substitution with occurs check
- Shadow-stack scoped type environment

**What's missing compared to GHC:**
- No type classes (operators typed as `Any -> Any -> Any`)
- No constraint solver (constraints are solved eagerly, never deferred)
- No dictionary elaboration (no evidence passing in Core IR)
- No bidirectional type checking (inference only, no checking mode)
- No skolemisation (no rigid type variables for signature checking)
- No kind system (type constructors not kinded)
- `Any` fallback masks real type errors

### Why this matters

Without these features, Flux cannot:
1. Type `+` as `Num a => a -> a -> a` (needs type classes + constraints)
2. Type `==` as `Eq a => a -> a -> Bool` (needs type classes)
3. Type `len` polymorphically across collections (needs Foldable + HKTs)
4. Reject `1 + "hello"` at compile time (needs constraint solving)
5. Auto-derive `Eq`, `Show` for ADTs (needs deriving mechanism)
6. Generate unboxed code for known types (needs typed Core IR)

### Scope

This proposal covers the inference engine changes only. Type class syntax, standard library annotations, and typed Core IR are covered by Proposal 0123. This proposal is the implementation companion — it specifies **how** to build each feature by referencing GHC's architecture.

## Reference-level explanation

### Architecture comparison

```
Flux (current):
  Source -> Parser -> HM Inference (Algorithm W) -> Untyped Core -> VM/LLVM
                      ↑ eager unification
                      ↑ Any fallback on failure
                      ↑ no constraints

GHC:
  Source -> Parser -> Renamer -> Type Checker -> Typed Core -> STG -> Cmm -> Asm
                                 ↑ bidirectional (check + infer)
                                 ↑ constraint generation
                                 ↑ constraint solving (OutsideIn(X))
                                 ↑ evidence elaboration (dictionaries)
                                 ↑ zonking (resolve all meta-vars)

Flux (target):
  Source -> Parser -> HM Inference + Constraint Generation -> Constraint Solver
           -> Dictionary Elaboration -> Typed Core -> Aether -> VM/LLVM
```

### Phase 1 — Constraint generation during inference

**Current Flux**: Unification is eager. When inferring `x + y`, the operator type rule immediately unifies operands and returns the result type.

**Target**: Defer overloaded operations to constraints. When inferring `x + y`:
1. Allocate fresh type variable `a`
2. Unify `typeof(x)` with `a`, unify `typeof(y)` with `a`
3. Emit constraint `Num a` (instead of immediately checking if `a` is Int/Float)
4. Return type `a`

**Implementation:**

Extend `InferCtx` with a constraint accumulator:

```rust
// src/ast/type_infer/mod.rs
struct InferCtx<'a> {
    // ... existing fields ...
    wanted_constraints: Vec<WantedConstraint>,  // NEW
}

// src/types/constraint.rs (new file)
pub enum WantedConstraint {
    ClassConstraint {
        class: ClassId,
        type_args: Vec<InferType>,
        evidence_var: EvidenceVarId,  // where to put the dictionary
        origin: ConstraintOrigin,
        span: Span,
    },
}

pub enum ConstraintOrigin {
    BinOp(BinOp),           // x + y generated Num a
    Comparison(BinOp),      // x == y generated Eq a
    FunctionCall(Identifier), // len(xs) generated Foldable t
    Literal(LiteralKind),   // 42 generated Num a (polymorphic literal)
    UserAnnotation,         // explicit constraint in signature
}
```

**GHC reference**: `GHC.Tc.Types.Constraint` defines `Ct` (constraint) with variants `CDictCan` (class dictionary), `CEqCan` (type equality), `CIrredCan` (irreducible). Flux only needs the dictionary variant initially — type equalities are already handled by eager unification.

**Operator desugaring change:**

```rust
// Before (src/ast/type_infer/expression/operators.rs):
fn infer_add(left, right) {
    let lt = infer(left);
    let rt = infer(right);
    unify(lt, rt);  // immediate
    lt               // return unified type
}

// After:
fn infer_add(left, right) {
    let a = fresh_var();
    let lt = infer(left);
    let rt = infer(right);
    unify(lt, a);
    unify(rt, a);
    emit_constraint(ClassConstraint { class: Num, type_args: [a], ... });
    a  // return constrained variable
}
```

**Files**: `src/ast/type_infer/expression/operators.rs`, new `src/types/constraint.rs`

### Phase 2 — Constraint solver

**Current Flux**: `src/ast/type_infer/solver.rs` is a 52-line stub (`solve_deferred_constraints` does nothing).

**Target**: A real constraint solver that runs after inference, resolving class constraints to specific instances.

**Algorithm** (simplified OutsideIn):

```
solve(wanted_constraints, instance_env):
    for each WantedConstraint(class, [tau]):
        // Step 1: Zonk (apply current substitution)
        tau' = apply_subst(tau)

        // Step 2: Check if already solved
        if tau' is a concrete type:
            if instance_env.has(class, tau'):
                evidence = instance_env.lookup(class, tau')
                fill(constraint.evidence_var, evidence)
            else:
                error("No instance for {class} {tau'}")

        // Step 3: Check if still polymorphic
        else if tau' is a variable:
            // Keep as residual (will be quantified)
            residual.push(constraint)

    return (solved_evidence, residual_constraints)
```

**GHC reference**: `GHC.Tc.Solver.Solve` implements `simplify_loop` which iterates solving until a fixed point. It uses an **inert set** (solved constraints available for rewriting) and a **work list** (constraints to process). For Flux, a single-pass solver is sufficient — iterate only if superclass expansion adds new constraints.

**Instance environment:**

```rust
// src/types/instance.rs (new file)
pub struct InstanceEnv {
    instances: HashMap<ClassId, Vec<Instance>>,
}

pub struct Instance {
    pub class: ClassId,
    pub type_args: Vec<InferType>,      // e.g., [Int] for Num Int
    pub context: Vec<WantedConstraint>, // e.g., [Eq a] for Eq (List a)
    pub evidence: EvidenceExpr,         // dictionary construction
}

// Instance matching
impl InstanceEnv {
    fn lookup(&self, class: ClassId, ty: &InferType) -> Option<&Instance> {
        self.instances.get(&class)?.iter().find(|inst| {
            unify(&inst.type_args[0], ty).is_ok()
        })
    }
}
```

**GHC reference**: `GHC.Tc.Solver.Dict` (`tryInstances`) matches wanted constraints against top-level instances. It uses `matchClassInst` which checks instance heads and verifies all instance context constraints are satisfiable. For Flux, start with exact matching (no backtracking).

**Superclass expansion:**

When `Ord a` is wanted and `Ord` has superclass `Eq`, also generate `Eq a`:

```rust
fn expand_superclasses(constraint: &WantedConstraint, class_env: &ClassEnv) -> Vec<WantedConstraint> {
    let class = class_env.get(constraint.class);
    class.superclasses.iter().map(|sc| {
        WantedConstraint::ClassConstraint {
            class: sc.class,
            type_args: constraint.type_args.clone(), // same type args
            evidence_var: fresh_evidence_var(),
            origin: ConstraintOrigin::Superclass(constraint.class),
            span: constraint.span,
        }
    }).collect()
}
```

**GHC reference**: `GHC.Tc.Solver.Dict` (`makeSuperClasses`) expands superclasses eagerly for Given constraints and lazily for Wanted constraints. Uses "expansion fuel" to prevent infinite loops from recursive superclass hierarchies. Flux should use eager expansion with a depth limit.

**Files**: `src/types/solver.rs` (rewrite), new `src/types/instance.rs`

### Phase 3 — Evidence elaboration (dictionary passing)

**Current Flux**: Core IR has no evidence. A call to `x + y` compiles to `PrimOp(Add, [x, y])`.

**Target**: After constraint solving, translate class method calls to explicit dictionary arguments in Core IR.

**Source → Core translation:**

```
// Source:
fn double<a: Num>(x: a): a { x + x }

// After constraint solving + elaboration:
// Core IR:
fn double(dict_num: NumDict, x: Value): Value {
    let add_fn = AdtField(dict_num, 0)  // extract 'add' method
    App(add_fn, [x, x])
}
```

**Dictionary representation in Core:**

```rust
// Each type class becomes an ADT in Core IR
// class Num<a> { fn add(a, a) -> a; fn sub(a, a) -> a; fn mul(a, a) -> a; ... }
// becomes:
CoreExpr::Adt {
    tag: "NumDict",
    fields: [closure_add, closure_sub, closure_mul, ...]
}

// Instance Num<Int> provides:
CoreExpr::Adt {
    tag: "NumDict",
    fields: [prim_int_add, prim_int_sub, prim_int_mul, ...]
}
```

**Call site elaboration:**

```rust
// Before elaboration:
// double(5)  where double : Num a => a -> a
//
// After elaboration (a = Int, Num Int solved):
// double(numIntDict, 5)
```

**GHC reference**: `GHC.Tc.Types.Evidence` defines `HsWrapper` which wraps Core expressions with type/evidence applications:
- `WpTyApp ty` — apply type argument `@Int`
- `WpEvApp ev` — apply dictionary argument `numIntDict`
- `WpTyLams [a]` — abstract over type variable
- `WpEvLams [d]` — abstract over dictionary variable

For Flux, dictionaries are just closures/ADTs — no special wrapper type needed. The elaboration pass walks Core IR and replaces `PrimOp(Add, [x, y])` with `App(AdtField(dict, 0), [x, y])` where `dict` is the resolved evidence variable.

**Files**: new `src/core/passes/dictionary.rs`, updates to `src/core/mod.rs` (add evidence vars to CoreDef)

### Phase 4 — Defaulting

**Problem**: After inference, some type variables remain ambiguous:
```flux
fn main() with IO {
    print(1 + 2)  // Num a => a, but which a?
}
```

**Defaulting rules** (following GHC, simplified):

1. Collect all unsolved constraints with ambiguous type variables
2. For each ambiguous variable `a` with constraints `C1 a, C2 a, ...`:
   - If all `Ci` have instances for `Int`, default `a = Int`
   - Else if all `Ci` have instances for `Float`, default `a = Float`
   - Else report "ambiguous type variable" error
3. Re-solve with defaulted types

**GHC reference**: `GHC.Tc.Solver.Default` implements defaulting with configurable default type lists. GHC's default list is `[Integer, Double]`. The `ExtendedDefaultRules` extension adds `()` and `[]`. For Flux, `[Int, Float]` is sufficient.

**Files**: `src/types/solver.rs` (defaulting pass after main solving)

### Phase 5 — Generalization with constraints

**Current Flux** (`src/types/scheme.rs`):
```rust
struct Scheme {
    forall: Vec<TypeVarId>,
    infer_type: InferType,
}
```
No constraints in schemes.

**Target**:
```rust
struct Scheme {
    forall: Vec<TypeVarId>,
    constraints: Vec<ClassConstraint>,  // NEW: Num a, Eq a, etc.
    infer_type: InferType,
}
```

**Generalization change:**

```rust
// Before:
fn generalize(ty: &InferType, env_free: &HashSet<TypeVarId>) -> Scheme {
    let forall = ty.free_vars().difference(env_free).collect();
    Scheme { forall, infer_type: ty.clone() }
}

// After:
fn generalize(ty: &InferType, residual: &[WantedConstraint],
              env_free: &HashSet<TypeVarId>) -> Scheme {
    let forall = ty.free_vars().difference(env_free).collect();
    // Only keep constraints that mention quantified variables
    let constraints = residual.iter()
        .filter(|c| c.mentions_any(&forall))
        .cloned().collect();
    Scheme { forall, constraints, infer_type: ty.clone() }
}
```

**Instantiation change:**

When instantiating a scheme with constraints, generate fresh evidence variables for each constraint:

```rust
fn instantiate(&self, counter: &mut u32) -> (InferType, Vec<WantedConstraint>) {
    let (ty, var_map) = self.instantiate_type(counter);
    let new_constraints = self.constraints.iter().map(|c| {
        c.apply_var_map(&var_map)  // substitute quantified vars with fresh
    }).collect();
    (ty, new_constraints)
}
```

**GHC reference**: `GHC.Tc.Gen.Bind` (`simplifyInfer`) decides quantification by analyzing which predicates mention quantified variables. The `decideQuantification` function removes predicates that are fully determined (no quantified vars) and keeps the rest as part of the type scheme. Flux should follow the same filter.

**Files**: `src/types/scheme.rs`, `src/ast/type_infer/statement.rs`, `src/ast/type_infer/function.rs`

### Phase 6 — Bidirectional type checking (optional, deferred)

**Current Flux**: Pure inference — types flow upward only.

**Target**: Add a checking mode where expected types flow downward. This improves inference for lambdas and higher-rank types.

**Example where checking helps:**

```flux
// Without checking mode:
let f = map(xs, \x -> x + 1)
// Lambda \x -> x + 1 is inferred bottom-up
// x has fresh var, + generates Num constraint
// Works but needs defaulting

// With checking mode:
// map : List<a> -> (a -> b) -> List<b>
// xs : List<Int>  =>  a = Int
// Expected lambda type: Int -> b
// Lambda checked against (Int -> b):
//   x : Int (pushed down), body infers to Int
//   b = Int
// No defaulting needed
```

**GHC reference**: `GHC.Tc.Gen.Expr` uses `ExpRhoType`:
- `Check ty`: expected type is known, check expression against it
- `Infer ref`: expected type is unknown, fill `ref` with inferred type

For Flux, this means adding an optional `expected_type` parameter to `infer_expression`:

```rust
fn check_or_infer_expression(
    &mut self,
    expr: &Expression,
    expected: Option<&InferType>,  // None = infer, Some = check
) -> InferType
```

When `expected` is `Some(ty)`:
- For lambdas: decompose `ty` into param/return types, push down
- For applications: use expected return type to guide inference
- For literals: unify with expected type immediately

**Impact**: Better inference, fewer annotations needed, prerequisite for higher-rank types.

**Files**: `src/ast/type_infer/expression/mod.rs`, `src/ast/type_infer/expression/lambda.rs`

### Phase 7 — Kind system

**Current Flux**: No kind tracking. `List`, `Option`, `Int` are all `TypeConstructor` variants without kind information.

**Target**: Type constructors carry kinds:

```rust
// src/types/kind.rs (new file)
pub enum Kind {
    Type,                       // * — the kind of value types
    Arrow(Box<Kind>, Box<Kind>), // k1 -> k2 — type constructor kinds
}

// Updated type_constructor.rs:
pub struct TypeConstructorInfo {
    pub name: TypeConstructor,
    pub kind: Kind,
}

// Built-in kinds:
// Int    : Type
// String : Type
// List   : Type -> Type
// Array  : Type -> Type
// Option : Type -> Type
// Map    : Type -> Type -> Type
// (->)   : Type -> Type -> Type
```

**Kind checking**: When a type application `List<Int>` is written:
1. Look up `List` kind: `Type -> Type`
2. Check that `Int` has kind `Type`
3. Result kind: `Type`

**GHC reference**: GHC unifies kinds and types — `Type :: Type` (self-referential). The kind checker in `GHC.Tc.Gen.HsType` uses the same unification infrastructure as type checking. Flux should keep kinds as a **separate, simpler system** — just `Type` and `->`. No kind polymorphism, no promoted types.

**Required for**: `Functor`, `Foldable`, `Traversable` type classes (their parameters have kind `Type -> Type`).

**Files**: new `src/types/kind.rs`, updates to `src/types/type_constructor.rs`

---

## Side-by-side: Flux vs GHC data structures

### Type variables

| | Flux | GHC |
|-|------|-----|
| **Representation** | `TypeVarId(u32)` | `TcTyVar { mtv_ref: IORef MetaDetails }` |
| **Binding** | Insert into `HashMap<TypeVarId, InferType>` | Mutate IORef to `Indirect ty` |
| **Scope tracking** | `var_levels: HashMap<TypeVarId, u32>` | `mtv_tclvl: TcLevel` embedded in var |
| **Kinds** | Uniform (none) | `TauTv`, `TyVarTv`, `ConcreteTv`, `CycleBreakerTv`, `QLInstVar` |
| **Resolution** | `resolve_head()` follows substitution chains | `readMetaTyVar` reads IORef |

**Recommendation**: Keep Flux's substitution-based approach. It's simpler, pure, and sufficient. GHC's mutable IORefs are faster but add complexity (zonking pass required). Flux programs are small enough that substitution composition is not a bottleneck.

### Substitution

| | Flux | GHC |
|-|------|-----|
| **Type** | `TypeSubst { type_bindings: HashMap, row_bindings: HashMap }` | No explicit substitution (IORef-based) |
| **Composition** | `compose(s1, s2)` left-biased merge | N/A — variable filling is in-place |
| **Application** | `apply_type_subst()` recursive walk with cycle detection | Zonking: `zonkTcType` traverses and reads IORefs |
| **Row bindings** | Explicit `row_bindings` for effect row variables | N/A (GHC has no row types) |

### Unification

| | Flux | GHC |
|-|------|-----|
| **Entry** | `unify(t1, t2) -> Result<TypeSubst, UnifyError>` | `uType(env, t1, t2) -> TcM CoercionN` |
| **Occurs check** | `occurs_in_with_ctx(var, ty, subst)` | `occCheckExpand(tv, ty)` |
| **Any/gradual** | `Any` unifies with everything | No `Any` — strict typing |
| **Evidence** | None (success/fail) | Returns `CoercionN` proof term |
| **Roles** | None | Nominal/representational/phantom |
| **Row unification** | 4-case algorithm for effect rows | N/A |
| **Deferred** | Never | Yes — queues for constraint solver |

### Schemes

| | Flux | GHC |
|-|------|-----|
| **Type** | `Scheme { forall, constraints, infer_type }` | `forall a. C a => rho` (embedded in `Type`) |
| **Constraints** | `Vec<SchemeConstraint>` with `class_name` + `type_vars: Vec<TypeVarId>` | Embedded as `FunTy (=>) constraint rho` |
| **Instantiation** | Replace forall vars with fresh vars + remap constraint type_vars | Fresh type vars + fresh evidence vars |
| **Generalization** | `free_in_type - free_in_env` + `collect_scheme_constraints` promotes variable constraints | Constraint-driven: `simplifyInfer` |

---

## Decisions

### What to adopt from GHC

| Feature | Why | Complexity |
|---------|-----|------------|
| Constraint generation | Enables type classes | Low — extend existing InferCtx |
| Single-pass constraint solver | Resolves class constraints | Medium — ~500 lines |
| Dictionary elaboration in Core | Compiles type classes | Medium — new Core pass |
| Defaulting (Int, Float) | Avoids ambiguous type errors | Low — 50 lines |
| Constrained schemes | Quantify predicates | Low — extend Scheme struct |
| Kind system (Type, ->) | Enables Functor/Foldable | Low — new 100-line module |
| Superclass expansion | Eq from Ord automatically | Low — 30 lines |

### What to skip

| Feature | Why skip | GHC complexity |
|---------|----------|----------------|
| Mutable unification variables | Substitution-based is simpler and sufficient | ~3K lines (zonking) |
| Bidirectional checking | Inference-only works for Flux's use cases initially | ~5K lines |
| Quick Look (impredicativity) | No higher-rank types needed | ~3K lines |
| Coercion evidence | Flux has no newtypes or GADTs | ~5K lines |
| Inert set solver | Single-pass sufficient for Flux | ~80K lines |
| Deferred type errors | Flux wants strict typing | ~2K lines |
| Role tracking | No newtype/coerce in Flux | ~2K lines |
| Skolemisation | Not needed without higher-rank types | ~1K lines |

---

## Drawbacks

- **Constraint generation changes inference behavior**: Some currently-inferred types will become ambiguous and require annotations or defaulting. Mitigated by defaulting rules.

- **Dictionary passing adds runtime overhead**: Each polymorphic call gets an extra argument. Mitigated by: (1) NaN-boxed dictionaries are cheap to pass, (2) monomorphic call sites inline the dictionary, (3) future specialization pass.

- **Solver complexity**: Even a simplified solver adds ~500 lines of non-trivial logic. Mitigated by: no type families, no backtracking, no overlapping instances.

## Rationale and alternatives

### Why not a full constraint-based approach (OutsideIn)?

GHC's OutsideIn(X) solver handles implications (nested scopes with Given/Wanted separation), type families, and GADTs. Flux doesn't need any of these. A simple single-pass solver that processes constraints after inference is sufficient and dramatically simpler (~500 lines vs ~850K lines).

### Why not monomorphization instead of dictionaries?

Monomorphization (Rust's approach) eliminates runtime overhead but causes code bloat and prevents separate compilation. Dictionary passing is simpler to implement, works with Flux's existing closure representation, and can be optimized later via specialization.

## Prior art

- **GHC**: Full constraint solver in `GHC/Tc/Solver/` (~850K lines). Dictionary passing via `HsWrapper` evidence.
- **PureScript**: Simplified constraint solver (~3K lines). No overlapping instances. Good reference for Flux's target complexity.
- **Koka**: Row-typed effects (like Flux) but no type classes. Uses overloading via implicit resolution.
- **Lean 4**: Type class instances with backtracking search. More complex than needed for Flux.
- **OCaml**: No type classes. Row polymorphism for objects. Different design space.

## Unresolved questions

- **Should Flux use GHC's level-invariant for constraint solving?** GHC's `TcLevel` prevents unsound generalization in nested implications. Flux doesn't have implications, so the existing level-based generalization should suffice.

- **How to handle recursive dictionaries?** `instance Eq a => Eq (List a)` requires the Eq dictionary for `a` to build the Eq dictionary for `List a`. This is handled by passing the `a`-dictionary as a closure argument. GHC builds recursive dictionaries via `letrec`. Flux should follow GHC.

- **When to run the solver — after each binding or after the whole program?** GHC runs the solver at generalization points (let-bindings and top-level definitions). Flux should do the same: solve constraints at each let/fn generalization point, carry residuals upward.

- **Polymorphic recursion requires explicit type signatures.** Currently, recursive functions like `List.fold(xs, acc, f)` are inferred monomorphically — the accumulator type gets unified with the list element type during the recursive call, making `fold(int_list, "", string_fn)` a type error. GHC solves this by requiring an explicit signature: `foldl :: forall a b. (b -> a -> b) -> b -> [a] -> b`. Flux needs the same: when a recursive function has a type annotation, use that annotation as the polymorphic type during recursive calls (GHC's `tcPolyCheck` path) instead of the monomorphic unification variables (`tcPolyInfer`). This falls under Phase 5 (generalization) or could be a standalone sub-proposal, since it does not require type classes — only skolemisation and signature-directed checking.

## Future possibilities

- **Specialization pragma**: `@specialize fn sort<Int>` to monomorphize hot polymorphic functions, eliminating dictionary overhead
- **Bidirectional checking**: Better inference for lambdas and higher-rank types
- **Implication constraints**: Needed if Flux adds GADTs or local type refinement
- **Type family reduction**: Type-level computation in the constraint solver
- **Backtracking instance search**: For overlapping or conditional instances (not recommended)
