- Feature Name: Constrained Type Parameters and Instance Context Enforcement
- Start Date: 2026-04-08
- Status: Implemented
- Completion Date: 2026-04-14
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0145 (Type Classes), Proposal 0146 (Type Class Hardening)

## Summary

Close the two remaining gaps in Flux's type class system:

1. **Constrained type parameter syntax**: Parse and enforce `fn f<a: Eq>(x: a, y: a) -> Bool` so users can write explicitly constrained polymorphic functions.

2. **Instance context enforcement**: When `instance Eq<a> => Eq<List<a>>` is declared, enforce that methods in the instance body can only use `eq` on elements of type `a` because the `Eq<a>` context guarantees it.

Without these, the type class system works internally (HM inference emits constraints, dictionary elaboration passes dictionaries) but users cannot express constrained polymorphism in source syntax, and instance contexts are parsed but ignored at the method level.

## Implementation status

Last updated: 2026-04-14

`0147` is **implemented**.

Implemented:

- constrained generic parameter syntax parses
- constrained generic params are represented in the AST
- explicit-bound class constraints are emitted during function inference
- generalized schemes carry those constraints
- call sites re-emit generalized scheme constraints

Completed:

- full instance-context enforcement for contextual instances
- end-to-end dictionary threading through contextual instance methods
- contextual-instance lowering through Core/native paths with focused runtime and dump coverage

---

## Motivation

### Problem 1: No way to declare constrained functions

Today, a user who writes:

```flux
fn contains<a: Eq>(xs: List<a>, elem: a) -> Bool {
    match xs {
        [] -> false,
        [h | t] -> if eq(h, elem) { true } else { contains(t, elem) }
    }
}
```

gets a parse error:

```
error[E034]: Missing Generic Parameter List
  |
  | fn contains<a: Eq>(xs: List<a>, elem: a) -> Bool {
  |              ^ expected `>` to close generic parameter list
```

The parser sees `a:` and expects a hash literal, not a constraint. The only way to use class methods polymorphically is to rely on HM inference to infer the constraint — but then the function's type signature doesn't document the requirement, and callers have no compile-time feedback about what's needed.

### Problem 2: Instance contexts are decorative

Today, a user can write:

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
```

The `Eq<a> =>` context parses correctly, but the `eq(h1, h2)` call inside the body doesn't know that `h1: a` has an `Eq` instance. It either resolves monomorphically (if the type is concrete) or falls through to the polymorphic stub (which panics). The context constraint should provide the evidence that `eq` can be called on elements of type `a`.

---

## Guide-level explanation

### Constrained type parameters

Type parameters can declare class constraints using `:` syntax:

```flux
// Single constraint
fn contains<a: Eq>(xs: List<a>, elem: a) -> Bool {
    match xs {
        [] -> false,
        [h | t] -> if eq(h, elem) { true } else { contains(t, elem) }
    }
}

// Multiple constraints on the same variable
fn show_max<a: Ord + Show>(x: a, y: a) -> String {
    if gt(x, y) { show(x) } else { show(y) }
}

// Multiple constrained variables
fn convert_and_show<a: Show, b: Eq>(x: a, y: b) -> String {
    show(x)
}
```

When you call a constrained function, the compiler verifies at compile time that the concrete types satisfy the constraints:

```flux
contains([1, 2, 3], 2)       // OK: Eq<Int> exists
contains(["a", "b"], "c")    // OK: Eq<String> exists
contains([1.0, 2.0], 3.0)    // OK: Eq<Float> exists
```

If no instance exists, you get a compile error:

```
error[E444]: No instance for `Eq<Color>`
  |
  | contains(colors, red)
  |          ^^^^^^ `contains` requires `Eq<a>`, but `Color` has no `Eq` instance
  |
  help: Add `instance Eq<Color> { fn eq(x, y) { ... } }`
```

### Instance contexts

Instance contexts declare what class instances are available within the instance body:

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
```

The `Eq<a> =>` prefix means: "this instance exists for any `List<a>` where `a` itself has an `Eq` instance." Inside the body, `eq(h1, h2)` is valid because `h1: a` and the context guarantees `Eq<a>`.

At call sites, the compiler transitively resolves contexts:

```flux
eq([1, 2], [1, 2])            // Uses Eq<List<Int>>, which requires Eq<Int> ✓
eq([[1], [2]], [[1], [2]])     // Uses Eq<List<List<Int>>>, requires Eq<List<Int>>, requires Eq<Int> ✓
```

---

## Reference-level explanation

### Part 1: Constrained type parameter syntax

#### Parser changes

**File:** `src/syntax/parser/statement.rs`

In `parse_type_params_angle_bracket`, after parsing each identifier, check for `:` followed by a constraint name. Support `+` for multiple constraints.

```
type_param       = IDENT [ ":" constraint_list ]
constraint_list  = constraint ( "+" constraint )*
constraint       = IDENT
```

Parsing `<a: Eq>` produces a type parameter `a` with an associated constraint `Eq<a>`.

#### AST changes

**File:** `src/syntax/statement.rs`

The `Statement::Function` variant already has `type_params: Vec<Identifier>`. Add a parallel field:

```rust
Statement::Function {
    // ... existing fields ...
    type_param_constraints: Vec<(Identifier, Vec<Identifier>)>,  // [(a, [Eq, Show]), ...]
}
```

Alternatively, use a new type `TypeParam { name: Identifier, constraints: Vec<Identifier> }`.

#### Type inference integration

**File:** `src/ast/type_infer/function.rs`

During function inference, for each constrained type parameter:
1. After allocating a fresh type variable for the parameter, emit class constraints for each bound.
2. These constraints flow through the existing `collect_scheme_constraints` → `Scheme.constraints` pipeline.

This means constrained type params produce the same `SchemeConstraint`s that operator usage produces — the downstream dictionary elaboration works unchanged.

#### Constraint syntax not parsed on instance methods

Instance method declarations inside `instance` blocks do NOT use this syntax — their constraints come from the instance context (`Eq<a> =>` prefix), not from individual method signatures.

### Part 2: Instance context enforcement

#### What needs to change

**File:** `src/types/class_dispatch.rs`

When generating mangled instance functions for a contextual instance like `instance Eq<a> => Eq<List<a>>`, the methods need access to the context's class methods. Currently, `eq(h1, h2)` inside the instance body doesn't resolve because `h1: a` is a type variable with no known instance.

Two approaches:

**Approach A — Dictionary threading (preferred):**

The context constraint `Eq<a>` means the instance method receives an implicit dictionary parameter. During dispatch generation, the mangled function `__tc_Eq_List_eq` gets an extra dictionary parameter for the `Eq<a>` context:

```
__tc_Eq_List_eq(__dict_Eq_a, xs, ys) {
    // eq(h1, h2) → __dict_Eq_a.eq(h1, h2)
}
```

At call sites like `eq([1, 2], [1, 2])`, the compiler passes `__dict_Eq_Int` as the context dictionary.

**Approach B — Recursive instance resolution:**

When resolving `eq(h1, h2)` inside the `Eq<List<a>>` instance, recognize that `a` has an `Eq` constraint from the instance context. Resolve `eq` calls on `a`-typed values through the context's dispatch chain.

#### Instance type argument validation

**File:** `src/types/class_env.rs`

Add validation that the number of type arguments in an instance matches the number of type parameters declared in the class:

```rust
if type_args.len() != class_def.type_params.len() {
    diagnostics.push(/* E447: Wrong number of type arguments */);
}
```

---

## Implementation plan

### Step 1: Parse constrained type parameters — DONE

- [x] Extend `parse_type_params_angle_bracket` to handle `a: Constraint` syntax
- [x] Support `+` for multiple constraints: `a: Eq + Show` (parses; `show` unavailable because builtin class polymorphic stubs are suppressed — separate issue)
- [x] Store constraints alongside type params in `Statement::Function` via `FunctionTypeParam { name, constraints }`
- [x] Thread through Core IR and CFG — dict elaboration handles constrained functions end-to-end

### Step 2: Emit constraints from parsed annotations — DONE

- [x] During function inference, emit `WantedClassConstraint` for each parsed constraint
- [x] Constraints flow through `collect_scheme_constraints` into `Scheme.constraints`
- [x] Dictionary elaboration prepends dict params to constrained function bodies
- [x] Call-site resolution passes concrete dictionaries (`__dict_Eq_Int`, etc.)

### Step 3: Instance context enforcement — DONE

- [x] Thread context constraints from `InstanceDef.context` into mangled function generation
- [x] Add dictionary parameters to contextual instance methods
- [x] At call sites, resolve and pass context dictionaries

**Current state**: contextual instances are fully semantic in the maintained paths. The generated mangled instance methods receive their context dictionaries, Core lowering resolves contextual dictionary arguments, and focused tests cover runtime dispatch, Core dumps, strict typing, and native lowering.

**Verification**:

- `tests/constrained_type_params_integration.rs`
- `tests/static_type_validation_tests.rs`
- `tests/ir_pipeline_tests.rs`
- `tests/llvm_type_class.rs`

### Step 4: Instance type argument count validation — DONE

- [x] E447 error for mismatched type argument count
- [x] E448 error for mismatched method arity
- [x] Validated during `collect_instances`

---

## Drawbacks

1. **Constrained type param syntax** adds parsing complexity. The `:` character is already used for hash literals and type annotations, which could cause ambiguity in edge cases.

2. **Instance context enforcement** via dictionary threading adds runtime parameters to contextual instance methods. For deeply nested types like `Eq<List<List<List<Int>>>>`, this creates a chain of dictionary passes. GHC handles this with aggressive specialization — Flux would need similar optimization.

---

## Prior art

- **Haskell:** `f :: Eq a => a -> a -> Bool` is the standard constrained function syntax. Instance contexts are fully enforced — `instance Eq a => Eq [a]` requires the context dictionary for element comparisons. GHC's dictionary elaboration handles this seamlessly.

- **PureScript:** Same syntax and semantics as Haskell. No overlapping instances, strict coherence. Instance contexts are required and enforced.

- **Rust:** `fn f<T: Eq>(x: T, y: T) -> bool` — trait bounds. Monomorphized at compile time (no dictionaries). Instance contexts correspond to `where` clauses: `impl<T: Eq> Eq for Vec<T>`.

- **Scala:** Context bounds via `def f[A: Eq](x: A)` or `using` clauses. Similar dictionary-passing semantics to Haskell.

---

## Unresolved questions

1. **Syntax for constrained type params:** Should it be `<a: Eq>` (Rust-style) or `<a>` with a separate `where` clause? The Rust-style is more concise for single constraints; a `where` clause scales better for complex constraints.

2. **Error reporting for context violations:** When `eq(h1, h2)` fails inside a contextual instance body, should the error mention the missing context, or should the context automatically provide evidence?

3. **Interaction with operator desugaring:** If `==` desugars to `eq`, and `eq` inside a contextual instance requires a context dictionary, the operator desugaring path needs to thread context dictionaries too.

---

## Future possibilities

### Covered by this proposal

| Feature | Current state | Target | Notes |
|---------|--------------|--------|-------|
| **Constrained type syntax** | `fn f<a: Eq>` doesn't parse | `fn f<a: Eq>(x: a) -> Bool` works | Step 1-2 |
| **Instance contexts** | Parsed, not enforced | `instance Eq<a> => Eq<List<a>>` provides evidence in body | Step 3 |

### Future proposals (out of scope)

| Feature | Current state | Haskell equivalent | Difficulty | Notes |
|---------|--------------|-------------------|------------|-------|
| **Strategy-based deriving** | Structural `deriving` only | `deriving (Eq, Show, Ord, ...)` with per-class generation | Medium | Needs codegen for each derivable class: structural equality for `Eq`, field-by-field comparison for `Ord`, field printing for `Show`. Context constraints (`Eq<a> =>`) should be auto-added for parameterized types. |
| **Functional dependencies** | No | `FunDeps` extension | Large | Resolves ambiguity in multi-param classes: `class Convert<a, b> \| a -> b` means `a` determines `b`. Without this, `Convert<Int, String>` and `Convert<Int, Float>` are ambiguous at call sites where only `a` is known. |
| **Associated types** | No | `TypeFamilies` extension | Large | Type-level functions within classes: `class Collection<c> { type Elem; fn empty() -> c }`. More ergonomic than multi-param + fundeps for container patterns. Requires kind-level reasoning. |
| **Overlapping instances** | No (by design) | `OverlappingInstances` pragma | Small | Deliberately omitted following PureScript's approach — overlapping instances break coherence and make reasoning about dispatch harder. Could be added as an opt-in pragma if needed. |
| **Orphan instance control** | No | Warnings for orphan instances | Small | An orphan instance is one defined in a module that owns neither the class nor the type. Without control, different modules can define conflicting instances. Add a warning (or error under `--strict`) for orphan instances. |
| **SPECIALIZE / monomorphization** | No | `{-# SPECIALIZE #-}` pragma | Medium | Dictionary indirection has a runtime cost. Monomorphization creates specialized copies of polymorphic functions for specific types, eliminating the dictionary. Could be automatic (based on call-site analysis) or pragma-driven. |
| **`where` clauses** | No | `f :: ... => ... where ...` | Small | More readable than inline constraints for complex signatures: `fn f<a, b>(x: a) -> b where a: Show, b: Eq`. |
| **Constraint aliases** | No | `type Printable a = (Show a, Eq a)` | Small | Reusable constraint bundles for common combinations. |
| **Quantified constraints** | No | `QuantifiedConstraints` extension | Large | `forall a. Eq<a> => Eq<List<a>>` as a first-class constraint. Enables more expressive instance declarations. |
