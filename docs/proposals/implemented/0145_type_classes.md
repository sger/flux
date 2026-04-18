- Feature Name: Type Classes
- Start Date: 2026-04-07
- Status: Implemented (2026-04-18) — core feature delivered; follow-ons shipped via 0146/0147/0149/0150 and Num defaulting via 0127
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0123 Phase 1–2 (done), Option A typed primops (done)
- Delivery notes: [0146](implemented/0146_type_class_hardening.md) (hardening: E445 superclasses, E446 extra methods, structural dedup, multi-param mangling), [0147](implemented/0147_constrained_type_params_and_instance_contexts.md) (constrained type param runtime dispatch + instance context enforcement, 2026-04-14), [0149](implemented/0149_operator_desugaring.md) (operator desugaring — removes `Any`-typed primop fallback), [0150](implemented/0150_hkt_instance_resolution.md) (HKT instance resolution, 2026-04-09), [0127 Phase 3](implemented/0127_type_inference_ghc_parity.md) (Num defaulting, commit `457eae2d`). Remaining: method arity validation (small polish), `Monoid` registration (deferred — awaits `Foldable`), `Flow.Foldable` consumer-surface migration (library work).

## Summary

Add Haskell-style type classes to Flux. Type classes enable constrained polymorphism — functions like `+` and `==` work on multiple types without falling back to `Any`. This replaces the current gradual typing escape hatch with compile-time verified overloading.

This proposal covers syntax, parsing, type inference integration, constraint solving, and dictionary-passing elaboration in Core IR. It is scoped to single-parameter type classes without higher-kinded types.

---

## Implementation status

Last updated: 2026-04-09

### Completed

| Step | Feature | Files | Notes |
|------|---------|-------|-------|
| **1. Parser** | `class`/`instance` keywords, AST types, superclass `=>` syntax, instance context syntax | `token_type.rs` (`FatArrow`), `type_class.rs`, `statement.rs`, `parser/statement.rs`, `core/mod.rs`, `cfg/mod.rs` | Full pipeline: Core IR, CFG, LIR. `class Eq<a> => Ord<a>` and `instance Eq<a> => Eq<List<a>>` parse correctly. |
| **2. ClassEnv** | Collect and validate declarations, error codes E440–E446 | `types/class_env.rs`, `compiler_errors.rs`, `registry.rs` | Validates: duplicate class (E440), unknown class (E441), missing methods (E442), duplicate instances (E443, structural equality), superclass instances (E445), extra methods (E446). |
| **3. Constraints** | Emit class constraints during HM inference for operators and class method calls | `constraint.rs`, `operators.rs`, `calls.rs`, `mod.rs` | Multi-param support: `type_args: Vec<InferType>`. Constraints recorded + resolved in `InferProgramResult`. |
| **4. Constraint solving** | Resolve constraints at generalization: concrete types → instance lookup | `class_solver.rs` | E444 for unsatisfied concrete constraints in the maintained static-typing path. Multi-param matching. Type variables promoted to `Scheme.constraints`. |
| **5. Dispatch** | Compile-time monomorphic resolution, operator desugaring | `class_dispatch.rs`, `compiler/pipeline.rs`, `core/lower_ast/mod.rs`, `expression.rs` | Mangled instance functions (`__tc_Class_Type1_Type2_method`). Operators desugar to class methods (`==` → `eq`, `+` → `add`, `++` → `append`). `type_of()` fallback removed. |
| **5b. Dictionary elaboration** | Core-to-Core pass: dict construction, body rewriting, constraint promotion, call-site resolution | `dict_elaborate.rs`, `scheme.rs`, `constraint.rs`, `function.rs`, `statement.rs`, `class_dispatch.rs`, `class_env.rs`, `cfg/mod.rs` | `Scheme.constraints` carries class constraints. `__dict_{Class}_{Type}` CoreDefs as MakeTuple. Constrained Lams get dict params. Class method calls → TupleField. Polymorphic forwarding + concrete call-site resolution via `resolve_dict_args_for_call`. |
| **6. Built-in classes** | `Eq`, `Ord`, `Num`, `Show`, `Semigroup` with instances for primitive types | `class_env.rs` | Builtins registered before user classes. Don't override user declarations. |
| **HKTs** | Higher-kinded types + kind system | `types/kind.rs`, `types/infer_type.rs` (`HktApp`), `types/unify.rs` | `Kind::Type` and `Kind::Arrow`. Per-method type params (`fn fmap<a, b>`). HKT syntax parses (`class Functor<f>`, `instance Functor<List>`) but compile-time instance resolution for HKT type constructors not yet implemented — runtime dispatch panics. |
| **Hardening** | Superclass enforcement, structural dedup, extra method validation, multi-param classes | Proposal 0146 (complete) | E445 superclass enforcement, `TypeExpr::structural_eq`, E446 extra methods, `ClassDef.type_params: Vec<Identifier>`, multi-arg mangling/constraints. |

### Delivered via follow-on proposals

| Step | Feature | Delivered by |
|------|---------|--------------|
| **4** | Num defaulting: unconstrained `Num` variables default to `Int` | ✅ [0127 Phase 3](implemented/0127_type_inference_ghc_parity.md) (commit `457eae2d`) |
| **6** | Complete operator desugaring — remove Any-typed primop fallback | ✅ [0149 Operator Desugaring](implemented/0149_operator_desugaring.md) |
| **7a** | Stdlib migration: split `Flow.List`/`Flow.Array` into typed modules | ✅ Step 7a landed; see `lib/Flow/List.flx`, `lib/Flow/Array.flx` |
| — | Constrained type param runtime dispatch: `fn f<a: Eq>(...)` | ✅ [0147](implemented/0147_constrained_type_params_and_instance_contexts.md) |
| — | Instance context enforcement: evidence inside instance body | ✅ [0147](implemented/0147_constrained_type_params_and_instance_contexts.md) |
| — | HKT instance resolution (compile-time dispatch for `Functor<List>` etc.) | ✅ [0150](implemented/0150_hkt_instance_resolution.md) |
| **Hardening** | Superclass enforcement, structural dedup, extra method validation, multi-param mangling | ✅ [0146](implemented/0146_type_class_hardening.md) |

### Remaining (not blocking closure)

| Step | Feature | Blocker | Difficulty | Impact |
|------|---------|---------|------------|--------|
| **2** | Method arity validation (instance vs class signature) | None | Small | Quality-of-diagnostics — doesn't block feature use |
| **6** | Register `Monoid` class | `Foldable` | Deferred by design | No value without `Foldable`; `Semigroup` covers `append` today |
| **7b** | `Flow.Foldable` consumer-surface migration | None | Medium | Library-side polish; class + instances exist in stdlib |

### Resolved limitations

All four original limitations have been addressed:

1. **Superclass parsing** — Done. `FatArrow` (`=>`) token added, both class and instance contexts parse.
2. **Operator desugaring** — Done. Polymorphic operators route through class methods.
3. **Runtime `type_of()` dispatch** — Removed. All dispatch via monomorphic resolution or dictionaries.
4. **Duplicate instance detection** — Fixed. `TypeExpr::structural_eq()` replaces `format!("{:?}")`.

### Architecture decisions

- **Dictionary passing (implemented) + monomorphic dispatch (retained)**: The dictionary elaboration pass (`dict_elaborate.rs`) runs as a Core-to-Core pass (Stage 0.5, before simplification). It follows the evidence pass pattern: constrained functions get dictionary parameters, class method calls become `TupleField` extractions. Monomorphic call sites (where `try_resolve_class_call` resolves to `__tc_*` mangled names during AST-to-Core lowering) are left unchanged — no dictionary overhead. Only truly polymorphic calls pay the indirection cost.

- **Scheme carries constraints**: `Scheme.constraints: Vec<SchemeConstraint>` records which type variables are class-constrained. Populated during HM generalization via `collect_scheme_constraints()`. The dict elaboration pass reads these to determine which functions need dictionary parameters.

- **AST preprocessing retained**: Instance methods are still injected as regular functions during Phase 1b (`generate_dispatch_functions`), before type inference. This ensures mangled functions (`__tc_*`) go through the full pipeline. Phase 1b also pre-interns `__dict_*` names for the dict elaboration pass.

- **Mangled names**: Instance methods use `__tc_ClassName_TypeName_methodName` naming (e.g., `__tc_Eq_Int_eq`). Dictionary values use `__dict_ClassName_TypeName` naming (e.g., `__dict_Eq_Int`). Both are internal — users call the class method name (`eq`).

---

## Motivation

After the maintained static-typing hardening and the typed primop foundation, Flux can reject programs with `Any`-style escape hatches. But many real programs still rely on polymorphic operations:

```flux
fn add(x, y) { x + y }     // inferred as: a -> a -> a (polymorphic)
                             // but + only works on Int and Float at runtime

fn show_value(x) {          // inferred as: a -> String
    to_string(x)            // but to_string only works on certain types
}
```

Without type classes, the compiler can't verify that `+` is valid for the actual type `a` gets instantiated to. Type classes solve this by making the constraint explicit:

```flux
fn add<a: Num>(x: a, y: a) -> a { x + y }
// Now the compiler proves Num<Int> exists before allowing add(1, 2)
```

---

## Guide-level explanation

### Declaring a type class

A type class defines a set of methods that types can implement:

```flux
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
    fn neq(x: a, y: a) -> Bool { !eq(x, y) }  // default implementation
}
```

- `Eq` is the class name
- `a` is the type parameter — any type that implements `Eq` will substitute for `a`
- `eq` is a required method (no body — instances must provide it)
- `neq` has a default body — instances can override it or use the default

### Implementing instances

An instance provides the class methods for a specific type:

```flux
instance Eq<Int> {
    fn eq(x, y) { prim_int_eq(x, y) }
}

instance Eq<String> {
    fn eq(x, y) { prim_str_eq(x, y) }
}

instance Eq<Bool> {
    fn eq(x, y) {
        match (x, y) {
            (true, true) -> true,
            (false, false) -> true,
            _ -> false
        }
    }
}
```

### Constrained functions

Functions that use class methods declare constraints with the `:` syntax in type parameters:

```flux
fn contains<a: Eq>(xs: List<a>, elem: a) -> Bool {
    match xs {
        [] -> false,
        [h | t] -> if eq(h, elem) { true } else { contains(t, elem) }
    }
}
```

The constraint `a: Eq` means: "this function works for any type `a` that has an `Eq` instance."

### Superclasses

A class can require another class as a prerequisite:

```flux
class Eq<a> => Ord<a> {
    fn compare(x: a, y: a) -> Int
    fn lt(x: a, y: a) -> Bool { compare(x, y) < 0 }
    fn gt(x: a, y: a) -> Bool { compare(x, y) > 0 }
    fn lte(x: a, y: a) -> Bool { compare(x, y) <= 0 }
    fn gte(x: a, y: a) -> Bool { compare(x, y) >= 0 }
}
```

The `Eq<a> =>` prefix means: every type with `Ord` must also have `Eq`. This lets `Ord` methods use `eq` without re-declaring the constraint.

### Constrained instances

An instance can require constraints on type parameters:

```flux
instance Eq<a> => Eq<List<a>> {
    fn eq(xs, ys) {
        match (xs, ys) {
            ([], []) -> true,
            ([h1 | t1], [h2 | t2]) -> eq(h1, h2) && eq(t1, t2),
            _ -> false
        }
    }
}

instance Eq<a> => Eq<Option<a>> {
    fn eq(x, y) {
        match (x, y) {
            (None, None) -> true,
            (Some(a), Some(b)) -> eq(a, b),
            _ -> false
        }
    }
}
```

### Operator desugaring

Operators desugar to class method calls:

| Expression | Desugars to | Requires |
|-----------|-------------|----------|
| `x + y` | `Num.add(x, y)` | `Num<a>` |
| `x - y` | `Num.sub(x, y)` | `Num<a>` |
| `x * y` | `Num.mul(x, y)` | `Num<a>` |
| `x / y` | `Num.div(x, y)` | `Num<a>` |
| `x == y` | `Eq.eq(x, y)` | `Eq<a>` |
| `x != y` | `Eq.neq(x, y)` | `Eq<a>` |
| `x < y` | `Ord.lt(x, y)` | `Ord<a>` |
| `x > y` | `Ord.gt(x, y)` | `Ord<a>` |
| `x <= y` | `Ord.lte(x, y)` | `Ord<a>` |
| `x >= y` | `Ord.gte(x, y)` | `Ord<a>` |
| `x ++ y` | `Semigroup.append(x, y)` | `Semigroup<a>` |

### Built-in class hierarchy

```
Eq
  └── Ord

Num

Show

Semigroup
  └── Monoid
```

### Complete example

```flux
class Show<a> {
    fn show(x: a) -> String
}

instance Show<Int> {
    fn show(x) { to_string(x) }
}

instance Show<Bool> {
    fn show(x) {
        if x { "true" } else { "false" }
    }
}

instance Show<a> => Show<List<a>> {
    fn show(xs) {
        "[" ++ join(map(xs, \(x) -> show(x)), ", ") ++ "]"
    }
}

fn print<a: Show>(x: a) with IO {
    println(show(x))
}

fn main() with IO {
    print(42)           // uses Show<Int>
    print(true)         // uses Show<Bool>
    print([1, 2, 3])    // uses Show<List<Int>>, which requires Show<Int>
}
```

---

## Reference-level explanation

### Phase 1 — Syntax and parsing

#### New keywords

Add `class` and `instance` as reserved keywords in the lexer.

#### New AST nodes

```rust
// In src/syntax/statement.rs

pub enum Statement {
    // ... existing variants ...

    /// Type class declaration: class Eq<a> => Ord<a> { methods... }
    Class {
        name: Identifier,
        type_params: Vec<Identifier>,
        superclasses: Vec<ClassConstraint>,
        methods: Vec<ClassMethod>,
        span: Span,
    },

    /// Instance declaration: instance Eq<a> => Eq<List<a>> { methods... }
    Instance {
        class_name: Identifier,
        type_args: Vec<TypeExpr>,
        context: Vec<ClassConstraint>,
        methods: Vec<InstanceMethod>,
        span: Span,
    },
}

/// A constraint like Eq<a> or Ord<a>
pub struct ClassConstraint {
    pub class_name: Identifier,
    pub type_args: Vec<TypeExpr>,
    pub span: Span,
}

/// A method signature in a class declaration
pub struct ClassMethod {
    pub name: Identifier,
    pub params: Vec<Identifier>,
    pub param_types: Vec<TypeExpr>,
    pub return_type: TypeExpr,
    pub default_body: Option<Block>,  // None = required method
    pub span: Span,
}

/// A method implementation in an instance declaration
pub struct InstanceMethod {
    pub name: Identifier,
    pub params: Vec<Identifier>,
    pub body: Block,
    pub span: Span,
}
```

#### Parsing rules

```
class_decl     = "class" [constraint "=>"] NAME "<" type_params ">" "{" class_methods "}"
instance_decl  = "instance" [constraint "=>"] NAME "<" type_args ">" "{" instance_methods "}"
constraint     = NAME "<" type_args ">"
class_methods  = (class_method)*
class_method   = "fn" NAME "(" params ")" "->" type [block]
instance_methods = (instance_method)*
instance_method  = "fn" NAME "(" params ")" block
```

**Parsing examples:**

```flux
// No superclass
class Eq<a> { ... }
// Parsed: superclasses = [], name = "Eq", type_params = ["a"]

// With superclass
class Eq<a> => Ord<a> { ... }
// Parsed: superclasses = [ClassConstraint("Eq", ["a"])], name = "Ord", type_params = ["a"]

// Instance with context
instance Eq<a> => Eq<List<a>> { ... }
// Parsed: context = [ClassConstraint("Eq", ["a"])], class_name = "Eq", type_args = [List<a>]
```

#### Ambiguity: `class Eq<a> => Ord<a>`

The parser sees `class` then `Eq<a>`. It doesn't know yet whether `Eq<a>` is the class being declared or a superclass constraint. Resolution:

1. Parse `Eq<a>`
2. If next token is `=>`, then `Eq<a>` is a superclass constraint — continue parsing the actual class name
3. If next token is `{`, then `Eq<a>` is the class being declared with no superclasses

This is the same disambiguation GHC uses.

### Phase 2 — Type inference integration

#### Class environment

A new data structure tracks declared classes and instances:

```rust
pub struct ClassEnv {
    /// class name → class definition
    pub classes: HashMap<Identifier, ClassDef>,
    /// (class name, head type constructor) → instance
    pub instances: Vec<InstanceDef>,
}

pub struct ClassDef {
    pub name: Identifier,
    pub type_param: Identifier,
    pub superclasses: Vec<ClassConstraint>,
    pub methods: Vec<MethodSig>,
    pub default_methods: HashMap<Identifier, Block>,
}

pub struct MethodSig {
    pub name: Identifier,
    pub param_types: Vec<InferType>,
    pub return_type: InferType,
}

pub struct InstanceDef {
    pub class_name: Identifier,
    pub head: InferType,              // e.g., Int, List<a>
    pub context: Vec<ClassConstraint>, // e.g., [Eq<a>] for Eq<List<a>>
    pub methods: HashMap<Identifier, Block>,
}
```

#### Constraint generation

During HM inference, when a class method is used, generate a constraint:

```
infer(x + y):
  fresh a
  unify(typeof(x), a)
  unify(typeof(y), a)
  emit constraint: Num<a>
  return type: a
```

Constraints are deferred — not solved immediately. They accumulate during inference and are resolved at generalization time.

```rust
pub struct ClassConstraintWanted {
    pub class_name: Identifier,
    pub type_arg: InferType,
    pub span: Span,
}
```

#### Constraint solving

At generalization time (when a function's type is finalized):

1. Apply the current substitution to all constraints
2. For each constraint `C<τ>`:
   - If `τ` is concrete (e.g., `Int`), look up `instance C<Int>` — if found, constraint is satisfied
   - If `τ` is a variable, the constraint becomes part of the function's type scheme: `forall a. C<a> => ...`
3. Report errors for unsatisfied constraints on concrete types

**Defaulting:** If a variable has only `Num` constraints and is never instantiated, default to `Int`.

#### Method resolution

When inference encounters a call to a class method (e.g., `eq(x, y)`):

1. Look up `eq` in the class environment → found in `Eq`
2. Instantiate the method signature: `eq : (a, a) -> Bool`
3. Emit constraint `Eq<a>`
4. Unify argument types with the instantiated parameter types

### Phase 3 — Dictionary elaboration (Core IR)

Type class usage is compiled to explicit dictionary passing in Core IR.

#### Dictionary representation

Each class becomes an ADT (record of closures):

```
// class Eq<a> { fn eq(a, a) -> Bool; fn neq(a, a) -> Bool }
// becomes:
data EqDict { EqDict((a, a) -> Bool, (a, a) -> Bool) }
```

Each instance becomes a dictionary value:

```
// instance Eq<Int> { fn eq(x, y) { prim_int_eq(x, y) } }
// becomes:
let eqIntDict = EqDict(\(x, y) -> prim_int_eq(x, y), \(x, y) -> !prim_int_eq(x, y))
```

#### Dictionary passing

Constrained functions get an extra dictionary parameter:

```
// Source:
fn contains<a: Eq>(xs: List<a>, elem: a) -> Bool { ... eq(h, elem) ... }

// Core IR:
fn contains(dict_eq: EqDict, xs: List, elem: Value) -> Bool {
    ... (dict_eq.0)(h, elem) ...   // extract eq method from dictionary
}

// Call site:
contains([1, 2, 3], 2)
// becomes:
contains(eqIntDict, [1, 2, 3], 2)
```

#### Superclass dictionaries

Superclass relationships add dictionary fields:

```
// class Eq<a> => Ord<a> { fn compare(a, a) -> Int; ... }
// becomes:
data OrdDict { OrdDict(EqDict, (a, a) -> Int, ...) }
//                      ^^^^^^ superclass dictionary embedded
```

### Phase 4 — Built-in classes and instances

The following classes and instances are provided by the compiler (not written in Flux source):

#### Eq

```flux
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
    fn neq(x: a, y: a) -> Bool { !eq(x, y) }
}

instance Eq<Int>    { fn eq(x, y) { prim_int_eq(x, y) } }
instance Eq<Float>  { fn eq(x, y) { prim_float_eq(x, y) } }
instance Eq<String> { fn eq(x, y) { prim_str_eq(x, y) } }
instance Eq<Bool>   { fn eq(x, y) { prim_bool_eq(x, y) } }
```

#### Ord

```flux
class Eq<a> => Ord<a> {
    fn compare(x: a, y: a) -> Int
    fn lt(x: a, y: a) -> Bool  { compare(x, y) < 0 }
    fn gt(x: a, y: a) -> Bool  { compare(x, y) > 0 }
    fn lte(x: a, y: a) -> Bool { compare(x, y) <= 0 }
    fn gte(x: a, y: a) -> Bool { compare(x, y) >= 0 }
}

instance Ord<Int>    { fn compare(x, y) { prim_int_cmp(x, y) } }
instance Ord<Float>  { fn compare(x, y) { prim_float_cmp(x, y) } }
instance Ord<String> { fn compare(x, y) { prim_str_cmp(x, y) } }
```

#### Num

```flux
class Num<a> {
    fn add(x: a, y: a) -> a
    fn sub(x: a, y: a) -> a
    fn mul(x: a, y: a) -> a
    fn neg(x: a) -> a
    fn abs(x: a) -> a
    fn from_int(n: Int) -> a
}

instance Num<Int> {
    fn add(x, y) { prim_int_add(x, y) }
    fn sub(x, y) { prim_int_sub(x, y) }
    fn mul(x, y) { prim_int_mul(x, y) }
    fn neg(x)    { prim_int_neg(x) }
    fn abs(x)    { prim_int_abs(x) }
    fn from_int(n) { n }
}

instance Num<Float> {
    fn add(x, y) { prim_float_add(x, y) }
    fn sub(x, y) { prim_float_sub(x, y) }
    fn mul(x, y) { prim_float_mul(x, y) }
    fn neg(x)    { prim_float_neg(x) }
    fn abs(x)    { prim_float_abs(x) }
    fn from_int(n) { prim_int_to_float(n) }
}
```

#### Show

```flux
class Show<a> {
    fn show(x: a) -> String
}

instance Show<Int>    { fn show(x) { prim_int_to_string(x) } }
instance Show<Float>  { fn show(x) { prim_float_to_string(x) } }
instance Show<String> { fn show(x) { x } }
instance Show<Bool>   { fn show(x) { if x { "true" } else { "false" } } }
```

#### Semigroup / Monoid

```flux
class Semigroup<a> {
    fn append(x: a, y: a) -> a
}

class Semigroup<a> => Monoid<a> {
    fn empty() -> a
}

instance Semigroup<String>   { fn append(x, y) { prim_str_concat(x, y) } }
instance Monoid<String>      { fn empty() { "" } }
```

---

## Implementation plan

### Step 1: Lexer + Parser (syntax only) — DONE

- [x] Add `class` and `instance` keywords to lexer
- [x] Add `ClassConstraint`, `ClassMethod`, `InstanceMethod` AST types
- [x] Add `Class` and `Instance` variants to `Statement`
- [x] Parse class and instance declarations
- [x] Add `CoreTopLevelItem::Class/Instance` and `IrTopLevelItem::Class/Instance`
- [x] Handle match exhaustiveness across 15+ files
- [x] Parse superclass syntax (`Eq<a> => Ord<a>`) — `FatArrow` token added
- [x] Parse instance context syntax (`instance Eq<a> => Eq<List<a>>`)

### Step 2: Class environment — DONE

- [x] Build `ClassEnv` from parsed `Class` and `Instance` statements
- [x] Validate: no duplicate classes (E440)
- [x] Validate: no duplicate instances for same type (E443)
- [x] Validate: instance for unknown class rejected (E441)
- [x] Validate: instance methods match class (missing required methods: E442)
- [x] Default methods respected (skipped if class provides default body)
- [x] Validate: superclass instances exist (E445 — checks class-level superclasses)
- [x] Validate: method arity matches class signature (E448)

### Step 3–5 MVP: Runtime dispatch — DONE

- [x] Generate mangled instance functions (`__tc_Eq_Int_eq`)
- [x] Generate `type_of()`-based dispatch functions for class method names
- [x] Inject generated functions into AST before predeclaration (Phase 1b)
- [x] End-to-end: `class Eq<a> { fn eq ... }` + `instance Eq<Int> { ... }` → `eq(1, 2)` works
- [x] Multi-instance dispatch in same scope — each call site instantiates fresh type variables; compile-time resolution selects the correct mangled instance function

### Step 3: Constraint generation — DONE

- [x] Add `Constraint::Class` variant and `WantedClassConstraint` to constraint system
- [x] Pass `ClassEnv` from compiler → `InferProgramConfig` → `InferCtx`
- [x] Pre-resolve well-known class symbols (Eq, Ord, Num, Semigroup) at inference start
- [x] Emit `Eq<a>` for `==`, `!=`; `Ord<a>` for `<`, `<=`, `>`, `>=`
- [x] Emit `Num<a>` for `+`, `-`, `*`, `/`, `%`; `Semigroup<a>` for `++`
- [x] Emit class constraint when a class method name is called (e.g., `eq(x, y)` → `Eq<typeof(x)>`)
- [x] Resolved constraints exposed in `InferProgramResult.class_constraints`
- [x] Generate functions for default class methods (e.g., `neq` from `Eq`)
- [x] Constraints enforced in the maintained static-typing path via `solve_class_constraints` (Step 4)

### Step 4: Constraint solving — DONE

- [x] After inference, walk `class_constraints` and check concrete types against ClassEnv
- [x] Concrete type with no instance → E444 "No type class instance"
- [x] Type variables left unsolved (future: add to scheme)
- [x] Compiler-generated code (default spans) skipped
- [x] Enforced in the maintained static-typing path
- [ ] Defaulting: unconstrained `Num` variables default to `Int` (deferred — Flux integer literals already infer as `Int` directly, unlike Haskell's polymorphic `Num a => a` literals. Defaulting only matters with `fromInteger` or polymorphic numeric constructors, which Flux doesn't have yet)

### Step 5: Dictionary elaboration — DONE

- [x] Extend `Scheme` with `constraints: Vec<SchemeConstraint>` field
- [x] Add `SchemeConstraint` type (class_name + type_var)
- [x] Add `generalize_with_constraints()` for constraint-aware generalization
- [x] Wire constraint promotion in function generalization (`function.rs`)
- [x] Wire constraint promotion in let-binding generalization (`statement.rs`)
- [x] Add `collect_scheme_constraints()` helper on `InferCtx`
- [x] Add `method_index()` to `ClassEnv` for canonical method ordering
- [x] Pre-intern `__dict_*` names during Phase 1b dispatch generation
- [x] Build `__dict_{Class}_{Type}` CoreDefs as MakeTuple of mangled refs
- [x] Rewrite constrained function bodies: prepend dict Lam params
- [x] Rewrite class method calls to `TupleField(dict, index)` extractions
- [x] Polymorphic forwarding: caller passes its own dict to callee
- [x] Wire dict elaboration pass into bytecode pipeline (`cfg/mod.rs`)
- [x] Wire dict elaboration pass into dump-core pipeline (`compiler/mod.rs`)
- [x] Guard: no-op when no functions have scheme constraints
- [x] 18 unit tests for dict construction, body rewriting, method index, integration
- [x] Concrete call-site resolution: resolve `__dict_{Class}_{Type}` during AST-to-Core lowering via `resolve_dict_args_for_call()`
- [x] Thread HM type info: `hm_expr_types` + `TypeEnv` used in `resolve_constraint_type()` to match argument types to constraint type vars
- [x] Remove runtime `type_of()` dispatch — `__rt_*` fallback removed from Core lowering, bytecode compiler, and polymorphic stubs

### Step 6: Built-in classes — DONE

- [x] Register `Eq`, `Ord`, `Num`, `Show`, `Semigroup` in the class environment
- [x] Register built-in instances: Eq/Show (Int, Float, String, Bool), Ord (Int, Float, String), Num (Int, Float), Semigroup (String)
- [x] Built-in classes don't override user-declared classes
- [x] Constraint solver verifies operator usage against built-in instances in the maintained static-typing path
- [ ] Register `Monoid` class (deferred — low value without `Foldable`; `Semigroup` covers `append`)
- [ ] Wire operator desugaring to class methods — see **Proposal 0149** (early operator desugaring). Desugar polymorphic operators to class method calls during type inference (AST→AST pass before Core IR), so both VM and LLVM backends see the same representation. Concrete Int/Float operators keep using fast-path primops.
- [ ] Remove `Any`-typed primop overloads — replaced by class dispatch (after Proposal 0149)

### Step 7: Flow stdlib migration — IN PROGRESS (7a complete, 7b partially landed)

- [x] **Step 7a**: Split `Flow.List` / `Flow.Array` into separate typed modules with full generic signatures
- [x] **Step 7a**: Add `Eq` constraints to List/Array functions: `contains`, `nub`, `delete`, `is_prefix`, `is_suffix`, `unique_by`
- [x] **Step 7a**: Add `Ord` constraints to List/Array functions: `sort`, `sort_by`, `maximum`, `minimum`
- [x] **Step 7a**: Add `Eq`/`Ord` constraints to Assert functions: `assert_eq`, `assert_neq`, `assert_gt`, `assert_lt`, `assert_gte`, `assert_lte`
- [x] **Step 7a**: Type `Array.range(start: Int, stop: Int) -> Array<Int>`
- [ ] **Step 7a**: Type `List.first` / `List.last` as `-> Option<a>` (deferred — breaks 31 example files that depend on bare return value; requires migration)
- [ ] **Step 7a**: Type `List.range` (deferred — cons literal `[start | range(...)]` inference conflicts with return annotation)
- [ ] **Step 7b**: Add `Foldable` class with `fold`, `length`, `to_list` methods — `Flow.Foldable` now exists as the stdlib home for the class plus `Foldable<List>` / `Foldable<Array>` instances, but the explicit-import consumer surface still needs follow-up work before this step can be marked complete.
- [ ] **Future (HKTs)**: Add `Functor` class, unify `map` across List, Array, Option — unblocked by **Proposal 0150**. Remaining work is library surface design plus concrete `Functor` instances.

The Flow standard library (`lib/Flow/*.flx`) currently uses untyped, dynamically-dispatched functions. Type classes enable a clean migration to typed, statically-dispatched modules.

#### Current state

```flux
// lib/Flow/List.flx — today
module Flow.List {
    public fn map(xs, f) {
        if is_array(xs) {             // runtime type dispatch
            fn map_arr(i, acc) { ... }
            map_arr(0, [||])
        } else {
            fn map_acc(ys, acc) { ... }
            map_acc(xs, [])
        }
    }
}
```

Problems:
- `map` accepts any type via `Any` — no compile-time safety
- Runtime `is_array()` dispatch is invisible to the type system
- `map` on a `Map` or `Int` silently does the wrong thing

#### Target state

Split the monolithic polymorphic functions into typed, collection-specific modules:

```flux
// lib/Flow/List.flx — after type classes
module Flow.List {
    public fn map<a, b>(xs: List<a>, f: (a) -> b) -> List<b> {
        fn go(ys: List<a>, acc: List<b>) -> List<b> {
            match ys {
                [] -> reverse(acc),
                [h | t] -> go(t, [f(h) | acc])
            }
        }
        go(xs, [])
    }

    public fn filter<a>(xs: List<a>, pred: (a) -> Bool) -> List<a> { ... }
    public fn fold<a, b>(xs: List<a>, init: b, f: (b, a) -> b) -> b { ... }
}

// lib/Flow/Array.flx — after type classes
module Flow.Array {
    public fn map<a, b>(xs: Array<a>, f: (a) -> b) -> Array<b> { ... }
    public fn filter<a>(xs: Array<a>, pred: (a) -> Bool) -> Array<a> { ... }
    public fn fold<a, b>(xs: Array<a>, init: b, f: (b, a) -> b) -> b { ... }
}
```

#### With Functor (future, requires HKTs)

Once higher-kinded types are available, the collection-specific `map` functions become instances of `Functor`:

```flux
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b) -> f<b>
}

instance Functor<List> {
    fn fmap(xs, f) { List.map(xs, f) }
}

instance Functor<Array> {
    fn fmap(xs, f) { Array.map(xs, f) }
}

instance Functor<Option> {
    fn fmap(opt, f) { Option.map_option(opt, f) }
}
```

#### Migration plan

| Phase | What happens | User impact |
|-------|-------------|-------------|
| Now | Flow stdlib follows the maintained static-typing path | None — existing code works |
| Step 6 (built-in classes) | Add typed signatures to Flow functions that have obvious types (e.g., `assert_eq<a: Eq>`) | None — signatures are additive |
| Step 7a | Split `Flow.List.map` / `Flow.Array.map` into separate typed functions | Import change: `import Flow.List exposing (map)` for list-specific map |
| Step 7b | Add `Foldable` class with `fold`, `length`, `to_list` methods | Explicit-import `Flow.Foldable` abstraction over existing List/Array implementations |
| Future (HKTs) | Add `Functor` class, unify `map` across all containers | `fmap` works on List, Array, Option generically |

#### Backward compatibility

During migration, the untyped `map`/`filter`/`fold` functions remain available as the auto-prelude defaults. Typed versions are opt-in via explicit imports:

```flux
// Old code — still works
fn main() with IO {
    let xs = map([1, 2, 3], \(x) -> x * 2)  // uses untyped Flow.List.map
    print(xs)
}

// New code — explicit typed import
import Flow.List exposing (map)
fn main() with IO {
    let xs = map([1, 2, 3], \(x: Int) -> x * 2)  // uses typed List.map
    print(xs)
}
```

The untyped versions are deprecated once typed alternatives cover all use cases, and removed in a future major version.

---

## What we are NOT implementing

| Feature | Reason |
|---------|--------|
| Multi-parameter type classes | ~~Adds ambiguity~~ — **Delivered in Proposal 0146** (type class hardening) |
| Functional dependencies | Only needed with multi-param classes; not yet needed |
| Associated types / type families | Huge complexity; not needed |
| Higher-kinded types (`Functor`, `Monad`) | ~~Separate proposal~~ — **Delivered** (kind system, `HktApp`, `Functor<List>` works) |
| Overlapping instances | Source of confusion; PureScript proves they're unnecessary |
| Deriving | Separate step after core classes work |
| `SPECIALIZE` / monomorphization | Optimization; dictionaries work first |
| Orphan instance warnings | Nice-to-have; not blocking |

---

## Syntax reference card

```flux
// ── Class declaration ──────────────────────────────────────────────
class ClassName<a> {
    fn method_name(x: a, y: a) -> ReturnType       // required
    fn default_method(x: a) -> ReturnType { body }  // has default
}

// ── Class with superclass ──────────────────────────────────────────
class SuperClass<a> => ClassName<a> {
    fn method(x: a) -> ReturnType
}

// ── Instance for concrete type ─────────────────────────────────────
instance ClassName<Int> {
    fn method_name(x, y) { implementation }
}

// ── Instance with context (constrained) ────────────────────────────
instance ClassName<a> => ClassName<List<a>> {
    fn method_name(xs, ys) { implementation }
}

// ── Constrained function ───────────────────────────────────────────
fn function_name<a: ClassName>(x: a, y: a) -> a {
    method_name(x, y)
}

// ── Multiple constraints ───────────────────────────────────────────
fn display_and_compare<a: Show, a: Ord>(x: a, y: a) -> String {
    if lt(x, y) { show(x) } else { show(y) }
}

// alternative syntax (comma-separated):
fn display_and_compare<a: Show + Ord>(x: a, y: a) -> String {
    if lt(x, y) { show(x) } else { show(y) }
}
```

---

## Error messages

### No instance found

```
error[E440]: No instance for Num<String>
  |
3 | let x = "hello" + "world"
  |                  ^ arising from a use of `+`
  |
  help: `+` requires `Num<a>`, but String does not implement Num.
        Did you mean `++` (string concatenation)?
```

### Ambiguous type variable

```
error[E441]: Ambiguous type variable `a`
  |
5 | print(from_int(0))
  |       ^^^^^^^^ type of `from_int(0)` could not be determined
  |
  help: Add a type annotation: `from_int(0) : Int`
        Default: the compiler would choose `Int` if this were the only constraint.
```

### Missing method in instance

```
error[E442]: Missing method `eq` in instance Eq<Color>
  |
8 | instance Eq<Color> {
  |          ^^^^^^^^^ `eq` is required but not implemented
  |
  help: Eq requires at minimum: fn eq(x: Color, y: Color) -> Bool
```

### Superclass not satisfied

```
error[E443]: No instance for Eq<Foo> (required by Ord<Foo>)
   |
12 | instance Ord<Foo> {
   |          ^^^^^^^^ Ord requires Eq as a superclass
   |
   help: Add an instance: instance Eq<Foo> { fn eq(x, y) { ... } }
```

---

## Prior art

- **Haskell/GHC**: The gold standard. Dictionary-passing elaboration, OutsideIn(X) solver. Flux follows the same model but without type families, GADTs, or overlapping instances.
- **PureScript**: Haskell-like with strict coherence (no overlapping instances). ~3K line solver. Good reference for a simpler-than-GHC implementation.
- **Rust traits**: Monomorphization instead of dictionaries. More efficient but requires separate compilation support. Not suitable for Flux's NaN-boxed runtime.
- **Koka**: No type classes; uses overloaded names with explicit dispatch. Simpler but less expressive.
