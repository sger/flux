- Feature Name: Module-Scoped Type Classes
- Start Date: 2026-04-09
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0145, Proposal 0150
- Status: Draft
- Date: 2026-04-09

## Summary

Introduce module-scoped type class syntax so classes and instances can live directly inside a module declaration and their methods are accessed through the module namespace. This replaces the current split model where `class` / `instance` are semantic top-level declarations while `module ... {}` is only an import-identity shell.

Target shape:

```flux
module Flow.Foldable {
    public class Foldable<f> {
        fn fold<a, b>(x: f<a>, init: b, func: (b, a) -> b): b
        fn length<a>(x: f<a>) -> Int
        fn to_list<a>(x: f<a>) -> List<a>
    }

    instance Foldable<List> {
        fn fold(xs, init, func) { ... }
        fn length(xs) { ... }
        fn to_list(xs) { ... }
    }

    instance Foldable<Array> {
        fn fold(xs, init, func) { ... }
        fn length(xs) { ... }
        fn to_list(xs) { ... }
    }
}
```

Usage:

```flux
import Flow.Foldable as Foldable

Foldable.fold([1, 2, 3], 0, fn(acc, x) { acc + x })
Foldable.length([1, 2, 3])
Foldable.to_list([|1, 2, 3|])
```

## Motivation

Current behavior has three problems:

- Module files historically rejected `class` and `instance`, forcing awkward file shapes.
- Class methods are treated as global semantic names, which collides with existing global helpers like `fold`, `length`, and `to_list`.
- `module Flow.Foldable { public fn loaded() { () } }` is only a workaround for import identity, not a coherent language feature.

This proposal makes modules the namespace boundary for type classes, matching the model users expect from Haskell-style module qualification.

## Goals

- Allow `class` and `instance` declarations inside module bodies as first-class members.
- Resolve class methods through module qualification, for example `Foldable.fold(...)`.
- Keep instance resolution type-driven under the hood.
- Avoid unqualified method-name collisions with legacy globals and prelude helpers.
- Preserve the existing `AST -> Core -> cfg` pipeline.

## Non-Goals

- No new semantic IR.
- No AST fallback in JIT or backend-specific hacks.
- No class-qualified syntax such as `Foldable::fold` or `Foldable.fold` where `Foldable` is the class rather than the module alias.
- No automatic migration of legacy global `fold`, `length`, `to_list` in this proposal.
- No change to HKT resolution semantics already delivered by `0150`.

## Proposed Syntax

### Module-Scoped Class Declaration

```flux
module Flow.Foldable {
    public class Foldable<f> {
        fn fold<a, b>(x: f<a>, init: b, func: (b, a) -> b): b
        fn length<a>(x: f<a>) -> Int
    }
}
```

Rules:

- `class` is valid inside a module body.
- `public class` exports the class and its method names through the module.
- Non-public classes remain module-private.

### Module-Scoped Instance Declaration

```flux
module Flow.Foldable {
    instance Foldable<List> {
        fn fold(xs, init, func) { ... }
        fn length(xs) { ... }
    }
}
```

Rules:

- `instance` is valid inside a module body.
- Instances are not "public/private" in the same sense as functions; they are available when the defining module is imported and loaded.
- Instance lookup remains global once the defining module is present in the compilation graph.

### Method Access

Preferred:

```flux
import Flow.Foldable as Foldable

Foldable.fold(xs, init, func)
Foldable.length(xs)
```

Not supported by this proposal:

- bare `fold(xs, ...)` as the primary `Foldable` API
- class-qualified method syntax detached from modules

## Semantic Model

### 1. Classes Belong to a Module Namespace

A class declared inside `module Flow.Foldable` has:

- semantic identity: `Flow.Foldable.Foldable`
- exported method surface: `Flow.Foldable.fold`, `Flow.Foldable.length`, `Flow.Foldable.to_list`

Internally, the class still has a canonical class symbol, but it is associated with the owning module.

### 2. Methods Are Module-Owned Names

Method names are resolved as module members first when written as:

```flux
Foldable.fold(...)
```

That means:

- `Foldable` is the imported module alias
- `fold` is a public class method exported by that module
- the compiler resolves the first argument type and chooses the correct instance method

This avoids conflicts with global `fold`.

### 3. Instances Stay Type-Directed

`instance Foldable<List>` and `instance Foldable<Array>` still feed the same class environment and resolver machinery:

- class matching
- dictionary construction
- compile-time monomorphic dispatch
- dictionary elaboration for polymorphic cases

The proposal changes lookup and namespacing, not the instance-selection algorithm.

## Resolution Rules

### Unqualified Name Resolution

For now:

- Existing global functions continue to win for bare names like `fold(...)`.
- Module-scoped class methods are intended to be used via qualification: `Foldable.fold(...)`.

This keeps backward compatibility.

### Qualified Name Resolution

For `Alias.method(...)`:

1. Resolve `Alias` to an imported module.
2. Check whether `method` is an exported function member.
3. Check whether `method` is an exported class method member.
4. If it is a class method:
   - infer the first argument type
   - attempt compile-time instance resolution
   - lower to mangled instance function when monomorphic
   - otherwise use dictionary elaboration

### Export Behavior

A `public class Foldable<f>` exports:

- the class itself for constraints and instance declarations
- its method names as module members

Example:

```flux
import Flow.Foldable as F

fn sum_list(xs) {
    F.fold(xs, 0, fn(acc, x) { acc + x })
}
```

## AST Changes

Current AST already supports `Statement::Class` and `Statement::Instance`. This proposal formalizes them as legal module members.

Changes:

- module-body validation must accept `Class` and `Instance`
- module-file validation must accept top-level `class` / `instance` only when they semantically belong to the file's module, or preferably require them to appear inside the module body after migration
- member collection must record exported class methods as module members

Preferred end state:

- class and instance declarations live inside `Statement::Module { body }`
- top-level class/instance outside module bodies becomes legacy or deprecated

## Compiler Changes

### Phase 1: Parsing and Validation

- Allow `class` and `instance` inside module bodies.
- Update module-content diagnostics to treat them as valid members.
- Decide whether top-level `class` / `instance` in module files remain temporarily supported for migration.

### Phase 2: Module Member Collection

- Extend module member collection to record public class methods as exported module members.
- A module should expose both:
  - ordinary functions
  - class methods from `public class` declarations

### Phase 3: Class Environment Identity

- Track owning module for each collected class definition.
- Track method ownership as `(module_name, method_name)`, not only `(class_name, method_name)`.

### Phase 4: Qualified Lookup

- When lowering `ModuleAlias.member(...)`, if `member` is a class method exported by that module:
  - resolve against the module-owned class method table
  - avoid falling back to legacy globals

### Phase 5: Dispatch Generation

- Continue generating mangled instance functions:
  - `__tc_Foldable_List_fold`
  - `__tc_Foldable_Array_length`
- Dispatch naming can remain based on class/type/method; no ABI change is required for this proposal.
- Only lookup changes: qualified module access should target these methods.

### Phase 6: Core Lowering

- Extend compile-time class-call resolution to handle qualified module member calls, not only bare identifiers.
- `Foldable.fold(xs, ...)` should lower exactly like a bare class method call does today after resolution.

## Example Lowering

Source:

```flux
import Flow.Foldable as Foldable

fn main() {
    Foldable.fold([1, 2, 3], 0, fn(acc, x) { acc + x })
}
```

Lowering intent:

- recognize `Foldable.fold` as module-qualified class method
- infer first argument type `List<Int>`
- resolve `Foldable<List>`
- lower to:

```text
__tc_Foldable_List_fold([1,2,3], 0, ...)
```

## Backward Compatibility

### Keep

- existing global `fold`, `length`, `to_list`
- existing unqualified user code
- existing built-in class machinery and dictionary elaboration
- existing mangled naming format

### Change

- module files may legally contain module-scoped type class declarations
- imported modules may expose class methods as namespace members

### Optional Future Deprecation

Later proposals may deprecate:

- top-level module-file `class` / `instance` outside module blocks
- unqualified collection helpers in favor of explicit module-qualified class APIs

## Migration Plan

### Step 1

Support module-body `class` / `instance` declarations fully.

### Step 2

Teach module export/member lookup about public class methods.

### Step 3

Support qualified class method resolution in Core lowering and bytecode compile-time dispatch.

### Step 4

Migrate `Flow.Foldable` to:

```flux
module Flow.Foldable {
    public class Foldable<f> { ... }
    instance Foldable<List> { ... }
    instance Foldable<Array> { ... }
}
```

### Step 5

Add end-to-end tests using:

```flux
import Flow.Foldable as Foldable
Foldable.fold(...)
Foldable.length(...)
Foldable.to_list(...)
```

## Test Plan

### Parser / Validation

- module body accepts `class`
- module body accepts `instance`
- invalid non-member statements still rejected
- diagnostics updated for module-file/module-body errors

### Resolver

- qualified `Foldable.fold([1,2,3], ...)` resolves to `__tc_Foldable_List_fold`
- qualified `Foldable.length([|1,2,3|])` resolves to `__tc_Foldable_Array_length`
- no collision with legacy bare `fold(...)`

### Integration

- `Flow.Foldable` module compiles
- explicit-import example compiles and runs
- Core dump shows mangled calls for qualified methods

### Regression

- existing global `fold`, `length`, `to_list` behavior unchanged
- existing `Functor<List>` `0150` tests remain green
- no regressions in `test_runner_cli` and examples snapshots

## Open Questions

- Should `public class` export all methods automatically, or require explicit export lists later?
- Should top-level `class` / `instance` in a module file remain supported during transition, or be disallowed once module-body support is complete?
- Should module-qualified class methods participate in `exposing (...)` imports, or require qualification only in the first iteration?

## Recommendation

Implement the Haskell-like model:

- classes live at module scope
- methods are accessed through the module namespace
- instances remain type-directed
- bare names are left alone for backward compatibility

That gives Flux a coherent path for `Flow.Foldable`, `Flow.Functor`, and future HKT classes without reopening the `0150` compiler semantics.
