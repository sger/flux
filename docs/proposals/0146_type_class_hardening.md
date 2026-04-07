- Feature Name: Type Class Hardening
- Start Date: 2026-04-07
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0145 (Type Classes MVP)
- Status: Draft
- Date: 2026-04-07

## Summary

Harden the type class implementation by closing five gaps left open in the MVP (Proposal 0145): runtime dispatch for polymorphic calls, superclass constraint enforcement, structural duplicate instance detection, extra method validation, and multi-parameter type class support.

These are distinct from the dictionary elaboration work (0145 Step 5) — they fix correctness and robustness issues in the existing static dispatch path.

---

## Motivation

The type class MVP (0145) delivers compile-time dispatch for monomorphic call sites. However, several edges are unguarded:

1. **Polymorphic calls panic at runtime.** A function like `fn apply_eq<a: Eq>(x: a, y: a) { eq(x, y) }` compiles but crashes because the dispatch stub's body is `panic("No instance")`. Users get no compile-time warning — just a runtime `STATUS_ACCESS_VIOLATION` or panic.

2. **Superclass constraints are parsed but ignored.** Writing `class Eq<a> => Ord<a>` parses without error, but declaring `instance Ord<Foo>` without a corresponding `instance Eq<Foo>` is silently accepted. This breaks the type class contract.

3. **Duplicate instance detection uses `format!("{:?}")`.** Two instances with structurally identical type arguments but different `Debug` representations could bypass the check, or conversely, two different types with the same debug string could falsely conflict.

4. **Extra methods in instances are silently accepted.** An instance can define methods that don't exist in the class — these become unreachable dead code with no warning.

5. **Only single type parameter classes are supported.** `type_param` takes `first().unwrap_or(*name)`, silently discarding additional parameters. Multi-parameter type classes (e.g., `class Convert<a, b>`) are needed for common patterns like type conversion.

---

## Guide-level explanation

After this proposal, the following behaviors change:

### Polymorphic dispatch produces a compile-time error (or works)

```flux
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) { x == y }
}

// Before: compiles, panics at runtime if called with non-Int
// After (option A): compile error "cannot resolve Eq<a> — polymorphic dispatch not yet supported"
// After (option B): dictionary passing makes this work
fn are_equal<a: Eq>(x: a, y: a) -> Bool {
    eq(x, y)
}
```

### Superclass constraints are enforced

```flux
class Eq<a> => Ord<a> {
    fn compare(x: a, y: a) -> Int
}

instance Ord<Color> {           // ERROR: no instance Eq<Color>
    fn compare(x, y) { 0 }
}
```

Error message:
```
error[E445]: Missing superclass instance
  |
  | instance Ord<Color> {
  |          ^^^^^^^^^ `Ord` requires `Eq<Color>`, but no such instance exists
  |
  help: Add `instance Eq<Color> { fn eq(x, y) { ... } }` first.
```

### Extra methods in instances are rejected

```flux
class Sizeable<a> {
    fn size(x: a) -> Int
}

instance Sizeable<Int> {
    fn size(x) { x }
    fn weight(x) { x * 2 }     // ERROR: `weight` is not a method of `Sizeable`
}
```

Error message:
```
error[E446]: Unknown instance method
  |
  | fn weight(x) { x * 2 }
  |    ^^^^^^ `weight` is not a method of class `Sizeable`
  |
  help: `Sizeable` declares: fn size(x: a) -> Int
```

### Multi-parameter type classes

```flux
class Convert<a, b> {
    fn convert(x: a) -> b
}

instance Convert<Int, Float> {
    fn convert(x) { to_float(x) }
}

instance Convert<Int, String> {
    fn convert(x) { to_string(x) }
}
```

---

## Reference-level explanation

### Track 1: Polymorphic dispatch

**Current state:** `generate_polymorphic_stub()` in `class_dispatch.rs` emits a function whose body either delegates to `__rt_{method}` (if the symbol exists) or panics. The `__rt_` functions are never generated (the `generate_dispatch_function` and `generate_polymorphic_dispatch` functions are `#[allow(dead_code)]`).

**Option A — Error on unresolved polymorphic calls:**

During constraint solving (Step 4, `class_constraint.rs`), if a constraint's type argument is still a type variable after substitution, emit:

```
error[E447]: Unresolved type class constraint
  |
  | fn apply_eq<a: Eq>(x: a, y: a) { eq(x, y) }
  |                                   ^^ cannot resolve `Eq<a>` at compile time
  |
  note: polymorphic type class dispatch is not yet supported
  help: specify a concrete type, e.g., `eq(x: Int, y: Int)`
```

This makes the limitation explicit instead of producing a runtime panic.

**Option B — Enable runtime dispatch:**

Re-enable `generate_polymorphic_dispatch()` or `generate_dispatch_function()` in `class_dispatch.rs` (currently dead code). This generates `type_of()` chains for runtime resolution:

```flux
// Generated for eq with instances Eq<Int> and Eq<String>:
fn eq(__x0, __x1) {
    if type_of(__x0) == "Int" { __tc_Eq_Int_eq(__x0, __x1) }
    else if type_of(__x0) == "String" { __tc_Eq_String_eq(__x0, __x1) }
    else { panic("No instance for eq") }
}
```

The TODO at `class_dispatch.rs:79` notes a type inference issue blocking this — the `Any`-typed dispatch params conflict with HM inference. This needs investigation.

**Option C — Dictionary passing (0145 Step 5):**

Full GHC-style elaboration. Most correct but highest effort. See 0145 Step 5 for the design.

**Recommendation:** Implement Option A first (cheap, safe), then Option C when dictionary elaboration is ready. Option B is a bridge but adds runtime cost and `type_of()` dependency.

#### Files affected

- `src/types/class_dispatch.rs` — un-`dead_code` and wire up dispatch functions (Option B), or add error for unresolved constraints (Option A)
- `src/ast/type_infer/class_constraint.rs` — emit E447 for unsolved type-variable constraints
- `src/diagnostics/compiler_errors.rs` — add E447

### Track 2: Superclass constraint enforcement

**Current state:** `parse_class_statement()` in `parser/statement.rs:1354-1366` detects the `=>` position but always returns `(vec![], first_name, first_args)` — superclasses are discarded. `ClassDef.superclasses` is always empty.

**Implementation:**

1. **Parser:** Implement `=>` detection. Since Flux has no `=>` token, use two-token lookahead: if current is `=` and peek is `>`, consume both as fat arrow. Alternatively, add `FatArrow` to `TokenType`.

2. **ClassEnv validation:** In `collect_instances()`, after validating the class exists and methods are present, check superclass satisfaction:

```rust
// For each superclass constraint in the class definition:
for superclass in &class_def.superclasses {
    // Check that an instance exists for the same head type
    let super_instance = env.resolve_instance_for_type(
        superclass.class_name,
        &type_name,
        interner,
    );
    if super_instance.is_none() {
        diagnostics.push(/* E445: Missing superclass instance */);
    }
}
```

3. **Error code:** Add E445 `MISSING_SUPERCLASS_INSTANCE`.

#### Files affected

- `src/syntax/parser/statement.rs` — parse `=>` in class head
- `src/syntax/lexer/token_type.rs` — (optional) add `FatArrow` token
- `src/types/class_env.rs` — validate superclass instances in `collect_instances()`
- `src/diagnostics/compiler_errors.rs` — add E445

### Track 3: Structural duplicate instance detection

**Current state:** `class_env.rs:205-208`:
```rust
let is_duplicate = env.instances.iter().any(|existing| {
    existing.class_name == *class_name
        && format!("{:?}", existing.type_args) == format!("{:?}", type_args)
});
```

This compares `Debug` string representations, which is fragile.

**Fix:** Add a structural equality function for `Vec<TypeExpr>`:

```rust
fn type_args_equal(a: &[TypeExpr], b: &[TypeExpr], interner: &Interner) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| type_expr_equal(x, y, interner))
}

fn type_expr_equal(a: &TypeExpr, b: &TypeExpr, interner: &Interner) -> bool {
    match (a, b) {
        (TypeExpr::Named { name: n1, args: a1, .. },
         TypeExpr::Named { name: n2, args: a2, .. }) => {
            n1 == n2 && type_args_equal(a1, a2, interner)
        }
        (TypeExpr::Function { params: p1, return_type: r1, .. },
         TypeExpr::Function { params: p2, return_type: r2, .. }) => {
            type_args_equal(p1, p2, interner)
                && type_expr_equal(r1, r2, interner)
        }
        _ => false,
    }
}
```

Also replace the similar pattern in `register_builtin_instance()` (line 466).

#### Files affected

- `src/types/class_env.rs` — replace `format!("{:?}")` comparisons with structural equality
- `src/syntax/type_expr.rs` — (optional) implement `PartialEq` for `TypeExpr` ignoring spans

### Track 4: Extra method validation

**Current state:** `collect_instances()` checks that all required methods are present, but does not check for methods that aren't part of the class.

**Fix:** After the missing-method check in `collect_instances()`, add:

```rust
for method in methods {
    let is_known = class_def.methods.iter().any(|m| m.name == method.name);
    if !is_known {
        let display_class = interner.resolve(*class_name);
        let display_method = interner.resolve(method.name);
        diagnostics.push(
            diagnostic_for(&INSTANCE_EXTRA_METHOD)
                .with_span(method.span)
                .with_message(format!(
                    "`{display_method}` is not a method of class `{display_class}`."
                ))
                .with_hint_text(format!(
                    "`{display_class}` declares: {}",
                    class_def.methods.iter()
                        .map(|m| interner.resolve(m.name).to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
        );
    }
}
```

**Error code:** Add E446 `INSTANCE_EXTRA_METHOD`.

**Severity:** This should be a warning, not an error — extra methods are harmless dead code but likely indicate a typo or misunderstanding.

#### Files affected

- `src/types/class_env.rs` — add extra method check in `collect_instances()`
- `src/diagnostics/compiler_errors.rs` — add E446

### Track 5: Multi-parameter type classes

**Current state:** `ClassDef.type_param` stores only the first type parameter:
```rust
let type_param = type_params.first().copied().unwrap_or(*name);
```

Additional parameters are silently discarded.

**Implementation:**

1. **Change `ClassDef.type_param` to `type_params: Vec<Identifier>`.**

2. **Update mangling:** Currently `__tc_{Class}_{Type}_{method}`. With multiple params: `__tc_{Class}_{Type1}_{Type2}_{method}`.

3. **Update instance matching:** `resolve_instance_for_type()` currently matches against a single type arg. Multi-param matching requires checking all type args.

4. **Update constraint generation:** Constraints must carry multiple type args: `ClassConstraintWanted { type_args: Vec<InferType> }`.

5. **HM inference impact:** Multi-param classes introduce ambiguity (which parameter determines the instance?). Haskell solves this with functional dependencies. For the initial implementation, require that the first parameter determines the instance (no fundeps).

#### Files affected

- `src/types/class_env.rs` — `ClassDef.type_param` → `type_params`, update all consumers
- `src/types/class_dispatch.rs` — update mangling for multi-param
- `src/core/lower_ast/mod.rs` — update `try_resolve_class_call()`
- `src/ast/type_infer/class_constraint.rs` — multi-arg constraints

---

## Implementation order

| Priority | Track | Effort | Risk |
|----------|-------|--------|------|
| **P0** | Track 3: Structural duplicate detection | Small | None — pure bugfix |
| **P0** | Track 4: Extra method validation | Small | None — additive warning |
| **P1** | Track 1 Option A: Error on unresolved polymorphic | Small | Low — makes implicit failure explicit |
| **P1** | Track 2: Superclass enforcement | Medium | Low — parser change + validation |
| **P2** | Track 5: Multi-param type classes | Large | Medium — touches inference, mangling, resolution |
| **P3** | Track 1 Option C: Dictionary elaboration | Large | Medium — see 0145 Step 5 |

P0 tracks are safe to implement immediately — they fix incorrect behavior without changing any working code paths. P1 tracks improve correctness guarantees. P2/P3 are feature additions.

---

## Drawbacks

1. **Track 1 Option A** makes some programs that "work" today into compile errors. However, those programs only work by accident (monomorphic call sites) and would panic on polymorphic use.

2. **Track 5** (multi-param classes) adds complexity to the constraint solver and mangling scheme. Haskell's experience shows that multi-param classes without functional dependencies lead to ambiguity errors. We may want to defer this until there are concrete use cases.

3. **Track 4** (extra method warning) could be noisy if users intentionally define helper functions inside instance blocks. However, instance blocks should only contain class methods — helper functions belong outside.

---

## Rationale and alternatives

- **Why not just do dictionary elaboration (0145 Step 5)?** Dictionary elaboration solves Track 1 completely but is a much larger change. The tracks in this proposal are incremental fixes that improve correctness now. They are also prerequisites — superclass enforcement and multi-param support are needed regardless of the dispatch mechanism.

- **Why structural equality instead of `PartialEq` derive for `TypeExpr`?** `TypeExpr` contains `Span` fields which should be ignored for equality. A derived `PartialEq` would compare spans, giving incorrect results. A custom implementation that ignores spans is more correct.

- **Why warn on extra methods instead of error?** Making it an error is stricter, but would break any existing code that uses extra methods in instances. A warning is safer for migration. It can be promoted to an error under `--strict-types`.

---

## Prior art

- **GHC:** Enforces all of these — superclass constraints, no extra methods (without `InstanceSigs`), structural instance matching, multi-param classes (with `MultiParamTypeClasses` extension).
- **PureScript:** Strict coherence — no overlapping instances, no orphans, superclass enforcement. Single-param only in early versions, multi-param added later.
- **Rust:** `impl` blocks can contain methods not in the trait (inherent methods), but trait impl blocks cannot. Extra methods in trait impls are a hard error.

---

## Unresolved questions

1. Should Track 4 (extra methods) be a warning or error? Under `--strict-types` it could be an error, with a warning in permissive mode.

2. For Track 5, should we require functional dependencies for multi-param classes, or is first-parameter-determines-instance sufficient?

3. For Track 1 Option A, what's the right error code? E447 is proposed but the existing E444 ("No type class instance") is close — should unresolved-polymorphic be a subcase of E444?

---

## Future possibilities

- **Functional dependencies:** `class Convert<a, b> | a -> b` — the type `a` determines `b`. Needed for unambiguous multi-param resolution.
- **Associated types:** `class Collection<c> { type Elem; ... }` — type-level functions within classes. More ergonomic than multi-param + fundeps for many use cases.
- **Deriving strategies:** `deriving (Eq, Show)` with auto-generation of instance bodies from ADT structure.
- **Instance chains / backtracking:** Overlapping instances with explicit priority. PureScript deliberately omits this; Haskell has it as a pragma.
