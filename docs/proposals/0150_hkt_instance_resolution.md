- Feature Name: HKT Instance Resolution
- Start Date: 2026-04-08
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0145 Steps 1–5 (done), HKT kind system (done)
- Status: Complete
- Date: 2026-04-09

## Summary

Enable compile-time instance resolution for higher-kinded type class parameters. `instance Functor<List>` should resolve when `fmap` is called with a `List<Int>` argument. This unblocks `Functor`, `Foldable`, and other HKT type classes.

## Implementation status

Last updated: 2026-04-09

Proposal 0150 is complete.

- `ClassEnv::match_instance_type_expr` now treats a bare constructor instance head like `List` as a match for `Con(List)`, `App(List, [...])`, and `HktApp(Con(List), [...])`.
- Focused resolver unit tests cover applied-list matching, `HktApp`, multi-argument constructors, mismatch rejection, and explicit-argument structural matching.
- End-to-end IR pipeline tests prove `Functor<List>` both lowers to `__tc_Functor_List_fmap` and executes successfully at runtime.

## Motivation

Flux already has the infrastructure for higher-kinded types:
- The kind system supports `Kind::Arrow` (`Type → Type`)
- `InferType::HktApp` represents applied HKT types
- `class Functor<f>` and `instance Functor<List>` parse correctly
- The `__tc_Functor_List_fmap` mangled function is generated from the instance declaration

Before this proposal, instance resolution failed at runtime:

```flux
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b): f<b>
}

instance Functor<List> {
    fn fmap(xs, func) { map(xs, func) }
}

fn main() with IO {
    let doubled = fmap([1, 2, 3], \x -> x * 2)
    print(doubled)  // panic: No instance of Functor.fmap for the given type
}
```

The failure occurred because `try_resolve_class_call` could not match the call-site argument type `List<Int>` against the instance type parameter `List`.

### Root cause

The instance resolver (`resolve_instance_with_subst` in `class_env.rs`) compares:

- **Instance pattern**: `Functor<List>` — type arg is `TypeExpr::Named { name: "List", args: [] }`
- **Actual type**: first arg of `fmap([1,2,3], ...)` has HM type `App(List, [Int])`

The old matcher at `match_instance_type_expr` checked `args.len() == actual_args.len()` — but the instance pattern had 0 args (`List` bare) while the actual had 1 (`List<Int>` applied). This length mismatch caused the match to fail.

The resolver did not understand that `List` as a **kind `Type → Type`** parameter should match the **constructor** of `App(List, [Int])`, not the fully-applied type.

### What this unblocks

- `Functor<List>`, `Functor<Array>`, `Functor<Option>` — unified `fmap` across containers
- `Foldable<List>`, `Foldable<Array>` — unified `fold`, `length`, `to_list` (Proposal 0145 Step 7b)
- Any user-defined HKT class: `Monad`, `Traversable`, `Applicative`, etc.

## Guide-level explanation

After this proposal, HKT type classes work end-to-end:

```flux
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b): f<b>
}

instance Functor<List> {
    fn fmap(xs, func) { map(xs, func) }
}

instance Functor<Option> {
    fn fmap(opt, func) { map_option(opt, func) }
}

fn main() with IO {
    print(fmap([1, 2, 3], \x -> x * 2))       // [2, 4, 6]
    print(fmap(Some(5), \x -> x + 1))          // Some(6)
}
```

The compiler resolves `fmap([1, 2, 3], ...)` by:
1. Seeing the first argument has type `List<Int>`
2. Decomposing `List<Int>` into constructor `List` + applied arg `Int`
3. Matching constructor `List` against instance `Functor<List>` ✓
4. Calling `__tc_Functor_List_fmap([1, 2, 3], \x -> x * 2)`

## Reference-level explanation

### Change 1: HKT decomposition in `match_instance_type_expr`

**File**: `src/types/class_env.rs`, function `match_instance_type_expr`

The old code handled `TypeExpr::Named { name, args: [] }` (bare constructor like `List`) matched against `InferType::App(tc, actual_args)` like this:

```rust
// Current: requires args.len() == actual_args.len()
// For List (0 args) vs App(List, [Int]) (1 arg) → FAILS
```

**Fix**: When the pattern is a bare type constructor (no args, starts with uppercase) and the actual type is `App(tc, args)` or `HktApp(Con(tc), args)`, match by comparing just the constructor:

```rust
TypeExpr::Named { name, args, .. } if args.is_empty() => match actual {
    // Exact match: bare constructor vs bare constructor
    InferType::Con(tc) => Self::type_constructor_matches(*name, tc, interner),
    // HKT decomposition: bare constructor `List` matches `App(List, [Int])`
    // by comparing just the constructor, ignoring applied args.
    InferType::App(tc, _) => Self::type_constructor_matches(*name, tc, interner),
    InferType::HktApp(head, _) => match head.as_ref() {
        InferType::Con(tc) => Self::type_constructor_matches(*name, tc, interner),
        _ => false,
    },
    _ => false,
}
```

This is safe because the kind system already ensures that `Functor<List>` is well-kinded — `f` has kind `Type → Type`, and `List` has kind `Type → Type`. The match only needs to verify the constructor identity.

### Change 2: No caller-side stripping needed

**File**: `src/core/lower_ast/mod.rs`, function `try_resolve_class_call`

`try_resolve_class_call` continues to pass the first argument's full type `App(List, [Int])` to `resolve_instance_with_subst`. The resolver now handles the decomposition internally, so no caller-side stripping is needed.

This keeps the change localized to the instance resolver and preserves existing mangling and dictionary lookup behavior.

### Change 3: Dictionary construction for HKT instances

**File**: `src/core/passes/dict_elaborate.rs`

The dictionary elaboration pass constructs `__dict_Functor_List` as `MakeTuple(__tc_Functor_List_fmap)`. This already works because mangling and dictionary naming continue to derive their key from the matched instance declaration's type args (`List`, not `List_Int`).

### What does NOT change

- Parser: `class Functor<f>` and `instance Functor<List>` already parse
- Kind system: already infers `f :: Type → Type`
- `HktApp` type: already exists in `InferType`
- Dispatch generation: `generate_from_statements` already processes HKT instances and generates `__tc_Functor_List_fmap`
- Mangled names: `__tc_Functor_List_fmap` already constructed correctly

## Drawbacks

1. **Ambiguity with bare constructors**: The match `TypeExpr::Named { name: "List", args: [] }` could be either a type variable or a bare constructor in non-HKT contexts. The `is_instance_type_var` check (lowercase first letter) already distinguishes these.

2. **No kind checking during instance resolution**: The fix matches by constructor name without verifying kinds. A malformed instance like `instance Functor<Int>` (where `Int` has kind `Type`, not `Type → Type`) would match incorrectly. This should be caught by kind checking during instance validation, which is a separate concern.

## Rationale and alternatives

### Why this design?

The shipped fix is minimal — one focused matcher change in `match_instance_type_expr` plus regression coverage. It leverages the existing infrastructure (kind system, HktApp, mangled name generation) without architectural changes.

### Alternatives

**Full kind-directed matching**: Instead of pattern-matching on `App(tc, _)` to extract the constructor, use the kind system to decompose types at the kind level. More principled but requires threading kind information through the instance resolver. Can be done as a follow-up.

**Explicit HKT syntax**: Require `instance Functor<List<_>>` with an explicit wildcard for applied args. Avoids ambiguity but differs from Haskell's established syntax.

## Prior art

### GHC (Haskell)
GHC resolves HKT instances by unifying `f ~ List` at the kind level during constraint solving. The instance head `Functor List` (where `List :: * -> *`) matches `Functor f` when `f` is instantiated to a type constructor of the right kind.

### PureScript
PureScript uses explicit kind annotations and resolves HKT instances during type checking, similar to GHC.

## Unresolved questions

1. **Multi-arg HKT instances**: `instance Bifunctor<Either>` where `Either` has kind `Type → Type → Type`. The decomposition needs to handle `App(Either, [String, Int])` matching `Either`. The proposed fix handles this naturally since it compares just the constructor.

2. **Nested HKT**: `instance Functor<Compose<F, G>>` where `Compose` is itself an HKT. This requires deeper structural matching and is out of scope for this proposal.

3. **Overlapping HKT instances**: `instance Functor<List>` and `instance Functor<f>` (a catch-all). Instance overlap checking needs to account for HKT parameters. Out of scope — Flux currently forbids overlapping instances.

## Future possibilities

1. **Kind-directed instance resolution**: Use the kind system to guide matching, enabling more precise resolution and better error messages for kind mismatches.

2. **Deriving for HKT classes**: `deriving Functor` could auto-generate `fmap` for ADTs with a single type parameter.

3. **Monad / Applicative tower**: With `Functor` working, `Applicative` and `Monad` become implementable, enabling do-notation desugaring for monadic effects.
