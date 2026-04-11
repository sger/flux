- Feature Name: Module-Scoped Type Classes
- Start Date: 2026-04-09
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0145, Proposal 0150
- Status: Draft (revised)
- Date: 2026-04-09

## Summary

Introduce module-scoped type class syntax so classes and instances can live directly inside a module declaration and their methods are accessed through the module namespace. This replaces the current split model where `class` / `instance` are semantic top-level declarations while `module ... {}` is only an import-identity shell.

The revised proposal also specifies the semantic rules that the original draft left open: **class identity**, **instance coherence and the orphan rule**, **interface-file (`.flxi`) representation**, and **public / private access**. These are load-bearing for Flux's incremental `.fxc` cache and for avoiding the multi-decade coherence pain Haskell still carries.

Target shape:

```flux
module Flow.Foldable {
    public class Foldable<f> {
        fn fold<a, b>(x: f<a>, init: b, func: (b, a) -> b): b
        fn length<a>(x: f<a>) -> Int
        fn to_list<a>(x: f<a>) -> List<a>
    }

    public instance Foldable<List> {
        fn fold(xs, init, func) { ... }
        fn length(xs) { ... }
        fn to_list(xs) { ... }
    }

    public instance Foldable<Array> {
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
- Class identity is a bare `Identifier`, so two modules that both define a class called `Foldable` would collide silently.
- The `.fxc` cache keys on SHA-2 of dependencies, but instance visibility is a transitive-closure property — without a coherence rule, a new instance in any upstream module invalidates any downstream cache that ever imports it.

This proposal makes modules the namespace boundary for type classes, matching the model users expect from Haskell-style module qualification, and adopts a Rust-style orphan rule so the `.fxc` cache stays sound.

## Goals

- Allow `class` and `instance` declarations inside module bodies as first-class members.
- Resolve class methods through module qualification, for example `Foldable.fold(...)`.
- Give every class a globally unique identity `(module_path, class_name)`.
- Enforce the **orphan rule**: an `instance C<T>` is legal only where the class *or* the head type constructor is defined.
- Support `public` / private visibility for both classes and instances, matching how `public fn` already works.
- Keep instance resolution type-directed under the hood.
- Preserve the existing `AST -> Core -> CFG` pipeline and the monomorphic-dispatch fast path from 0145.
- Specify interface-file (`.flxi`) representation so cross-module compilation and caching are sound.

## Non-Goals

- No new semantic IR.
- No AST fallback in JIT or backend-specific hacks.
- No class-qualified method syntax such as `Foldable::fold` detached from module aliases.
- No flexible / overlapping / incoherent instance extensions, ever.
- No automatic migration of legacy global `fold`, `length`, `to_list` in this proposal.
- No change to HKT resolution semantics already delivered by `0150`.
- No re-export mechanism (`export module Other`) — reserved as future work.

---

## Proposed Syntax

### 1. Class declaration — with modules

Inside a `module` block, a class is declared with `class` or `public class`:

```flux
module Flow.Foldable {
    public class Foldable<f> {
        fn fold<a, b>(x: f<a>, init: b, func: (b, a) -> b): b
        fn length<a>(x: f<a>) -> Int

        // default method — implementations may omit it
        fn to_list<a>(x: f<a>) -> List<a> {
            fold(x, [], fn(acc, y) { append(acc, [y]) })
        }
    }

    // private helper class — visible only inside Flow.Foldable
    class FoldableInternal<f> {
        fn fold_raw<a>(x: f<a>) -> List<a>
    }
}
```

Rules:

- `class` is a legal module-body statement.
- `public class` exports the class name *and* all of its method names through the module surface.
- Bare `class` is **module-private**: the class name, its methods, and any instances of it are only visible inside the defining module.
- A class is **atomic**: you cannot export a subset of its methods. Partial class exports create "can I call a method of a class I can see?" holes; Flux refuses to model them. If you want private helpers, write private top-level functions in the same module instead of hidden methods.

### 2. Class declaration — without modules (legacy / top-level)

Top-level `class` and `instance` outside a module block remain supported for backward compatibility during the 0151 migration window, with two rules:

```flux
// legacy form — still parses and compiles
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
    fn neq(x: a, y: a) -> Bool { !eq(x, y) }
}

instance Eq<Int> {
    fn eq(x, y) { x == y }
}
```

Rules for top-level (non-module) declarations:

- The class's owning module is the **implicit file module** derived from the source file's module root (same rule Flux already uses for top-level `fn`).
- A top-level `class` is implicitly `public` — legacy files have no other way to be imported.
- Top-level `public class` is accepted but redundant; a warning is emitted.
- Top-level declarations still participate in the orphan rule (§Coherence). The implicit file module is the owning module for that check.
- Top-level `class` / `instance` outside a module block will emit a **soft-deprecation warning** one release after 0151 lands, and become a hard error two releases after that. The migration window gives stdlib and user code time to wrap existing declarations in `module ... { }`.

### 3. Instance declaration

Instances use `instance` or `public instance`:

```flux
module Flow.Foldable {
    public class Foldable<f> { ... }

    // exported — visible to any importer of Flow.Foldable
    public instance Foldable<List> {
        fn fold(xs, init, func) { ... }
        fn length(xs) { ... }
    }

    // module-private — only in-module code can resolve against it
    instance Foldable<InternalRing> {
        fn fold(r, init, func) { ... }
        fn length(r) { ... }
    }
}
```

Rules:

- `instance` is legal inside a module body.
- `public instance` is visible to every module that imports the defining module (directly or via explicit re-import — *not* transitively; see §Coherence).
- Bare `instance` is module-private and exists only for in-module resolution.
- Instances carry no user-visible name, but they *do* have an owning module, and visibility is a property of that module's export surface.
- You cannot partially implement a class. An `instance Foldable<List>` must provide every non-default method of `Foldable`. (Enforced already by 0145's `E442`.)

### 4. How to initialize a class and an instance — worked example

Minimal end-to-end example showing declaration, instance, and use site:

```flux
// lib/Flow/Show.flx
module Flow.Show {
    public class Show<a> {
        fn show(x: a) -> String
    }

    public instance Show<Int> {
        fn show(x) { int_to_string(x) }
    }

    public instance Show<Bool> {
        fn show(x) { if x { "true" } else { "false" } }
    }
}
```

```flux
// examples/use_show.flx
import Flow.Show as Show

fn main() with IO {
    print(Show.show(42))        // "42"
    print(Show.show(true))      // "true"
}
```

A user type in another module adds its own instance — legal under the orphan rule because `MyType` is defined in the same module as the instance:

```flux
// app/MyModule.flx
module App.MyModule {
    import Flow.Show as Show

    public type MyType { MkMy(Int) }

    // legal: head type `MyType` is defined here
    public instance Show.Show<MyType> {
        fn show(MkMy(n)) { concat("MkMy(", Show.show(n), ")") }
    }
}
```

```flux
// app/main.flx
import Flow.Show as Show
import App.MyModule as My

fn main() with IO {
    print(Show.show(My.MkMy(7)))    // "MkMy(7)"
}
```

Note that `Show.show(My.MkMy(7))` works even though `Show` is imported from `Flow.Show` and the instance is in `App.MyModule`. Instance resolution is type-directed: the compiler finds `instance Show.Show<MyType>` via the type's owning module (`App.MyModule`), which is in scope because `main.flx` imports it.

### 5. Public and private access — summary table

| Declaration form              | Class name visible | Methods callable as `M.method` | Instance participates in resolution from other modules |
|-------------------------------|--------------------|--------------------------------|-------------------------------------------------------|
| `public class C<a> { ... }`   | yes, across imports | yes, `M.method(...)`           | n/a (instances handle it)                              |
| `class C<a> { ... }`          | module-private     | no                              | no (private classes can have only private instances)  |
| `public instance C<T> { ... }`| n/a                | n/a                             | yes, when importer has imported `M` *and* can name `C` |
| `instance C<T> { ... }`       | n/a                | n/a                             | no — only in-module call sites resolve against it     |

Consistency rules:

- A private class cannot have a `public instance`. The compiler rejects this (`E447`).
- A `public instance` of a `public class` must satisfy the orphan rule below.
- Instance visibility is **not transitive**: importing `M` which itself imports `Flow.Show` does not put `Flow.Show`'s instances in scope. You must import `Flow.Show` directly. This departs from Haskell but matches Rust and gives Flux's cache a local invalidation story.

### 6. Constraint syntax with qualified classes

Class constraints can name a class either by its short name (resolved against imports in scope) or by its module-qualified form:

```flux
import Flow.Foldable as F

// short form — `Foldable` resolves to F.Foldable via the import
fn total_short<a: Foldable<Int>>(xs: a) -> Int {
    F.fold(xs, 0, fn(acc, x) { acc + x })
}

// explicit form — unambiguous
fn total_qualified<a: F.Foldable<Int>>(xs: a) -> Int {
    F.fold(xs, 0, fn(acc, x) { acc + x })
}
```

Rules:

- Short-form constraints resolve against the same name table as type references.
- Qualified constraints are the canonical form in error messages.
- Two classes with the same short name from different modules in scope require qualification (`E448` if not qualified).

### 5a. Imports: file-level and module-body

Imports may appear **either** at file top level (outside any `module` block) **or** inside a module body. Both positions are legal and behave slightly differently:

```flux
// app/thing.flx

// (a) file-level import: visible to every module in this file
import Flow.Eq as Eq

module App.Thing {
    // (b) module-body import: visible only inside App.Thing
    import Flow.Show as Show

    public data Foo { MkFoo(Int) }

    // uses both Eq (file-level) and Show (module-body)
    public instance Eq.Eq<Foo> {
        fn eq(MkFoo(a), MkFoo(b)) { a == b }
    }

    public instance Show.Show<Foo> {
        fn show(MkFoo(n)) { concat("MkFoo(", Show.show(n), ")") }
    }
}

module App.Other {
    // sees Eq (file-level) but NOT Show (that import is scoped to App.Thing)
    public fn compare(x: Foo, y: Foo) -> Bool {
        Eq.eq(x, y)
    }
}
```

Rules:

- **File-level imports** live in the file's top-level scope and are inherited by every `module` block in the same file. They are the right place for imports used across every module in the file (the common case — one `module` per file).
- **Module-body imports** live in the module's namespace and are visible only to code inside that module block. They are the right place for imports that only one module in a multi-module file needs, or imports that should appear in a module's `.flxi` metadata alongside the module itself.
- **Name collision between file-level and module-body imports** is resolved by **module-body wins** (local scope shadows outer scope), analogous to §7's rule that locally-declared names shadow imported and global ones. A collision that introduces the *same* short name bound to *different* targets is a hard error at the module-body import site (`E456`, reserved below).
- **Imports are never exported.** Writing `import Flow.Show as Show` inside `module App.Thing` does not make `Show` reachable as `App.Thing.Show` from another module. An import is a local name-lookup aid, not a public member. There is no `public import` in 0151, and re-exports are deferred to a future proposal.
- **Instance visibility comes from the defining module's import, not from who's calling.** If `App.Thing` defines `public instance Show.Show<Foo>`, any caller that has imported both `App.Thing` and `Flow.Show` can use it. The fact that `App.Thing` imported `Flow.Show` via a module-body import does not leak; the instance is still discoverable through normal type-directed resolution.
- **Duplicate imports** (same module, same alias, appearing both file-level and module-body) are allowed but emit a warning. Same module with *different* aliases is allowed without warning (useful when the file-level alias is long and one module wants a shorter local alias).

Precedence summary, from highest to lowest, for a bare name inside a module body:

1. Locally-declared names in the module (classes, methods, functions, types).
2. Module-body imports (via their alias or via `exposing`).
3. File-level imports (via their alias or via `exposing`).
4. Legacy global helpers (prelude).

### 6a. Mixed module bodies: classes, instances, functions, types together

A module body is a single namespace that can freely mix ordinary declarations with class and instance declarations. Resolution inside the body follows a strict priority order, which matters because it determines what the bare name `fold` refers to at every site:

```flux
module Flow.Foldable {
    import Flow.List as L

    // 1. public ADT — exported like any data type
    public data Tree<a> { Leaf, Node(Tree<a>, a, Tree<a>) }

    // 2. private helper — module-local, not in .flxi
    fn tree_to_list_acc<a>(t: Tree<a>, acc: List<a>) -> List<a> {
        match t {
            Leaf => acc,
            Node(l, x, r) => tree_to_list_acc(l, [x, ...tree_to_list_acc(r, acc)]),
        }
    }

    // 3. public class — note the `<f>` type parameter is MANDATORY
    public class Foldable<f> {
        fn fold<a, b>(x: f<a>, init: b, func: (b, a) -> b): b
        // default method — may call other methods of the same class unqualified
        fn length<a>(x: f<a>) -> Int {
            fold(x, 0, fn(acc, _) { acc + 1 })
        }
    }

    // 4. instance of own class + own type — legal (orphan rule: class-local)
    public instance Foldable<Tree> {
        fn fold(t, init, func) { ... }
    }

    // 5. instance of own class + foreign type — legal (orphan rule: class is local)
    public instance Foldable<List> {
        fn fold(xs, init, func) { L.fold_left(xs, init, func) }
    }

    // 6. public ordinary function that consumes the class
    public fn sum_foldable<a: Foldable<Int>>(xs: a) -> Int {
        fold(xs, 0, fn(acc, x) { acc + x })   // `fold` = local class method
    }

    // 7. private smoke test — not exported
    fn __smoke_test() -> Bool {
        sum_foldable([1, 2, 3]) == 6
    }
}
```

**Type parameter declaration is mandatory.** A class that mentions `f<a>` in its methods must bind `f` in its header: `public class Foldable<f>`. Writing `public class Foldable { fn fold(x: f<a>, ...) }` is rejected at parse/infer time because `f` is a free type variable. This is not a new rule introduced by 0151 — it's inherited from 0145's type-parameter scoping — but the proposal spells it out here to preempt the "can I drop `<f>`?" question.

**Name priority inside the module body:**

1. Locally-declared names (this module's classes, methods, functions, types) — always win.
2. Directly imported names (`L.fold_left` above is reached only via its alias; bare `fold_left` would not resolve).
3. Legacy global helpers (`fold`, `length`, `to_list`) — lowest priority; shadowed by any locally-declared match.

**Consequence for the example:**

- Inside `Foldable.length`'s default body, `fold` is the local class method (rule 1).
- Inside `sum_foldable`, `fold` is *also* the local class method — you do not have to write `Foldable.fold` inside the class's own defining module.
- Inside `__smoke_test`, the `==` still routes through `Eq<Int>` as normal (0145 behavior).
- From *outside* the module, `fold` bare still resolves to the legacy global; the only way to reach this class's `fold` is `Foldable.fold(...)` or `import Flow.Foldable exposing (fold)` (see §9).

**What cannot appear in a module body:**

- Top-level effect handlers that escape the module's with-binding scope.
- `instance` declarations referencing a private class from another module (visibility error, `E450`).
- Two `public instance` declarations for the same `(ClassId, head_type)` pair — rejected as duplicate instance, `E443`.
- A `public class` whose method signature mentions a private type of the same module — this leaks a private name through the export surface (`E451`, new diagnostic reserved below).

### 7. Unqualified access inside the defining module

Inside a module body, locally-declared class methods shadow imported and global names. This keeps default-method bodies and cross-method helpers readable:

```flux
module Flow.Foldable {
    public class Foldable<f> {
        fn fold<a,b>(x: f<a>, init: b, func: (b,a)->b): b

        // `fold` here refers to this class's own method,
        // not to any legacy global `fold`.
        fn length<a>(x: f<a>) -> Int {
            fold(x, 0, fn(acc, _) { acc + 1 })
        }
    }
}
```

Outside the module, the only way to call `fold` is `Foldable.fold(...)` or an explicit `import Flow.Foldable exposing (fold)` (see §9).

### 8. Method access from importers

Preferred:

```flux
import Flow.Foldable as Foldable

Foldable.fold(xs, init, func)
Foldable.length(xs)
```

Not supported by this proposal:

- Bare `fold(xs, ...)` as the primary `Foldable` API from an importer (existing global `fold` keeps winning).
- Class-qualified method syntax detached from modules (`Foldable::fold`).

### 9. Optional unqualified import via `exposing`

Class methods can be brought into unqualified scope at the import site:

```flux
import Flow.Foldable as Foldable exposing (fold, length)

fn main() with IO {
    print(fold([1,2,3], 0, fn(a,x) { a + x }))     // resolves to Foldable.fold
    print(length([1,2,3]))                          // resolves to Foldable.length
}
```

Rules:

- `exposing (...)` may list class methods individually. `exposing (..)` (star-exposing) is not provided in this proposal — explicit is better.
- A name collision between an exposed class method and an existing top-level identifier in the importing module is a hard error at the import site (`E449`), not silent shadowing.
- The unqualified call still goes through type-directed instance resolution; `exposing` only affects name lookup, not dispatch.

---

## ADTs (`data`) and Module-Scoped Classes

Algebraic data types in Flux are declared with `data`, not `type`. They are first-class module members and interact with classes in three places: **visibility**, **orphan rule**, and **`deriving` clauses**. This section specifies all three.

### 10. `data` inside a module body

A `data` declaration lives in a module body exactly like `fn`, `class`, and `instance`, and carries the same `public` / private visibility modifier:

```flux
module Flow.Geometry {
    // public ADT — type name, all constructors, and all methods-via-deriving exported
    public data Shape {
        Circle(Float),
        Rect(Float, Float),
        Triangle(Float, Float, Float),
    }

    // generic public ADT
    public data Tree<a> {
        Leaf,
        Node(Tree<a>, a, Tree<a>),
    }

    // private ADT — only visible inside Flow.Geometry
    data InternalGrid<a> {
        Grid(Int, Int, List<a>),
    }
}
```

Rules for `data` visibility:

- **`public data T { C1, C2, ... }` is atomic.** The type name *and every constructor* are exported together. There is no partial export such as "export the type but hide a constructor" — that creates the same "can I pattern-match on something I can see?" holes that partial class exports create, and Flux refuses to model them. If you need an opaque type, make the whole `data` private and expose only smart constructors as `public fn`.
- **`data T { ... }` (no `public`) is module-private.** The type name and all constructors are invisible outside the module. Pattern matching, construction, and type references all fail at the module boundary.
- **Generic ADT type parameters** (`data Tree<a>`) work exactly as in 0145; 0151 does not change them.
- **Constructors are *not* their own visibility unit.** This matches Rust's enum rule (all variants are pub if the enum is pub) and differs from Haskell's `Foo(..)` partial-export syntax. Explicit non-goal: no `public data Foo { public C1, C2 }`.

### 11. ADTs and the orphan rule

The orphan rule from §Coherence treats a `data` declaration as a **head type constructor** for purposes of instance legality:

> An `instance C<T>` is legal in module `M` only if either (a) class `C` is defined in `M`, or (b) at least one head type constructor of `T` is a `data` (or built-in type constructor) defined in `M`.

Worked examples:

```flux
module App.Geometry {
    import Flow.Show as Show

    public data Shape { Circle(Float), Rect(Float, Float) }

    // legal: head type `Shape` is defined here, class `Show` is foreign
    public instance Show.Show<Shape> {
        fn show(s) {
            match s {
                Circle(r) => concat("Circle(", Show.show(r), ")"),
                Rect(w, h) => concat("Rect(", Show.show(w), ", ", Show.show(h), ")"),
            }
        }
    }
}
```

```flux
module App.ThirdParty {
    import Flow.Show as Show
    import App.Geometry as G

    // ILLEGAL — E447
    // neither `Show` nor `Shape` is defined here
    public instance Show.Show<G.Shape> {
        fn show(s) { "..." }
    }
}
```

The second form is rejected because it would let two importers see two different `Show<Shape>` instances depending on which of `Flow.Show`, `App.Geometry`, or `App.ThirdParty` they transitively import — the classic orphan problem. The diagnostic points at the class's owning module (`Flow.Show`) and the type's owning module (`App.Geometry`) and tells the author to put the instance in one of them.

Nested / generic heads:

- `instance Show.Show<List<Shape>>` is legal in a module that defines either `List` **or** `Shape`. The rule walks the head type's constructor, not its full type tree — the *outermost* constructor `List` or *any* mentioned constructor is enough.
- `instance Show.Show<(Int, String)>` — the head is the tuple type constructor, which is built-in. Built-in type constructors are treated as "owned by the prelude," so this is legal **only** in the module that owns the class (i.e. you cannot add new instances for built-in types anywhere other than where the class is declared). This prevents orphan instances for `Show<Int>`, `Show<String>`, etc., outside their defining module.

### 12. Private ADTs and public instances — E455

Combining public / private across `data` and `instance` introduces a subtle leak: a `public instance` for a private `data` would let outsiders observe the private type through the class dictionary.

```flux
module Leaky {
    import Flow.Show as Show

    // private ADT
    data Secret { Mk(Int) }

    // ILLEGAL — E455
    public instance Show.Show<Secret> { ... }
}
```

The mangled method `__tc_Flow_Show_Show_Secret_show` would be callable from outside (the mangled name itself is a public symbol in the object file), which is exactly the leak. Diagnostic `E455` is reserved for this case and tells the author to either make `Secret` public or make the instance private. This is the ADT mirror of `E450` (public instance of a private class).

Legal combinations:

| `data` visibility | `instance` visibility | Status    | Notes                                        |
|-------------------|-----------------------|-----------|----------------------------------------------|
| public            | public                | allowed   | normal case                                  |
| public            | private               | allowed   | in-module-only instance, e.g. for a test     |
| private           | private               | allowed   | fully internal                               |
| private           | public                | **E455**  | would leak the private type through dispatch |

### 13. `deriving` clauses on `data`

Flux supports `deriving (Eq, Ord, Show, ...)` on `data` (see [examples/strict_types/type_class_deriving.flx](examples/strict_types/type_class_deriving.flx)). Under 0151, `deriving` generates **an instance owned by the module that contains the `data` declaration**, which is always orphan-rule compliant by construction (the type's owning module and the instance's owning module are the same).

```flux
module Flow.Color {
    import Flow.Eq as Eq
    import Flow.Ord as Ord
    import Flow.Show as Show

    public data Color { Red, Green, Blue }
        deriving (Eq.Eq, Ord.Ord, Show.Show)
}
```

Rules:

- Each derived class name in the `deriving (...)` list is resolved like a short-name class constraint. If a class is ambiguous, use the qualified form `Flow.Eq.Eq` as shown.
- Each derived instance inherits the `data`'s visibility: `public data` with `deriving` produces public instances; `data` (private) with `deriving` produces private instances.
- A `deriving` clause that names a private class from another module is rejected with `E448`-style "unknown class" (you cannot reach a private class from outside).
- `deriving` generates instances with their effect rows pinned to `<>` (pure). Classes whose methods are pure-only (`Eq`, `Ord`, `Show`, `Num`, `Semigroup`) work; classes with row-polymorphic methods are not `deriving`-eligible in this proposal (deferred to a future "deriving strategies" proposal).
- The orphan rule is trivially satisfied by `deriving` because the generated instance lives in the same module as the `data`.

### 14. Generic ADTs and HKT classes

`deriving` and hand-written instances both work for generic ADTs, subject to the HKT rules already shipped in 0150:

```flux
module Flow.Option {
    public data Option<a> { None, Some(a) }

    public instance Flow.Functor.Functor<Option> {
        fn fmap(opt, f) {
            match opt {
                None    => None,
                Some(x) => Some(f(x)),
            }
        }
    }
}
```

`Functor<Option>` is legal in `Flow.Option` because `Option` is defined there. It remains legal even though `Functor` is foreign (defined in `Flow.Functor`), because the orphan rule needs either one, not both.

### 15. ADT interaction with Aether

ADTs are already first-class participants in Aether (they are the main reason Perceus exists). 0151 does not change any Aether machinery for `data` declarations themselves. The only 0151-specific concern is that **a `data`'s visibility must not affect whether Aether sees its constructors, destructors, and match arms** — exactly the same invariant as for private class methods (Invariant A). Private `data` lowering still emits constructor functions and `Case` arms into `CoreProgram.defs`; visibility is enforced at resolution, not at lowering.

### 16. ADT summary for this proposal

| Concept                                      | Rule                                                             |
|-----------------------------------------------|------------------------------------------------------------------|
| `data` inside a module body                  | Legal, with `public` / private like `fn`                         |
| Constructor-level visibility                 | **Not supported** — all constructors inherit the type's visibility |
| Orphan rule treats `data` as head type       | Yes — defining a `data T` in module `M` lets `M` host `instance C<T>` for any foreign `C` |
| `public instance` of a private `data`        | **E455** — would leak the type through dispatch                   |
| `deriving` generates module-local instances  | Yes — always orphan-rule compliant                                |
| `deriving` for row-polymorphic classes        | **Not in this proposal** — deferred to future "deriving strategies" |
| Generic ADTs and HKT instances               | Same 0150 rules, no new restrictions                              |
| Aether sees private ADTs' constructors       | Yes — Invariant A applies to ADTs too                             |

---

## Semantic Model

### Class identity

Every class has a globally unique identity:

```
ClassId = (ModulePath, Identifier)
```

Two classes with the same short name in different modules are distinct. Class constraints, instance heads, and dictionary keys all reference `ClassId`, not the bare `Identifier`.

Implementation:

- [src/types/class_env.rs:67-73](src/types/class_env.rs#L67-L73) `ClassEnv.classes` becomes `HashMap<ClassId, ClassDef>`.
- [src/types/class_env.rs:53-60](src/types/class_env.rs#L53-L60) `InstanceDef.class_name: Identifier` becomes `class_id: ClassId`.
- `ClassConstraint` likewise references `ClassId`.
- Superclass references are stored as fully-resolved `ClassId`, so a downstream module that imports only `Flow.Ord` can still check the `Flow.Eq.Eq` superclass without importing `Flow.Eq` directly.

### Coherence and the orphan rule

An `instance C<T>` is legal in module `M` only if **either**:

1. The class `C` is defined in `M`, **or**
2. At least one head type constructor in `T` is defined in `M`.

Rationale:

- Makes cache invalidation local: a new `instance C<T>` only affects modules that imported the module defining `C` or the module defining the head type of `T`. No transitive-closure walk needed.
- Eliminates GHC's "orphan module" `.hi` marker and the associated slowdown.
- Prevents the "two modules define conflicting `Eq<MyType>` and the winner depends on import order" class of bug.
- Small trade-off: newtype wrapper patterns for adapting foreign instances are not expressible. Flux's scale makes this an acceptable loss.

Diagnostic: `E447` — *orphan instance: class `C` is defined in module `X` and head type `T` is defined in module `Y`; an instance must live in one of those modules*.

### Instance visibility

- Private instance (`instance`) is only in the resolution table when compiling its defining module.
- Public instance (`public instance`) is in the resolution table for any module that **directly imports** its defining module.
- Instance visibility is **not transitive**: if `A` imports `B` and `B` imports `C`, instances defined in `C` are *not* visible to `A` unless `A` also imports `C`. The orphan rule guarantees that anyone who needs `instance C<T>` has already imported either `C`'s module or `T`'s module, so this never creates a "missing instance" surprise in practice.

### Instance selection is still type-directed

`instance Foldable<List>` and `instance Foldable<Array>` still feed the same class environment and resolver machinery:

- class matching against `ClassId`
- dictionary construction
- compile-time monomorphic dispatch (fast path retained from 0145)
- dictionary elaboration for polymorphic cases

The proposal changes namespacing and visibility; it does not change the instance-selection algorithm.

### Resolution rules (summary)

**Unqualified name:**

- Inside a class's defining module, the class's own method wins.
- Inside any other module, legacy global functions continue to win (backward compat) unless the name was brought in via `exposing`.
- A name brought in via `exposing` wins over legacy globals; a collision with a locally-defined top-level name is an error.

**Qualified name `Alias.method(...)`:**

1. Resolve `Alias` to an imported module.
2. Check whether `method` is an exported function member of that module.
3. Check whether `method` is an exported class method member of that module.
4. If it is a class method:
   - infer the first argument type,
   - attempt compile-time instance resolution against the visible instance set,
   - lower to a mangled instance function when monomorphic,
   - otherwise use dictionary elaboration.

### Mangled name ABI

To keep class identity unique, mangled instance functions and dictionary globals include the owning module path:

```
__tc_<mod_path>_<class>_<type...>_<method>
__dict_<mod_path>_<class>_<type...>
```

where each `<segment>` is individually encoded by the rules in §*Symbol Encoding* below and segments are joined with a single bare `_`.

Example (common case — no special characters in any segment):

```
Flow.Foldable.Foldable<List>.fold
  →  __tc_Flow_Foldable_Foldable_List_fold

Flow.Foldable.Foldable<List>
  →  __dict_Flow_Foldable_Foldable_List
```

This is an **ABI break** relative to the 0145 mangling scheme. It is intentional:

1. Without the module path, two modules defining a class called `Foldable` would produce colliding mangled names.
2. Without the encoding rules below, user identifiers containing `_` would collide with the segment separator (the classic `Foo_Bar` vs `Foo.Bar` problem).

Cached `.fxc` files must be invalidated on upgrade. The release notes will call this out.

### Symbol Encoding

Both GHC (Z-encoding) and Koka (`asciiEncode`) solve the separator-collision problem with the same core trick: **self-escape the character used as the segment separator** so that a bare occurrence of it in the final symbol can only have come from a segment join. Flux adopts a minimal variant of Koka's mnemonic scheme, chosen because:

- Flux source identifiers are snake_case by convention, so `_` is the dominant non-alphanumeric. A mnemonic scheme keeps `fold_left` readable as `fold__left`, whereas a GHC-style `zu` escape would produce `foldzuleft` and force everyone to install a demangler.
- Flux already uses `__` as a marker prefix (`__tc_`, `__dict_`), so "double underscore is special" is a mental model users already have.
- Flux identifiers do not allow `-`, `?`, `'`, or most of the characters that force Koka into context-sensitive rules. The encoding table is therefore tiny and context-free.

#### Normative encoding rules

Each **segment** (a single component of the mangled name — module path component, class name, type name, or method name — *not* the whole mangled name) is encoded by applying the following substitutions, in order, to its source characters:

| Source character | Encoded as | Notes |
|------------------|------------|-------|
| `_`              | `__`       | Self-escape. Must be applied *first*. |
| `.`              | `_dot_`    | Only appears inside segments if an identifier literally contains a dot; normally absorbed by the splitter. |
| `'`              | `_q_`      | Reserved for future use; Flux identifiers currently reject `'`. |
| `A`–`Z`          | `A`–`Z`    | Pass through unchanged. |
| `a`–`z`          | `a`–`z`    | Pass through unchanged. |
| `0`–`9`          | `0`–`9`    | Pass through unchanged. |
| any other        | `_xHH_`    | Hex escape. `HH` is two lowercase hex digits of the byte value. Reserved; Flux rejects such characters in source. |

After every segment is encoded, segments are joined with a single bare `_`. Because `_` in source becomes `__`, and every other escape in the table uses `_<name>_` (surrounded by underscores), **any bare `_` in the final symbol is unambiguously a segment separator**. This gives the scheme the same bijection property as GHC's Z-encoding, without the debugger-readability cost.

#### Module path splitting

A module path `Flow.Foldable.Inner` is split on `.` *before* segment encoding, yielding three segments: `Flow`, `Foldable`, `Inner`. This is why the `.` → `_dot_` row in the table is listed as "normally absorbed" — the only way to see `_dot_` in output is if a user writes a literal `.` inside a non-path identifier, which the parser currently rejects. The row is in the table for forward compatibility.

#### Worked examples

**Common case — no special characters.** The encoding is the identity on every segment, so everything in the proposal above remains byte-identical:

```
Flow.Foldable.Foldable<List>.fold
  segments: [Flow, Foldable, Foldable, List, fold]
  encoded:  [Flow, Foldable, Foldable, List, fold]
  joined:   __tc_Flow_Foldable_Foldable_List_fold
```

**The `Show_Show` regression case.** Two distinct classes that would collide under naive mangling are now disambiguated by the `_` → `__` self-escape:

| Source                                 | Segments                                   | Mangled                                          |
|----------------------------------------|--------------------------------------------|--------------------------------------------------|
| `Flow.Show.Show<Secret>.show`          | `[Flow, Show, Show, Secret, show]`         | `__tc_Flow_Show_Show_Secret_show`                |
| `Flow.Show_Show<Secret>.show`          | `[Flow, Show_Show, Secret, show]`          | `__tc_Flow_Show__Show_Secret_show`               |
| `My.Mod.Foo<Bar_Baz>.qux`              | `[My, Mod, Foo, Bar_Baz, qux]`             | `__tc_My_Mod_Foo_Bar__Baz_qux`                   |
| `My.Mod.Foo_Bar<Baz>.qux`              | `[My, Mod, Foo_Bar, Baz, qux]`             | `__tc_My_Mod_Foo__Bar_Baz_qux`                   |
| `App.Test_Suite.Eq<Config>.eq`         | `[App, Test_Suite, Eq, Config, eq]`        | `__tc_App_Test__Suite_Eq_Config_eq`              |

All four of the Show_Show / Foo_Bar cases are distinct. You can recover the source form by scanning the mangled name left to right: every bare `_` is a separator, every `__` is a literal `_` in source, and every `_<word>_` is a mnemonic escape.

**Dictionary globals.** The same rules apply:

```
Flow.Show_Show<Secret>
  →  __dict_Flow_Show__Show_Secret
```

**Generic ADT instance head.** For multi-argument heads like `Bifunctor<Either>`, the type argument list is joined segment-by-segment with the same rules:

```
Flow.Bifunctor.Bifunctor<Either, Int>.bimap
  →  __tc_Flow_Bifunctor_Bifunctor_Either_Int_bimap
```

Nested or applied types (`Functor<Compose<F, G>>`) are out of scope for 0151; their encoding will be specified in a follow-up proposal.

#### Bijection proof sketch

For any two distinct source triples `(module_path, class, head_types, method)` with `(module_path', class', head_types', method')`, the mangled outputs differ. Proof: the only way the outputs can agree is if each segment encodes to the same string. The encoding is injective on segments because:

- Every escape sequence begins and ends with `_` and contains at least one non-underscore character in between (e.g. `_dot_`, `_xHH_`, or the degenerate self-escape `__` which contains no middle but is the only sequence of exactly two consecutive `_`).
- `_` in source always becomes `__`, never `_<anything>`.
- Alphanumerics pass through unchanged.

So the decoder is a simple left-to-right scan: runs of `__` are literal `_`, runs of `_<word>_` with `<word>` in the known table are their mnemonic, and a lone `_` is a segment separator. Because each rule has a unique prefix behavior, decoding is unambiguous. And because segments are then joined with the one character (`_`) that cannot appear bare inside any segment, segment boundaries are unambiguous too. QED.

#### Reference implementation

A 20-line reference encoder and decoder will live in `src/types/mangle.rs` (new module), with the encoding rules as a `const` table and a property-test (`proptest`-style) round-trip asserting `decode(encode(s)) == s` for arbitrary segment strings drawn from the Flux identifier grammar.

#### Prior art comparison

| Language | Scheme                  | Separator self-escape | Readability | Spec size     |
|----------|-------------------------|-----------------------|-------------|---------------|
| GHC      | Z-encoding              | `_` → `zu`            | low         | ~25 lines     |
| Koka     | `asciiEncode`           | `_` → `__`            | high        | ~60 lines (context-sensitive) |
| Flux     | this proposal           | `_` → `__`            | high        | ~15 lines     |

Flux's scheme is strictly smaller than Koka's because Flux rejects identifier characters (`-`, `?`, `'`) that force Koka into context-sensitive rules.

#### Test cases (normative)

These cases must be covered by unit tests in `src/types/mangle.rs`:

1. **Identity on alphanumerics:** `encode("Foldable") == "Foldable"`.
2. **Self-escape of underscore:** `encode("Foo_Bar") == "Foo__Bar"`.
3. **Multiple underscores:** `encode("a_b_c") == "a__b__c"`.
4. **Leading and trailing underscores:** `encode("_priv") == "__priv"`, `encode("trailing_") == "trailing__"`.
5. **Empty segment rejected:** encoding an empty segment is a compiler invariant violation; the encoder asserts.
6. **Join with separator:** `join(["Flow", "Show", "Show", "Secret", "show"]) == "Flow_Show_Show_Secret_show"`.
7. **Show_Show regression:** the four distinct mangled names in the worked-example table above must all be produced from their respective source triples and must all be distinct.
8. **Round-trip:** `decode(encode(s)) == s` for every segment in Flux's identifier grammar (property test).
9. **Decode rejects invalid input:** `decode("foo_bar_")` with a trailing lone `_` fails, `decode("foo_xZZ_")` with a non-hex escape fails.
10. **No collision between escape mnemonics and user identifiers:** `encode("dot") == "dot"` (not `_dot_`), proving that only literal `.` in source produces the `_dot_` sequence in output.

### Interface file (`.flxi`) representation

A module's `.flxi` records:

- Every `public class` with its `ClassId`, type parameters, kinds, superclass `ClassId`s, and method signatures (including default-method presence flags).
- Every `public instance` owned by this module, with its `ClassId`, head type(s), instance context, and the list of method names implemented (for arity cross-check).
- Private classes and instances are **not** written to `.flxi`.

Hash inputs for the `.fxc` cache must include the full class/instance table from every directly-imported `.flxi`. This is a small extension of the existing SHA-2 dependency hashing and preserves cache soundness under the orphan rule.

---

## AST Changes

Current AST already supports `Statement::Class` and `Statement::Instance`. This proposal formalizes them as legal module members and adds visibility:

- Module-body validation accepts `Class` and `Instance`.
- `Statement::Class` gains a `visibility: Visibility` field (reusing the existing `Visibility` enum used by `fn`).
- `Statement::Instance` gains a `visibility: Visibility` field.
- Module member collection records exported class methods as module members.
- Top-level `class` / `instance` outside a module body is still parsed, treated as `public`, and emits a soft-deprecation warning once 0151 lands.

Preferred end state (after the migration window):

- Class and instance declarations live inside `Statement::Module { body }`.
- Top-level class/instance becomes a hard error.

---

## Compiler Changes

### Phase 1: Parsing and validation

- Accept `class` / `public class` / `instance` / `public instance` inside module bodies.
- Accept top-level `class` / `instance` for backward compat; treat as implicitly public and owned by the file's implicit module.
- Update module-content diagnostics to treat class/instance as valid members.

### Phase 2: Class identity refactor

- Replace `HashMap<Identifier, ClassDef>` with `HashMap<ClassId, ClassDef>` in `ClassEnv`.
- Update `InstanceDef`, `ClassConstraint`, and all downstream consumers to reference `ClassId`.
- Resolve short-name class references against the import table during collection; fail with `E448` if ambiguous.

### Phase 3: Module member collection

- Record each `public class`'s method names as exported module members, alongside ordinary functions.
- Tag each method with its `ClassId` so qualified lookup can reach it.

### Phase 4: Orphan rule enforcement

- After class/instance collection, walk every `public instance` and verify the orphan rule.
- Emit `E447` on violation, pointing at both the class's owning module and the head type's owning module.

### Phase 5: Visibility enforcement

- Private classes: methods not exported as module members; instances of a private class may not be `public instance` (`E450`).
- Private instances: excluded from the visible-instance set when compiling any module other than the defining one.

### Phase 6: Qualified lookup

- When lowering `ModuleAlias.member(...)`, if `member` is a class method exported by that module, resolve against the module-owned class method table.
- For `exposing`-imported methods, extend the local name table at the import site; still dispatch via the class method path.

### Phase 7: Dispatch generation

- Mangled names gain the module path prefix (see §Mangled name ABI).
- Dictionary globals likewise.
- `class_dispatch.rs` generates `__tc_<mod>_<class>_<type>_<method>` from the `ClassId` already present on the instance.

### Phase 8: Core lowering

- Extend compile-time class-call resolution to handle qualified module member calls, not only bare identifiers.
- `Foldable.fold(xs, ...)` lowers exactly like a bare class method call does today after resolution, with the new mangled names.

### Phase 9: Interface files

- Extend the `.flxi` writer and reader to round-trip public classes and public instances.
- Update `.fxc` hash inputs to include the class/instance table of every directly-imported `.flxi`.

---

## Effects and Instance Methods

Flux has row-polymorphic effect types. Class methods need to play nicely with them. This section spells out exactly how.

### Prior art — how Koka handles this

Koka does not have Haskell-style type classes. It solves the "a type must support operation X" problem with **named implicit parameters** (`?foo : ...`) and **named handlers** for effectful dictionaries. Observed in [E:/Github/koka/lib/std/core/list.kk](file:///E:/Github/koka/lib/std/core/list.kk):

```koka
pub fun (==)( xs : list<a>, ys : list<a>, ?(==) : (a,a) -> e bool ) : e bool
pub fun cmp( xs : list<a>, ys : list<a>, ?cmp : (a,a) -> e order ) : e order
pub fun show( xs : list<a>, ?show : a -> e string ) : e string

pub fun foldl1(xs : list<a>, f : (a,a) -> <exn|e> a) : <exn|e> a
```

Key observations:

- The "dictionary" is literally a function parameter, prefixed `?`, inferred by the compiler.
- The effect row `e` is a plain type variable on the function arrow, and it naturally flows through: if the `?cmp` comparison is pure, the whole call is pure; if it does I/O, the whole call carries I/O in its row.
- **Effects are not part of the class shape.** They attach to the *instance* through the row variable, not to the declaration.
- Named handlers (`named effect reader`, `named handler ... fun ask() msg`, see [E:/Github/koka/samples/handlers/named/ask.kk](file:///E:/Github/koka/samples/handlers/named/ask.kk)) give Koka multiple simultaneous "instances" via runtime evidence handles, which is more flexible than a single-canonical-instance model.

What Koka gets right: effect polymorphism on methods is automatic because the dictionary is just an ordinary effectful function. What it gives up: compile-time instance resolution and the monomorphic-dispatch fast path Flux already has.

### The problem for Flux

0145 class method signatures currently have no effect row. If we naively add one:

```flux
public class Show<a> {
    fn show(x: a) -> String with <>     // pure
}
```

...then an instance that actually needs an effect:

```flux
public instance Show<LogHandle> {
    fn show(h) {
        log_line(h, "shown")             // IO effect!
        handle_id(h)
    }
}
```

...is rejected because the instance body's inferred effect row `<IO>` does not unify with the class's declared row `<>`. That is the right thing to do for `Show`, which is *supposed* to be pure. But for a `Foldable.fold` whose `func` callback is effectful, or a `Monad.bind` whose entire point is to sequence effects, rejecting rowful instances would be useless.

### Design rule — effect-polymorphic class methods

**Class method signatures are row-polymorphic by default.** Each method declares its effect row with a row variable that is *implicitly universally quantified* over the method's own type parameters, and every callback parameter shares the same row unless otherwise annotated. The row variable is written `e` by convention:

```flux
public class Foldable<f> {
    fn fold<a, b, e>(x: f<a>, init: b, func: (b, a) -> b with e): b with e
    fn length<a, e>(x: f<a>) -> Int with e
    fn to_list<a, e>(x: f<a>) -> List<a> with e
}
```

Reading: "for any effect row `e`, `fold` with a callback doing `e` itself does `e`." This matches Koka's `<exn|e>` discipline exactly, but stays inside Flux's existing `with <row>` syntax.

Three sugar forms to keep signatures short:

**(1) `with <>` — pure, no row polymorphism.** Methods that must be pure (`Eq.eq`, `Ord.compare`, `Show.show`, `Num.add`):

```flux
public class Eq<a> {
    fn eq(x: a, y: a) -> Bool with <>
    fn neq(x: a, y: a) -> Bool with <> { !eq(x, y) }
}
```

Instance bodies whose inferred row is not `<>` are rejected at instance collection (`E452` — impure body for a pure class method).

**(2) `with _` — the default, row-polymorphic over a fresh variable.** This is what you get if you write nothing:

```flux
public class Foldable<f> {
    fn fold<a, b>(x: f<a>, init: b, func: (b, a) -> b): b     // implicit `with _`
}
```

Desugars exactly to the explicit form above. The row variable is fresh per method, so different methods of the same class do not have to agree.

**(3) `with <E, ..>` — a minimum row that the method always performs.** Useful for classes whose methods are inherently effectful:

```flux
public class MonadIO<m> {
    fn lift<a>(action: () -> a with <IO>) -> m<a> with <IO, ..>
}
```

`<IO, ..>` means "at least `IO`, plus whatever the caller adds." An instance body must perform *at least* `IO`; it may perform more.

### Novel Flux syntax: `instance ... with <row>` for instance-level effect pinning

This is new — not present in Haskell, Koka, or PureScript — and gives Flux something the others lack: **the ability to pin an instance to a concrete effect row when the class default is row-polymorphic.**

```flux
// class is row-polymorphic
public class Logger<a> {
    fn log(x: a) -> Unit with _
}

// a pure instance — no effect
public instance Logger<NullDevice> with <> {
    fn log(_) { () }
}

// an IO instance — pinned
public instance Logger<StdoutHandle> with <IO> {
    fn log(h) { stdout_write(h, format(h)) }
}

// an instance pinned to a custom effect
public instance Logger<TraceBuffer> with <Trace> {
    fn log(buf) { trace_append(buf, format(buf)) }
}
```

Effect pinning gives three wins:

1. **Call-site effect inference is exact.** When `Logger.log(stdout)` resolves monomorphically to `__tc_Flow_Log_Logger_StdoutHandle_log`, the callee's pinned row `<IO>` is added to the caller's row automatically. No row-variable propagation needed — the compiler already knows the concrete effect.
2. **Mangling stays stable.** The pinned row is *not* part of the mangled name (which stays `__tc_<mod>_<class>_<type>_<method>`), because each `(class, type)` pair still has a single canonical instance. The row is metadata on the instance, consulted during unification.
3. **Instance coherence is preserved.** Pinning is not overlap. You still cannot have two instances of `Logger<StdoutHandle>`; pinning attaches a row to the *one* instance.

Rules:

- The `with <row>` on the instance must be a *subtype* of the class method's declared row. For `with _` classes, any row satisfies. For `with <E, ..>` classes, the pinned row must contain `E`. For `with <>` classes, pinning is forbidden (`E453` — pinning a pure class).
- The pinned row applies uniformly to **every** method of the instance. If different methods of the same instance need different rows, factor the class.
- The pinned row is reflected in `.flxi` so downstream modules can inline-resolve effect rows across compilation units.

### Constraints with effect rows

Class constraints on function signatures can reference a class whose methods are row-polymorphic without repeating the row:

```flux
// the row variable propagates automatically
fn sum_all<a: Foldable, e>(xs: a<Int>) -> Int with e {
    Foldable.fold(xs, 0, fn(acc, x) { acc + x })   // callback is pure; e = <>
}

fn log_all<a: Foldable, e>(xs: a<String>) -> Unit with <IO, e> {
    Foldable.fold(xs, (), fn(_, s) { print(s) })   // callback does IO; e >= <IO>
}
```

This is just standard HM effect row inference; 0151 makes it work at class method boundaries by *not* hard-coding `<>` on class method signatures.

### Interaction with `IO`, `Handler`, and named effect handlers

When a class method's inferred row includes a named effect like `<Reader[Config]>`, the caller must have that effect in scope, same as any function call. Class methods do not bypass effect checking. The `try_resolve_class_call` path in [src/core/lower_ast/mod.rs](src/core/lower_ast/mod.rs) must be extended to unify the resolved instance's row with the call site's row and surface the usual E3xx row-mismatch diagnostics on failure.

### Diagnostic summary for effects

- `E452` — instance body performs an effect not permitted by the class method's declared row.
- `E453` — instance pins a row on a pure (`with <>`) class.
- `E454` — instance's pinned row does not satisfy the class's minimum row (`with <E, ..>`).

---

## Aether / Perceus Interaction

Flux's Aether pass (`src/aether/`) performs Perceus-style reference-counting: borrow inference, dup/drop insertion, drop specialization, dup/drop fusion, and reuse insertion. Module-scoped classes must slot into this pipeline without special cases. The good news is that 0145's dispatch strategy already lines up well; the detailed rules below formalize what already works and patch the two places where visibility can break assumptions.

### Pipeline position

Aether runs **after** dictionary elaboration and Core simplification. Concretely, the order in [src/core/passes/mod.rs](src/core/passes/mod.rs) is:

1. Stage 0.5 — `elaborate_dictionaries()` (dict construction, method calls → `TupleField`)
2. Stages 1–2 — Core simplification (beta, case-of-case, inline_lets, ...)
3. Stage 3 — Aether passes: borrow inference → dup/drop insertion → drop specialization → dup/drop fusion → reuse insertion → reuse specialization

By the time Aether sees a class method, it is already either:

- A direct call to a mangled `__tc_<mod>_<class>_<type>_<method>` function (monomorphic fast path), or
- A `TupleField(dict, method_index)` extraction followed by an indirect call (polymorphic path).

Neither form is special to Aether. Mangled instance methods are ordinary `CoreDef`s with ordinary ANF bodies. Indirect calls through dict tuples are ordinary tuple-field + call sequences. This is the central invariant and 0151 preserves it.

### Borrow inference over mangled methods

The borrow registry in [src/aether/borrow_infer.rs:186-250](src/aether/borrow_infer.rs#L186-L250) infers parameter modes (Owned / Borrowed) by analyzing function bodies and iterating to a fixed point across SCCs. Mangled instance functions participate like any other function. Empirical evidence: the existing snapshot [tests/snapshots/aether/aether__aether__borrow_calls__dump_core.snap](tests/snapshots/aether/aether__aether__borrow_calls__dump_core.snap) shows `__tc_Semigroup_String_append` inferred as `aether_call[borrowed, borrowed] string_concat(...)` with no class-specific handling.

0151's only requirement: **the borrow registry must see every mangled `__tc_*` method, including those generated for private classes and private instances.** Today this works by accident — `class_dispatch::generate_from_statements` injects every dispatch function regardless of visibility. Under 0151 we formalize it:

> **Invariant A (Aether visibility):** Private classes and private instances still emit their mangled `__tc_*` functions into `CoreProgram.defs`. Visibility is enforced at *resolution* (who can write the call), not at *lowering* (who emits the body). The borrow registry is populated from `CoreProgram.defs`, so all class methods have inferred modes regardless of public/private status.

Without this invariant, a caller-site `TupleField(dict, idx)` pointing at a private method would fall into the `BorrowCallee::Unknown` branch at [src/aether/borrow_infer.rs:287-290](src/aether/borrow_infer.rs#L287-L290) and conservatively mark every argument as `Owned`, forcing spurious `dup`s and losing the zero-alloc fast path.

### Dictionary tuples and dup/drop

Dictionaries are built by [src/core/passes/dict_elaborate.rs:136-140](src/core/passes/dict_elaborate.rs#L136-L140) as `MakeTuple(Var(__tc_*), ...)`. They are global constants — never modified, only read. Aether must treat dictionary globals as **borrowed on access**: a `TupleField(dict, idx)` read must not emit `dup` on `dict`, because `dict` is never consumed.

> **Invariant B (dictionary borrow):** `CoreDef`s whose body is a `MakeTuple` of function references and whose name begins with `__dict_` are tagged `BorrowMode::Borrowed` for all their read sites. Dup/drop insertion skips them.

This matches the existing behavior at [src/aether/insert.rs:80-87](src/aether/insert.rs#L80-L87) but makes it an explicit named invariant so future refactors can't silently regress it.

### dup/drop on class method arguments

When Aether encounters `fold(xs, init, func)` that resolved to `__tc_Flow_Foldable_Foldable_List_fold`, the dup/drop pattern depends on the borrow registry entry for that mangled function, which is in turn inferred from the instance body.

- If the body passes `xs` straight through to `List.foldl` (which has `[borrowed, owned, borrowed]`), then `__tc_Foldable_List_fold` inherits the same modes, and the call site emits no `dup` for `xs`.
- If the body captures `xs` into a closure and returns it (consumption), the inferred mode becomes `Owned` and the call site emits a `dup` for `xs` if it's used again after the call.

**Important:** Because the mangled function name is stable per `(mod, class, type, method)`, the borrow signature is stable across the whole program. Aether does not need to re-infer it per call site; it can cache on the mangled name. This caching is already how `BorrowRegistry` works — 0151 doesn't change it.

### Indirect calls through dict tuples

Polymorphic call sites look like:

```text
let m = TupleField(dict, 0)
m(xs, init, func)
```

For the indirect call `m(...)`, Aether cannot statically pick an instance, so borrow modes fall back to a **pessimistic default** computed as the *meet* (least-upper-bound under the `Owned ⊒ Borrowed` lattice) of every instance's signature for that method. The meet is computed once per class method during dictionary elaboration and stored alongside the class definition.

> **Invariant C (polymorphic meet):** For each class method, Aether precomputes `meet_mode = ⊔ { mode(inst, method) | inst in instances_of(class) }`. Indirect calls through `TupleField(dict, idx)` use `meet_mode`. Direct calls to mangled functions use the per-instance mode.

This gives monomorphic call sites the tightest possible dup/drop pattern while keeping polymorphic sites sound. It does not exist in the current code — it is the one genuinely new piece of Aether machinery 0151 adds.

### fip / fbip interaction

Flux does not currently have `fip` / `fbip` annotations (Koka-style fully-in-place / frame-bounded in-place). 0151 does not add them. However, the per-instance borrow signature computed above is exactly the evidence a future `fip`-analysis pass would need, and Invariant C ensures polymorphic calls can be safely excluded from `fip`-verified regions. This is future work and explicitly out of scope.

### What breaks if we skip the invariants

| Skipped invariant | Failure mode |
|-------------------|--------------|
| Invariant A (Aether visibility) | Private class methods become `Owned`-conservatively; spurious `dup`s; lost fast path |
| Invariant B (dictionary borrow) | Every class method call emits `dup/drop` on the dictionary global; huge overhead on polymorphic call sites |
| Invariant C (polymorphic meet) | Either polymorphic calls are unsound (if we use monomorphic modes) or monomorphic calls are pessimistic (if we use the meet everywhere) |

### Compiler changes in Aether

Concrete work items:

1. **[src/aether/borrow_infer.rs](src/aether/borrow_infer.rs)** — add `meet_mode` precomputation for class methods, consumed by indirect calls.
2. **[src/aether/borrow_infer.rs](src/aether/borrow_infer.rs)** — thread `ClassId` into the borrow registry so visibility-suppressed methods stay discoverable by their mangled name.
3. **[src/core/passes/dict_elaborate.rs](src/core/passes/dict_elaborate.rs)** — tag generated `__dict_*` defs with a `BorrowMode::Borrowed` hint that Aether reads (Invariant B).
4. **[src/types/class_env.rs](src/types/class_env.rs)** — record visibility on `ClassDef` and `InstanceDef`, but do *not* gate code generation on it (Invariant A).

No changes required to drop specialization, dup/drop fusion, or reuse insertion — they operate on post-inference ANF and are oblivious to where the borrow modes came from.

---

## Example Lowering

Source:

```flux
import Flow.Foldable as Foldable

fn main() {
    Foldable.fold([1, 2, 3], 0, fn(acc, x) { acc + x })
}
```

Lowering intent:

- Recognize `Foldable.fold` as a module-qualified class method.
- Resolve `Foldable` alias → module `Flow.Foldable`.
- Look up `fold` in `Flow.Foldable`'s class method table → `ClassId(Flow.Foldable, Foldable)`, method index 0.
- Infer first argument type `List<Int>`.
- Resolve instance `Foldable<List>` against the visible instance set.
- Lower to:

```text
__tc_Flow_Foldable_Foldable_List_fold([1,2,3], 0, <closure>)
```

---

## Backward Compatibility

### Kept

- Existing global `fold`, `length`, `to_list` and other prelude helpers.
- Existing unqualified user code that does not rely on module-scoped classes.
- Existing built-in class machinery and dictionary elaboration from 0145.
- Top-level `class` / `instance` parses and compiles (with deprecation warning).

### Changed

- Mangled names for all class instance functions and dictionary globals (now include module path). `.fxc` caches will be invalidated on upgrade.
- `ClassEnv` internal data structures (class identity by `ClassId`).
- Module files may legally contain class/instance declarations with `public`/private visibility.

### Removed (after migration window)

- Top-level `class` / `instance` outside a module body (deprecation → hard error two releases after 0151 lands).

---

## Phased Implementation Plan

The proposal is large enough that landing it as a single change would be risky: it touches the parser, type inference, class environment, module member collection, the `.flxi` / `.fxc` cache, symbol mangling, effect rows, Aether, ADTs, and the entire stdlib. Each of the seven phases below is **independently landable, testable, and reviewable**, and each leaves the tree in a shippable state. Phase order is chosen so that later phases never rip out work from earlier ones.

### Phase dependency graph

```text
Phase 0 (Preflight spikes)
   │
   ▼
Phase 1 (Foundation)
   │
   ├──► Phase 2 (Coherence + ADTs + .flxi)
   │       │
   │       ├──► Phase 3 (Unqualified access via exposing)
   │       │
   │       ├──► Phase 4 (Effects on instance methods)
   │       │
   │       └──► Phase 5 (Aether invariants)
   │
   └──► (all of phases 2–5 must land before)
           │
           ▼
        Phase 6 (Stdlib migration + soft deprecation)
           │
           ▼
        Phase 7 (Hard deprecation)
```

Phases 3, 4, and 5 are independent of each other after phase 2, so they can proceed in parallel or in any order based on team bandwidth.

### Phase 0 — Preflight spikes

**Goal.** De-risk the three genuine unknowns in the proposal before committing to Phase 1. Each spike is a short, throwaway investigation whose only output is a clear green / yellow / red signal on a specific design decision. No code is merged from Phase 0 except possibly small scaffolding commits if all spikes come back green.

**Landed when.** All three spike questions below have a documented answer and the findings are reflected back into the proposal if needed (adjusting later phase plans based on what was learned).

**Spike 0.1 — `ClassMethod` effect row capacity.**

- Question: does the existing `ClassMethod` struct (inside `Statement::Class`) already carry an effect row field analogous to `Statement::Function.effects`? If yes, Phase 4 obstacle #1 is free. If no, Phase 4 must add the field and thread it through parsing and inference.
- Action: read [src/syntax/type_class.rs](src/syntax/type_class.rs) and [src/syntax/parser/statement.rs](src/syntax/parser/statement.rs) `parse_class_method` (line 1394). Look for `effects: Vec<EffectExpr>` or equivalent.
- Decision gate: **green** = field exists, no extra work. **yellow** = field missing but AST trivially extensible. **red** = structural refactor required (unlikely).
- Time budget: 30 minutes.

**Spike 0.2 — `ClassEnv` call-site inventory.**

- Question: how many call sites does the `HashMap<Identifier, ClassDef>` → `HashMap<ClassId, ClassDef>` refactor touch? This sets the concrete size of Phase 1's mechanical work and tells us whether to do it as one PR or split across several.
- Action: `grep -rn "class_name: Identifier\|classes: HashMap\|ClassEnv::lookup\|instances_for" src/` and count distinct files and occurrences. Categorize by subsystem (type infer, core lowering, dispatch, dict elaborate, cfg, bytecode).
- Decision gate: **green** = ≤30 sites across ≤6 files, one PR. **yellow** = 30-80 sites across 6-12 files, split into preparation + refactor PRs. **red** = 80+ sites, needs a `ClassKey` newtype trick to alias the old API during a gradual migration.
- Time budget: 30 minutes.

**Spike 0.3 — Effect row unification on per-instance pinning.**

- Question: can Flux's existing unifier at [src/types/unify.rs](src/types/unify.rs) attach a concrete effect row (e.g. `<IO>`) to a row variable that was introduced at class-declaration time? This is the load-bearing mechanic for the novel `instance ... with <row>` syntax in §Effects. If the unifier can't do this, we need a different surface syntax.
- Action: write a throwaway test in `tests/` that constructs a synthetic class method with a row variable `e`, then attempts to unify it against a concrete row `<IO>` via `ClassEnv::resolve_instance_with_subst` or the lower-level unify entry point. Observe whether the substitution sticks and whether downstream `TypeSubst::apply` propagates it.
- Decision gate: **green** = unification succeeds and the substitution propagates. **yellow** = unification succeeds but needs manual substitution threading. **red** = row variables at method sites are second-class and can't be pinned; Phase 4 needs redesign.
- Time budget: 2-3 hours (most of the Phase 0 budget).

**Files touched.** None merged. All spike work is in throwaway branches or in commented-out scratch tests. The only artifact merged from Phase 0 is an **update to this proposal** reflecting each spike's findings (e.g., "Phase 4 obstacle #6 resolved, unifier handles it" or "Phase 4 redesign needed, see §14a").

**Tests required.** Spike 0.3 writes a unit test that stays in a scratch branch unless it's green, in which case it can be cleaned up and landed as the first Phase 4 regression test.

**Risk.** By construction, Phase 0 is zero-risk — it's investigation, not code change. Its purpose is to convert unknown risk in later phases into known risk.

**Not in this phase.** Any refactor, any AST change, any new module. Phase 0 is read-only except for the throwaway unification test.

### Phase 0 findings (2026-04-09)

Phase 0 was executed and resolved all three spikes. Summary below; each spike's conclusion is reflected in the affected later phase.

**Spike 0.1 — `ClassMethod` effect row capacity: RED.**

- `ClassMethod` at [src/syntax/type_class.rs:27-38](src/syntax/type_class.rs#L27-L38) has no `effects` field. Only `name`, `type_params`, `params`, `param_types`, `return_type`, `default_body`, `span`.
- `parse_class_method` at [src/syntax/parser/statement.rs:1394-1500](src/syntax/parser/statement.rs#L1394-L1500) has no grammar for `with <row>` — after the return type it jumps straight to the optional default body.
- **Impact on Phase 4:** the AST and parser work to add effect rows to class methods is **not free**. Add to Phase 4's scope: one new field on `ClassMethod`, one new field on `InstanceMethod` (for pinning), grammar extensions to `parse_class_method` and `parse_instance_statement`.
- **Mitigation:** the AST change is mechanical and small (~50 lines). No design redesign required. Phase 4 risk remains *medium-high* but for a different reason — the type-system-invasive work is the constraint solver integration, not the AST.

**Spike 0.2 — `ClassEnv` call-site inventory: YELLOW.**

- Pattern `class_name: Identifier` appears in **19 definition sites across 9 files**.
- Patterns `.class_name`, `.classes.`, `ClassEnv::`, `instances_for`, `resolve_instance`, `lookup_class` appear in **98 call sites across 15 files**.
- Hot concentration: `class_env.rs` (39), `dict_elaborate.rs` (16), `class_dispatch.rs` (10), `bytecode/compiler/*` (~15), `ast/type_infer/*` (~9).
- **Impact on Phase 1:** the `ClassId` refactor is real work. Falls in the YELLOW band of the Phase 0 decision gate.
- **Mitigation:** **Split Phase 1 into sub-phases 1a and 1b**:
  - **Phase 1a:** introduce `ClassId` as a newtype wrapper around `(ModulePath, Identifier)`, add a parallel `ClassEnv` API (`lookup_class_by_id`, `instances_for_id`) alongside the existing `Identifier`-keyed API. Internal storage still uses bare `Identifier`; new API proxies through a synthetic `ClassId` with an empty `ModulePath`. No behavior change. Reviewable in one PR, ~30 sites.
  - **Phase 1b:** flip primary storage to `HashMap<ClassId, ClassDef>`, migrate all call sites to the new API, delete the compatibility shim. ~70 sites, reviewable in one PR.
- This split is added as a normative change to the Phase 1 description below.

**Spike 0.3 — Effect row unification on per-instance pinning: GREEN.**

- [src/types/unify.rs:312-371](src/types/unify.rs#L312-L371) already implements `unify_effect_rows` with full Rémy-style row unification: closed↔closed, open↔closed (tail absorbs the difference), open↔open (introduces residual row variable).
- [src/types/infer_effect_row.rs:27-30](src/types/infer_effect_row.rs#L27-L30) represents rows as `{ concrete: HashSet<Identifier>, tail: Option<TypeVarId> }` — exactly the Rémy representation needed for pinning.
- Function types already carry effect rows via `InferType::Fun(params, ret, effects)` at [src/types/unify.rs:123](src/types/unify.rs#L123).
- The existing test `unify_fun_types_with_open_effect_row_binds_tail` at [src/types/unify.rs:756-771](src/types/unify.rs#L756-L771) **is literally the instance-pinning scenario**: an open-row function unifies against a closed-row function by binding the tail variable to the difference.
- **Impact on Phase 4:** the novel `instance ... with <row>` syntax is backed by machinery that already exists and is already tested. No unifier work, no subst-propagation work. Phase 4's risk drops from *medium-high* to *medium* — all remaining risk is in the constraint generator and call-site row threading, not in unification.
- **Mitigation:** none needed; the green signal is decisive.

### Phase 0 net effect on the plan

- Phase 1 is split into **1a** (prepare) and **1b** (flip storage).
- Phase 4 keeps its scope but shifts the risky work from "unifier integration" to "constraint generator integration."
- Total phase count grows from 8 to 9 (phases 0, 1a, 1b, 2, 3, 4, 5, 6, 7).
- Overall proposal risk is **lower** post-spike than it was at proposal time, because the single genuinely unknown question (Spike 0.3) came back green.

---

### Phase 1 — Foundation ✅ COMPLETE (2026-04-09)

**Status.** Phase 1 landed in 9 incremental commits across two sub-phases (1a and 1b), driven by the call-site count from Spike 0.2:

- **Phase 1a (commits #1–#6)** — surface-level work: module-body validator whitelist, `ModulePath`/`ClassId` types and parallel `ClassEnv` API, `is_public` field on `Statement::Class`/`Instance`, end-to-end runtime smoke test, `public class`/`public instance` parsing, qualified `Module.method(...)` dispatch through the bytecode compiler.
- **Phase 1b (commits #1–#4)** — internal storage refactor: `ClassDef.module` populated during collection, `InstanceDef.instance_module` and `InstanceDef.class_id` populated during `collect_instances` and `collect_deriving`, **storage flipped to `HashMap<ClassId, ClassDef>`**, disambiguation rule (`lookup_class_in_module_or_global`) for same-named classes, tightened `instances_for_id` and `resolve_instance_with_subst_by_id` to filter by `class_id`.

**Test count delta.** Suite grew from 1805 to 1830 over the nine commits (**+25 tests, 0 regressions**), including these load-bearing proof tests:

- `module_scoped_class_with_int_instance_runs_via_existing_dispatch` — runtime end-to-end smoke
- `qualified_call_full_dotted_form` and `qualified_call_via_import_alias` — qualified resolution headline
- `two_classes_with_same_short_name_in_different_modules_coexist` — Phase 1b Step 3 storage flip proof
- `instances_for_id_returns_disjoint_buckets_for_same_named_classes` and `resolve_instance_with_subst_by_id_respects_class_id` — Phase 1b Step 4 ClassId-keyed lookup proof
- `module_scoped_deriving_records_owning_module` — `deriving` clauses inherit owning module

**Deferred to later phases.** The originally-listed `src/types/mangle.rs` extraction did not happen — mangled name construction still uses ad-hoc `format!` strings in `class_dispatch.rs` and `expression.rs`. This is orthogonal to the rest of Phase 1 and can be folded into Phase 6 (stdlib migration) or done as a standalone refactor. The `ClassConstraint` and IR-type migrations to `ClassId` were also deferred — they're not load-bearing for Phase 2 work because the existing `class_id`-keyed lookups on `ClassEnv` already disambiguate at the resolution boundary.

---

**Goal.** Make module-body `class` / `instance` declarations parse, infer, and lower, with globally unique class identity and the new symbol mangling. No semantic enforcement yet — you can write an orphan instance and it will compile.

**Landed when.**
- `module M { public class C<a> { ... } public instance C<T> { ... } }` parses and compiles. ✅
- `Alias.method(...)` qualified calls resolve. ✅
- Two different modules can each declare a class called `Foldable` without colliding mangled names. ✅ (proof test: `two_classes_with_same_short_name_in_different_modules_coexist`)
- `import` at both file level and module body works. ✅
- Top-level legacy `class` / `instance` still works unchanged. ✅

**Files touched.**
- [src/syntax/parser/statement.rs](src/syntax/parser/statement.rs) — accept `class` / `instance` with optional `public` inside module bodies.
- [src/ast/statement.rs](src/ast/statement.rs) — add `visibility: Visibility` to `Statement::Class` and `Statement::Instance`.
- [src/types/class_env.rs](src/types/class_env.rs) — `ClassId = (ModulePath, Identifier)`, `HashMap<ClassId, ClassDef>`, `InstanceDef.class_id`.
- [src/types/class_dispatch.rs](src/types/class_dispatch.rs) — mangled name generation uses the new symbol encoding.
- [src/types/mangle.rs](src/types/mangle.rs) — **new module**: the encoding rules from §Symbol Encoding, with inline unit tests.
- [src/core/lower_ast/mod.rs](src/core/lower_ast/mod.rs) — `try_resolve_class_call` handles `ModuleAlias.method(...)` forms.
- [src/syntax/module_graph.rs](src/syntax/module_graph.rs) — module-body imports as distinct scopes from file-level imports.

**Error codes introduced.** None yet (enforcement comes in phase 2).

**ABI break.** Yes — mangled-name format changes for all class instance functions and dictionary globals. Cached `.fxc` files are invalidated on upgrade. Release notes must call this out.

**Tests required.**
- All parser / validation tests from the main test plan.
- All symbol encoding tests (§Symbol Encoding test cases 1–10).
- Smoke test: a module with two classes, two instances, two functions compiles and runs on both VM and LLVM backends.
- Show_Show regression test from the encoding table.
- Two-modules-same-class-name test.

**Risk.** Medium. The `ClassId` refactor touches everything downstream but is mechanical. The mangling change is a one-time ABI break — painful but done once. The parser work is small.

**Not in this phase.** Orphan rule enforcement, visibility enforcement, `.flxi` serialization, effect rows on methods, Aether work, `exposing`, stdlib migration, deprecation.

---

### Phase 2 — Coherence (orphan rule + visibility + `.flxi` + ADTs)

**Goal.** Make the semantic rules that protect the cache actually fire. After this phase, the `.fxc` cache is sound under module-scoped classes, and orphan instances are rejected.

**Note on error codes.** Phase 2 was originally drafted to use `E447`, `E448`, ..., but `E447` and `E448` were already taken in `compiler_errors.rs` by `INSTANCE_TYPE_ARG_ARITY` / `INSTANCE_METHOD_ARITY`. Phase 2's new diagnostics start at the first free code, `E449` (orphan), and the visibility / ambiguity / private-leak diagnostics shift up accordingly: `E450` (visibility-public-of-private-class), `E451` (public-class-mentions-private-type), `E455` (public-instance-for-private-ADT), `E456` (short-name constraint ambiguity).

**Landed when.**
- ✅ **`instance C<T>` in a third module (neither class's nor type's owning module) is rejected with `E449`.** *(landed 2026-04-09)*
- ✅ **A `public instance` of a private class is rejected with `E450`.** *(landed 2026-04-09)*
- ✅ **A `public class` whose signature mentions a private type is rejected with `E451`.** *(landed 2026-04-09)*
- ✅ **A `public instance` of a `public class` for a private ADT is rejected with `E455`.** *(landed 2026-04-09)*
- ✅ **Two `public instance`s of the same `(ClassId, head_type)` in different modules are rejected as duplicates (`E443` extended).** *(landed 2026-04-09 — the existing dedup gate already keys on `class_id` + structural type args, which is module-blind by design; the diagnostic now adds a hint surfacing the existing instance's owning module when it differs from the new one)*
- ✅ **`.flxi` round-trips `public class` and `public instance` entries with `ClassId`, superclasses, and pinned rows placeholder.** *(landed 2026-04-09)*
- ✅ **`.fxc` cache hash changes when a directly-imported module adds/removes/modifies a `public class` or `public instance`, and does *not* change on private additions.** *(landed 2026-04-09 — same commit as `.flxi`; the fingerprint computation now folds the public-class/instance tables in)*
- ✅ **Short-name constraint ambiguity (`<a: Foldable>` when two `Foldable`s are in scope) fires `E456`.** *(landed 2026-04-09)*
- ✅ **`deriving` clauses on `data` generate instances in the data's own module (trivially orphan-compliant) using the new `ClassId`.** *(orphan-walker exemption verified 2026-04-09)*

**Files touched.**
- [src/types/class_env.rs](src/types/class_env.rs) — orphan rule walker, visibility fields enforced on lookups.
- [src/types/module_interface.rs](src/types/module_interface.rs) — `.flxi` schema extended for classes and instances.
- [src/bytecode/compiler/](src/bytecode/compiler/) — `.fxc` hash inputs include the class/instance table.
- [src/ast/type_infer/class_solver.rs](src/ast/type_infer/class_solver.rs) — ambiguity diagnostic `E448`.
- [src/types/class_dispatch.rs](src/types/class_dispatch.rs) — `deriving` emits under the new `ClassId` and respects ADT visibility.
- [src/diagnostics/](src/diagnostics/) — `E449`, `E450`, `E451`, `E455`, `E456`.

**Error codes introduced.** `E449`, `E450`, `E451`, `E455`, `E456`.

**Tests required.**
- Orphan rule: positive and negative cases for each of "class-local", "type-local", "third-module" placements, for both hand-written and `deriving`-generated instances.
- Visibility: every row of the visibility × visibility table from §12.
- `.flxi` round-trip tests.
- Cache invalidation tests (public adds invalidate; private adds don't).
- Ambiguous short-name constraint test.

**Risk.** Low-to-medium. The orphan rule walker is small. The `.flxi` schema extension is the most invasive piece but is a pure data change.

**Not in this phase.** `exposing`, effect rows on methods, Aether invariants, stdlib migration, deprecation.

---

### Phase 3 — Unqualified access via `exposing`

**Goal.** Let users write bare `fold(xs, ...)` when they've explicitly asked for it via `exposing`, and formalize the inside-the-defining-module shadowing rule.

**Note on error codes.** Phase 3 was originally drafted to use `E449` and `E456`, but Phase 2 absorbed both during the cascade (E449 = orphan, E456 = ambiguous constraint). Phase 3's collision diagnostics start at the next free codes, **`E457`** (exposing-vs-local-collision) and **`E458`** (file-vs-module-body import collision). E452–E454 remain reserved for Phase 4 (effects).

**Landed when.**
- ✅ **Parser already accepts `exposing (..)` and `exposing (name, name, ...)` on imports** *(predates Phase 3 — exists since the original `ImportExposing` enum)*
- ✅ **Bytecode compiler routes `exposing`-listed module functions into bare-name scope** *(predates Phase 3 — `expose_imported_native_symbols` and `build_preloaded_borrow_registry` already handle `ImportExposing::Names`)*
- ✅ **`lookup_class_method` resolves bare class method calls without an explicit `exposing` clause when the class is in scope** *(predates Phase 3 — see `infer_call`'s pre-inference class-method lookup)*
- ✅ **A name collision between `exposing (foo)` and a local top-level `foo` is rejected with `E457`.** *(landed 2026-04-09)*
- ✅ **A file-level vs module-body import that bind the same short name to different targets is rejected with `E458`.** *(landed 2026-04-09)*
- ✅ **Inside `module Flow.Foldable`, a default method body can call `fold(...)` unqualified and reach the local class's own method (not any legacy global).** *(landed 2026-04-09 — verified by `inside_module_default_method_resolves_to_sibling`; the existing `lookup_class_method` short-name lookup already implements the required semantics, and the new walker confirmed it doesn't trip the new collision checks)*

**Deferred to a follow-up.** A bare `fold` call falling back to "type-directed instance resolution, not the legacy global" — i.e. having an explicit `exposing` clause **suppress** a legacy top-level `fold` and route through dictionary dispatch instead. This is the only Phase 3 bullet that requires a behavior change to existing name resolution priority and is held back until stdlib migration (Phase 6) can adjust the legacy globals at the same time. The collision diagnostics (E457/E458) make the current behavior safe in the meantime: if the user creates an ambiguous setup the compiler tells them to disambiguate explicitly.

**Files touched.**
- [src/diagnostics/compiler_errors.rs](src/diagnostics/compiler_errors.rs) — `EXPOSING_LOCAL_COLLISION` (E457), `IMPORT_NAME_COLLISION_FILE_VS_MODULE` (E458).
- [src/diagnostics/registry.rs](src/diagnostics/registry.rs) — register both, route to `ModuleSystem` category.
- [src/bytecode/compiler/passes/import_validation.rs](src/bytecode/compiler/passes/import_validation.rs) — *new*. Walks file-level and module-body imports, builds local name sets and exposed-name maps, fires E457/E458.
- [src/bytecode/compiler/passes/collection.rs](src/bytecode/compiler/passes/collection.rs) — invokes the new pass after `collect_class_declarations`.
- [src/bytecode/compiler/passes/mod.rs](src/bytecode/compiler/passes/mod.rs) — declares the new pass module.

**Error codes introduced.** `E457`, `E458`.

**Tests required.**
- `exposing` brings in the expected names and only those names.
- Collision tests for `E449` and `E456`.
- Inside-the-defining-module shadowing test (critical — confirms default methods can call sibling class methods unqualified).

**Risk.** Low. This phase is localized to name resolution and does not touch mangling, Aether, or the cache.

---

### Phase 4 — Effects on instance methods

**Status as of 2026-04-10:** ✅ **done** — **Phase 4 — Module-Scoped Type Classes** is complete. Pre-phase spike, Phase 4a-prereq, Phase 4a, Phase 4b, and Phase 4c have all landed, and the final acceptance bar is covered by executable regressions for module-scoped type classes.

| Sub-commit | Description | Status |
|---|---|---|
| **Pre-phase spike** | Three parallel investigations (Flux effect system audit, Koka effect-on-class semantics, Haskell mtl/polysemy patterns). Conclusion: existing effect-row grammar is sufficient, no new syntax required. | ✅ done 2026-04-09 |
| **Phase 4a-prereq** | Whitelist `Statement::EffectDecl` in the module-body validator at [src/bytecode/compiler/statement.rs:1685](src/bytecode/compiler/statement.rs#L1685). Lets `effect Foo { ... }` declarations live inside `module { ... }` blocks. | ✅ done 2026-04-09 (1-line change + 3 integration tests, all green) |
| **Phase 4a** | `ClassMethod.effects` + `InstanceMethod.effects` AST fields, 2 `parse_effect_list()` calls, `MethodSig.effects`, Phase 1b mangling-pass field assignment, Rule 1 walker (E452). | ✅ done 2026-04-10 |
| **Phase 4b** | `try_resolve_class_call` propagates the resolved instance's effect row into the caller's ambient row. | ✅ done 2026-04-10 |
| **Phase 4c** | Row-polymorphic class methods (`with \|e`) end-to-end. | ✅ done 2026-04-10 |

**Side discoveries from Phase 4a-prereq.** `handle` is a reserved keyword in Flux's effect handler postfix syntax (`expr handle Effect { ... }`) and cannot be used as a parameter name. The four worked examples below have been updated to use `hnd: h` rather than `handle: h`. Other reserved-keyword parameter names to avoid in tests: likely `effect`, `perform`, `resume` — to be verified incrementally as they're encountered.

**Goal.** Let class methods and instance methods carry the **same effect-row syntax that regular Flux functions already use**, with the resolved instance's effect row propagating to call sites via type-directed dispatch. This is the biggest user-visible win of the proposal after Phase 1.

**Pre-phase spike findings (2026-04-09).** Three parallel investigations of (a) Flux's existing effect machinery, (b) Koka's effect-on-class story, (c) Haskell's mtl/polysemy patterns concluded that **Flux's existing function-level effect-row grammar is sufficient for class methods — no new syntax, tokens, or keywords are required**. Earlier drafts of this proposal used fictional notation (`with <>` / `with _` / `with <E, ..>`); those forms do not parse and have been removed.

#### The seven real Flux effect forms (Phase 4 reuses *all* of them verbatim)

Defined by `EffectExpr` in [src/syntax/effect_expr.rs](src/syntax/effect_expr.rs) and parsed by `parse_effect_list()` in [src/syntax/parser/helpers.rs:730](src/syntax/parser/helpers.rs#L730):

| # | Form | Meaning | Where it appears today |
|---|---|---|---|
| 1 | (omit `with`) | Pure — closed empty row | `fn add(x, y) { x + y }` |
| 2 | `with IO` | Single concrete effect | [lib/Flow/IO.flx](lib/Flow/IO.flx) |
| 3 | `with IO, Time` | Multiple concrete (comma-separated) | [examples/aoc/2024/day03.flx](examples/aoc/2024/day03.flx) |
| 4 | `with IO + Time` | Multiple concrete (`+` operator, equivalent to comma) | [examples/type_system/32_effect_poly_mixed_io_time_ok.flx](examples/type_system/32_effect_poly_mixed_io_time_ok.flx) |
| 5 | `with \|e` | Row-polymorphic — named row tail variable | [examples/guide_type_system/06_hof_with_e_compose.flx](examples/guide_type_system/06_hof_with_e_compose.flx) |
| 6 | `with IO \| e` | Lower bound — concrete labels followed by row tail | [examples/type_system/32_effect_poly_mixed_io_time_ok.flx](examples/type_system/32_effect_poly_mixed_io_time_ok.flx) |
| 7 | `with \|e - IO` | Row subtraction — "any row that excludes IO" | [examples/type_system/102_effect_row_subtract_var_satisfied_ok.flx](examples/type_system/102_effect_row_subtract_var_satisfied_ok.flx) |

`unify_effect_rows` in [src/types/unify.rs:312](src/types/unify.rs#L312) already implements Rémy-style row unification across all seven forms. **No new tokens, no new keywords, no new AST variants.**

#### Semantic model — the class declaration is a *floor*, not a ceiling

This is the load-bearing design choice. When a class method declares an effect row `C` and an instance method declares row `I`, the relationship is:

> **`C ⊆ I`** — the instance must include every effect the class promises, and may add more.

The class's declared row is a **floor**, not a ceiling. Consequences:

- **Pure-by-default everywhere.** Omit `with` on a class method → floor is `<>` (closed empty), so any instance row is allowed (since `<>` is a subset of every row).
- **Instances may add effects** beyond what the class declares, **but must annotate them explicitly**. Silent inference is *not* allowed for instance methods that exceed the class's row.
- **Callers see the resolved instance's row, not the class's row.** When `eq(user1, user2)` resolves via type-directed dispatch to `Eq<UserId>::eq`, the caller's ambient effect row gains whatever effects that specific instance declares — not the class's row. The class is just a method-shape contract; the actual effects come from the instance.
- **No "sealed class" syntax in this phase.** If users later want "this class is locked pure forever, no instance may ever add effects", that's a Phase 4d follow-up (perhaps `sealed class Eq<a> { ... }`). For now the floor model gives strictly more flexibility, and pure-by-default still makes the common case ergonomic.

#### The two checks Phase 4 introduces

| Check | Rule | Diagnostic | Where it runs |
|---|---|---|---|
| **Rule 1** | Class method's declared row `C` is a subset of the instance method's declared row `I`. | `E452` — "instance method `Class<T>::method` is missing effects required by the class declaration" | `class_env::collect_instances` walker (no inference needed — pure annotation-vs-annotation comparison) |
| **Rule 2** | Instance method's body has inferred row `B`, and `B ⊆ I`. | Reuses Flux's existing function-effect-mismatch error (no new code, just runs `__tc_*` mangled instance methods through the same check that already runs on top-level functions) | `phase_type_inference` — automatic, since instance methods become mangled top-level functions after Phase 1b dispatch generation |

**Rule 1** is the new piece: it catches `class Logger<h> { fn log(h: h, msg: String) with IO | e }` paired with an instance whose method declares `with Time` only (missing `IO`).

**Rule 2** comes free: the `__tc_Eq_UserId_eq` mangled function already flows through HM inference, and any body whose effects exceed its declared `with` clause already fires Flux's standard E400-series effect-mismatch diagnostic.

#### Landed when

- A class method may carry any of the seven `with` forms, parsed by reusing `parse_effect_list()`.
- An instance method may carry any of the seven `with` forms, parsed by the same helper.
- Rule 1 (class row ⊆ instance row) fires `E452` at instance-collection time when violated.
- Rule 2 (instance body row ⊆ instance declared row) fires the existing function-effect-mismatch diagnostic.
- Calling `eq(user1, user2)` resolves to `Eq<UserId>::eq` and propagates **that instance's** declared effect row into the caller's ambient row, via `try_resolve_class_call`.
- Built-in stdlib classes (`Eq`, `Ord`, `Show`, `Num`, `Semigroup`) continue to type-check unchanged because their methods carry no `with` clause (floor = `<>`, instances default to pure).
- Two instances of the same class in two different modules with **different effect signatures** are both legal and produce different caller-side effect rows when dispatched (e.g. `Logger<StdoutHandle>` with `with IO` and `Logger<NullHandle>` with no `with`).

#### Deferred

- **Head-level `instance Foo<X> with IO { ... }`** (per-instance pinning at the head, applying to every method) is a convenience deferred until users complain about repeating `with IO` on each method. The per-method form is already grammatical and fully expressive.
- **`sealed class` / explicit ceiling syntax** — see "no sealed class" note above. Add only if a real use case appears.

#### Phase 4a prerequisite — whitelist `effect` declarations in module bodies

The module-body validator at [src/bytecode/compiler/statement.rs:1685](src/bytecode/compiler/statement.rs#L1685) currently allows `Function`, `Let`, `Data`, `Class`, `Instance`, and `Import` inside `module { ... }` blocks but **does not** allow `Statement::EffectDecl`. This means `effect Console { print: String -> () }` declared inside a `module` block is rejected with `INVALID_MODULE_CONTENT`.

The four worked examples below all live in module form, and several of them put `effect` declarations inside their own modules (e.g. `module Flow.Console { effect Console { ... } }`). For those examples to parse, the module-body whitelist must learn one new arm:

```rust
Statement::EffectDecl { .. } => {}
```

This is a **1-line change** following the same pattern as Phase 1's whitelist extension for `Class`/`Instance`/`Import`. It ships as the **first commit** of Phase 4 (Phase 4a-prereq), independent of any parser or walker work.

#### Module-scoped effect names — known limitation, deferred

Effects in Flux today are a **global namespace**: `EffectExpr::Named { name: Identifier }` ([src/syntax/effect_expr.rs:12](src/syntax/effect_expr.rs#L12)) stores a single bare identifier with no module path. Two modules cannot both declare `effect Console` without colliding on the bare name `Console` — analogous to the pre-Phase-1 problem with classes that ClassId resolved.

This is **not a Phase 4 blocker**. The four examples use distinct effect names (`Console`, `Clock`, `AuditLog`, etc.) and reference them by bare name from any module, exactly as effects work today. The only thing that's *new* is that the `effect` declaration itself can live inside a `module { ... }` block (per the prereq above).

A full fix — module-scoped effect identity (`EffectId = (ModulePath, Identifier)`, mirroring `ClassId`) — is parked for a Phase 4d-or-later follow-up if anyone hits a real collision. Until then, the convention is "one effect per dedicated `module Flow.X` file", same as the stdlib pattern.

#### Files touched

- **[src/bytecode/compiler/statement.rs](src/bytecode/compiler/statement.rs)** — *Phase 4a-prereq*: 1-line whitelist for `Statement::EffectDecl` in the module-body validator.
- [src/syntax/type_class.rs](src/syntax/type_class.rs) — `ClassMethod.effects: Vec<EffectExpr>` and `InstanceMethod.effects: Vec<EffectExpr>`. Same field type as `Statement::Function.effects` already has.
- [src/syntax/parser/statement.rs](src/syntax/parser/statement.rs) — call `parse_effect_list()` after a class-method return type and after an instance-method parameter list. One-line additions in two places.
- [src/types/class_env.rs](src/types/class_env.rs) — `MethodSig.effects`, propagated from `ClassMethod`. New `enforce_class_method_effect_floor` walker fires `E452` when Rule 1 is violated.
- [src/types/class_dispatch.rs](src/types/class_dispatch.rs) — Phase 1b mangling pass now carries `instance_method.effects` into the synthesized top-level `Statement::Function.effects` instead of the current hardcoded `vec![]`. **One field assignment.**
- [src/ast/type_infer/expression/calls.rs](src/ast/type_infer/expression/calls.rs) — `try_resolve_class_call` looks up the *resolved instance's* mangled scheme in `cached_member_schemes`, extracts the `Fun(_, _, effect_row)`, and unifies that row into the caller's ambient row. Same code path as `infer_call` for regular `Fun(...)` callees.
- [src/types/module_interface.rs](src/types/module_interface.rs) — populate the `pinned_row_placeholder` field on `PublicClassEntry` / `PublicInstanceEntry` (added in Phase 2 specifically to absorb Phase 4 rows without a cache format bump).
- [src/diagnostics/](src/diagnostics/) — `E452`.

#### Complete Phase 4 surface area

Every piece, where it comes from, what's new vs. what already exists. This makes the "no new syntax" claim verifiable line by line.

| Piece | Source | Status |
|---|---|---|
| `module Foo.Bar { ... }` | Phase 1 | ✅ ships |
| `import Flow.X as Alias` (file scope) | predates Phase 1 | ✅ ships |
| `import Flow.X as Alias` (inside module body) | Phase 1 | ✅ ships |
| `public class` / `public instance` / `public data` | Phase 2 | ✅ ships |
| `class Foo<a> { fn m(x: a) -> T }` | Proposal 0145 | ✅ ships |
| `instance Foo<X> { fn m(x) { body } }` | Proposal 0145 | ✅ ships |
| `effect Foo { op: T -> U }` (top-level) | existing | ✅ ships |
| `effect Foo { op: T -> U }` (inside `module { ... }`) | **Phase 4a-prereq** | ⚠️ 1-line whitelist add |
| `with Effect` after a class method return type | **Phase 4a** | ⚠️ 1 `parse_effect_list()` call |
| `with Effect` between an instance method's params and body | **Phase 4a** | ⚠️ 1 `parse_effect_list()` call |
| `with \|e` row variable on a class method | reuses 4a parser hook | ✅ same code path |
| `with E1, E2` comma-separated on a class method | reuses 4a parser hook | ✅ same code path |
| `with E \| e` lower bound on a class method | reuses 4a parser hook | ✅ same code path |
| `perform Effect.op(args)` (inside instance body) | existing | ✅ no change |
| `expr handle Effect { op(resume, ...) -> ... }` | existing | ✅ no change |
| Qualified class method call `Module.method(args)` | Phase 1 | ✅ ships |
| Type-directed dispatch propagating effects to caller | **Phase 4b** | row from the resolved instance's mangled scheme flows through `try_resolve_class_call` |
| Row-polymorphic class methods end-to-end | **Phase 4c** | row variable in class signature instantiates per call site |

**Total Phase 4 code delta** (excluding tests): the 1-line prereq + 2 `parse_effect_list()` calls + 2 AST fields + 1 `MethodSig` field + 1 mangling-pass assignment + 1 walker (Rule 1 / E452) + the `try_resolve_class_call` row-propagation step. Estimate: **~60 lines of compiler code** spread across the three sub-phases.

#### Error codes introduced

`E452` only. The original proposal's `E453` (pinning a row on a `with <>` class) and `E454` (pinning a row that doesn't satisfy a `with <E, ..>` minimum) collapse into `E452` because under the floor model both are the same rule: instance row must be a superset of class row.

#### Tests required

Each of the four worked examples below becomes an integration test in [tests/module_scoped_classes_tests.rs](tests/module_scoped_classes_tests.rs), invoked through `compile_source` and asserted against expected diagnostics. Plus:

- `Eq<UserId>` with `fn eq(a, b) with AuditLog { perform AuditLog.record(...); ... }` — **valid**: class row `<>` ⊆ instance row `<AuditLog>`. Caller of `Eq.eq(u1, u2)` gains `AuditLog`.
- `Eq<UserId>` with `fn eq(a, b) { perform AuditLog.record(...); ... }` (no `with` clause) — rejected by Rule 2 (body has effects, declaration is pure). Standard function-effect-mismatch diagnostic.
- `Logger<h>` declared as `fn log(...) with IO | e`, instance with `fn log(...) with Time` only — rejected by Rule 1 (instance is missing `IO`). E452.
- `Foldable<f>` with `fn fold(..., step: (b, a) -> b with |e) -> b with |e`, instance `Foldable<List>` with the standard recursive body — caller passing a closure that performs `Console` has `Console` propagated to its ambient row via the row variable `e`.
- Built-in `Eq`, `Ord`, `Show`, `Num`, `Semigroup` continue to type-check (no `with` clauses anywhere in their declarations).

#### Risk

Low-to-medium. The original proposal estimated medium-high based on the assumption of new syntax; the Spike A finding that all seven effect forms are already GREEN cuts the risk substantially. Reading 2 (floor semantics) further reduces risk by avoiding the need for a post-inference walker — Rule 1 is a pure annotation-vs-annotation check, and Rule 2 falls out of the existing function-effect-mismatch machinery.

**Mitigation:** ship in four small commits.

- **Commit 1 — Phase 4a-prereq.** 1-line whitelist for `Statement::EffectDecl` in the module-body validator + 1 integration test. Independent of all the parser/walker work below.
- **Commit 2 — Phase 4a.** `ClassMethod.effects` + `InstanceMethod.effects` AST fields + 2 `parse_effect_list()` calls + Rule 1 walker (E452) + Phase 1b mangling-pass field assignment. No row propagation to callers yet — class-method calls still resolve through the existing path. The smallest commit that makes the seven `with` forms parse on classes.
- **Commit 3 — Phase 4b.** `try_resolve_class_call` propagates the resolved instance's row into the caller's ambient row. `Eq<UserId>` with `with AuditLog` callers now actually need `AuditLog` in their signature.
- **Commit 4 — Phase 4c.** `Foldable` with `with |e` end-to-end. The row variable in the class declaration instantiates freshly per call site and unifies with the closure's row.

Each commit is independently shippable.

#### Worked examples

All four examples use Flux's existing effect-system syntax (`effect` / `perform` / `handle` / `with` / `|e`) and the existing module/class/instance syntax from Phases 1–3. Every token in user-facing position is something that already parses today, with the sole exception of `with` after a class-method return type and `with` between an instance-method parameter list and its body — both of which are added by Phase 4a's two `parse_effect_list()` calls.

> **Note on parameter naming.** `handle` is a reserved keyword in Flux's effect handler syntax (`expr handle Effect { op(resume, args) -> ... }`) and cannot be used as a parameter name. The examples below use `hnd` for any parameter that would otherwise be named `handle` — discovered while writing the Phase 4a-prereq integration tests.

```rust
// ─── Example 1: Console effect + Logger class across four modules ────
//
// User-defined effect, class method that declares it, instance that
// performs it, caller that handles it. Same effect/perform/handle
// pattern as examples/type_system/22_handle_discharges_effect.flx,
// but with the effectful function moved into a class.

module Flow.Console {
    effect Console {
        print: String -> ()
    }
}

module Flow.Logger {
    import Flow.Console as Console

    public class Logger<h> {
        fn log(hnd: h, msg: String) -> Unit with Console
    }
}

module App.StdLog {
    import Flow.Logger as Logger
    import Flow.Console as Console

    public data StdoutHandle { Stdout }

    public instance Logger<StdoutHandle> {
        fn log(hnd, msg) {
            perform Console.print(msg)
        }
    }
}

module App.Main {
    import App.StdLog as StdLog
    import Flow.Logger as Logger
    import Flow.Console as Console

    public fn run() -> Int {
        Logger.log(StdLog.Stdout, "ok") handle Console {
            print(resume, _msg) -> resume(())
        }
        1
    }
}

fn main() with IO {
    print(to_string(App.Main.run()))
}
```

```rust
// ─── Example 2: Eq<UserId> with custom AuditLog effect ───────────────
//
// The class is pure (floor = empty). Eq<Int> stays pure. Eq<UserId>
// adds an audit-log effect by declaring it explicitly on the instance
// method and performing it in the body. Type-directed dispatch routes
// each caller to the right instance, and the effect propagates only
// when the audit instance is selected.

module Flow.Eq {
    public class Eq<a> {
        fn eq(x: a, y: a) -> Bool
    }

    public instance Eq<Int> {
        fn eq(x, y) { x == y }
    }
}

module Flow.AuditLog {
    effect AuditLog {
        record: String -> ()
    }
}

module App.Users {
    import Flow.Eq as Eq
    import Flow.AuditLog as AuditLog

    public data UserId { Id(Int) }

    public instance Eq<UserId> {
        fn eq(a, b) with AuditLog {
            perform AuditLog.record("comparing users")
            match (a, b) {
                (Id(x), Id(y)) -> x == y
            }
        }
    }
}

module App.Service {
    import Flow.Eq as Eq
    import Flow.AuditLog as AuditLog
    import App.Users as Users

    // Dispatches to Eq<Int> — pure because that instance has no `with`.
    public fn ints_equal(x: Int, y: Int) -> Bool {
        Eq.eq(x, y)
    }

    // Dispatches to Eq<UserId> — gains AuditLog from the resolved instance.
    public fn users_equal(a: Users.UserId, b: Users.UserId) -> Bool with AuditLog {
        Eq.eq(a, b)
    }
}

fn main() with IO {
    let result = App.Service.users_equal(App.Users.Id(1), App.Users.Id(2))
        handle AuditLog {
            record(resume, msg) -> {
                print(msg)
                resume(())
            }
        }
    print(to_string(result))
}
```

```rust
// ─── Example 3: Foldable with row-polymorphic step callback ──────────
//
// The class method's `step` callback may carry any row `|e`. The result
// inherits the same row. The instance body itself is pure — the row
// variable is bound at the call site, not at the instance.

module Flow.Foldable {
    public class Foldable<f> {
        fn fold<a, b>(
            xs: f<a>,
            init: b,
            step: (b, a) -> b with |e,
        ) -> b with |e
    }

    public instance Foldable<List> {
        fn fold(xs, init, step) {
            match xs {
                Nil        -> init,
                Cons(h, t) -> Foldable.fold(t, step(init, h), step),
            }
        }
    }
}

module Flow.Console {
    effect Console {
        print: String -> ()
    }
}

module App.Reports {
    import Flow.Foldable as Foldable
    import Flow.Console as Console

    // Effectful caller: |e instantiates to <Console> at this call site,
    // and the caller's signature must therefore include Console.
    public fn print_all(lines: List<String>) -> Int with Console {
        Foldable.fold(lines, 0, fn(count, line) {
            perform Console.print(line)
            count + 1
        })
    }

    // Same class, same instance, pure caller. The row variable
    // instantiates to <> at this call site.
    public fn count_lines(lines: List<String>) -> Int {
        Foldable.fold(lines, 0, fn(count, _) { count + 1 })
    }
}

fn main() with IO {
    let n = App.Reports.print_all(["a", "b", "c"]) handle Console {
        print(resume, msg) -> {
            print(msg)
            resume(())
        }
    }
    print(to_string(n))
}
```

```rust
// ─── Example 4: Multiple effects on one class method, handled separately
//
// Comma-separated effect list on a class method. Two `perform` calls in
// the instance body. Two stacked `handle` blocks at the call site
// discharging both effects.

module Flow.Console {
    effect Console {
        print: String -> ()
    }
}

module Flow.Clock {
    effect Clock {
        now: () -> Int
    }
}

module Flow.Tracer {
    import Flow.Console as Console
    import Flow.Clock as Clock

    public class Tracer<h> {
        fn trace(hnd: h, msg: String) -> Unit with Console, Clock
    }
}

module App.SimpleTracer {
    import Flow.Tracer as Tracer
    import Flow.Console as Console
    import Flow.Clock as Clock

    public data Handle { H }

    public instance Tracer<Handle> {
        fn trace(hnd, msg) {
            let t = perform Clock.now()
            perform Console.print("[" ++ to_string(t) ++ "] " ++ msg)
        }
    }
}

module App.Main {
    import App.SimpleTracer as SimpleTracer
    import Flow.Tracer as Tracer
    import Flow.Console as Console
    import Flow.Clock as Clock

    public fn run() with IO {
        Tracer.trace(SimpleTracer.H, "starting up")
            handle Clock {
                now(resume) -> resume(0)
            }
            handle Console {
                print(resume, msg) -> {
                    print(msg)
                    resume(())
                }
            }
    }
}

fn main() with IO {
    App.Main.run()
}
```

**The novel Flux feature**, illustrated by Examples 2 and 3: two instances of the same class with **different effect signatures**, both selected by type-directed dispatch, with the per-instance row flowing into the caller's ambient row. Koka has no classes; Haskell would need to encode it via fundeps or higher-rank types (per Spike C). Flux gets it for free because effect rows are already first-class and the resolved instance's mangled scheme already carries the correct row — Phase 4 just teaches the parser to accept the seven existing `with` forms in two new positions and teaches `try_resolve_class_call` to read the resolved instance's row into the caller's ambient row.

---

### Phase 5 — Aether invariants

**Goal.** Ensure module-scoped classes do not regress Perceus dup/drop optimization, and add proper Aether test coverage for user-defined classes.

**Landed when.**
- Invariant A verified: private class methods still produce `__tc_*` defs in `CoreProgram.defs` and are inferred by the borrow registry with non-`Unknown` modes.
- Invariant B verified: `__dict_*` globals are tagged `Borrowed` on read; no spurious `dup/drop` on dictionary accesses.
- Invariant C implemented: per-class `meet_mode` precomputed from all instances and used on indirect `TupleField` calls.
- New Aether snapshot tests exist for user-defined classes (filling the gap that today only covers built-ins).
- No regression in the Aether benchmark group (`cargo bench --bench aether`).

**Files touched.**
- [src/aether/borrow_infer.rs](src/aether/borrow_infer.rs) — `meet_mode` precomputation, `ClassId`-aware registry lookup.
- [src/core/passes/dict_elaborate.rs](src/core/passes/dict_elaborate.rs) — tag generated `__dict_*` defs with borrow hint.
- [src/types/class_env.rs](src/types/class_env.rs) — expose `instances_for(ClassId)` to the borrow inference pass.
- [tests/aether_cli_snapshots.rs](tests/aether_cli_snapshots.rs) — new snapshots for user-defined classes.
- [examples/aether/](examples/aether/) — new `class_borrow_calls.flx` fixture.

**Error codes introduced.** None.

**Tests required.**
- All tests from the main test plan's §Aether / Perceus interaction section.
- Regression: existing `aether__aether__borrow_calls__dump_core.snap` updated only for the mangled-name prefix change.
- Benchmark: cross-run comparison of the Aether suite to confirm no regressions.

**Risk.** Medium. The `meet_mode` computation is the only genuinely new Aether machinery. Mitigation: land Invariants A and B first as pure bookkeeping changes (they're hygiene improvements even without classes), then add C.

**Not in this phase.** `fip` / `fbip` annotations (future work), any change to reuse insertion or drop specialization.

---

### Phase 6 — Stdlib migration and soft deprecation

**Goal.** Move the standard library onto the new form, validate the whole proposal against real code, and warn users on legacy top-level `class` / `instance`.

**Landed when.**
- `Flow.Eq`, `Flow.Ord`, `Flow.Num`, `Flow.Show`, `Flow.Semigroup`, `Flow.Foldable`, `Flow.Functor` are rewritten inside `module Flow.X { ... }` blocks with `public class` and `public instance` for every existing built-in.
- All stdlib tests pass under the migrated form.
- All `examples/` programs that used built-in classes still run on both backends.
- A deprecation warning fires on any top-level `class` / `instance` in user code.
- Parity check (`scripts/check_parity.sh`) passes on the full `examples/` tree.
- Benchmark sweep shows no perf regression (`scripts/bench/bench.sh all`).

**Files touched.**
- [lib/Flow/](lib/Flow/) — every `.flx` / `.flxi` pair for the listed modules.
- [src/runtime/base/](src/runtime/base/) — built-in class registrations updated to the new `ClassId` form.
- [src/main.rs](src/main.rs) `inject_flow_prelude()` — confirm the prelude injection still works with module-scoped stdlib.
- [src/syntax/parser/statement.rs](src/syntax/parser/statement.rs) — emit deprecation warning for legacy top-level `class` / `instance`.

**Error codes introduced.** None (warning only).

**Tests required.**
- Full stdlib test suite (`cargo test`).
- Full examples parity check on VM and LLVM backends.
- Full benchmark sweep with comparison against pre-phase-6 baseline.
- A dedicated fixture that triggers the deprecation warning and snapshots the diagnostic text.

**Risk.** Medium. The stdlib migration is mechanical but extensive. Mitigation: migrate one module at a time, with CI green after each.

---

### Phase 7 — Hard deprecation of top-level `class` / `instance`

**Goal.** Remove legacy top-level support; module-body form is the only form.

**Landed when.** One release after phase 6 lands. Top-level `class` / `instance` outside a `module` block is rejected at parse time with a helpful "wrap this in a `module ... { ... }` block" diagnostic.

**Files touched.**
- [src/syntax/parser/statement.rs](src/syntax/parser/statement.rs) — reject the legacy form.
- [src/ast/statement.rs](src/ast/statement.rs) — remove the top-level branches.
- Any remaining top-level `class` / `instance` in `lib/`, `examples/`, or `tests/` — migrate.

**Error codes introduced.** None (reuses existing E-codes with an updated message).

**Tests required.** Regression suite stays green.

**Risk.** Low. Mostly a deletion; the deprecation warning in phase 6 gives users a full release to migrate.

---

### Summary of landable checkpoints

| Phase | Status | Ships | Users can... |
|-------|--------|-------|--------------|
| 0 | ✅ done | Preflight spike findings | — (internal de-risking only) |
| 1 | ✅ done | Foundation + ClassId-keyed storage | write module-scoped classes, call via `Alias.method(...)`, have two classes with the same short name in different modules |
| 2 | ✅ done | Coherence + ADTs + `.flxi` | rely on orphan rule, incremental cache stays sound, `deriving` works under new rules |
| 3 | ✅ done* | `exposing` | opt into unqualified calls, use inside-module shadowing in default methods |
| 4 | 🟡 in progress | Effects | write effectful instance methods, pin instances to concrete rows |
| 5 | ⏳ pending | Aether | no perf regression on polymorphic dispatch |
| 6 | ⏳ pending | Stdlib migration + warning | use a fully-migrated stdlib; see deprecation warnings on legacy code |
| 7 | ⏳ pending | Hard deprecation | — (removal only) |

\* Phase 3 is "done" except for the deferred legacy-global suppression bullet (see Phase 3 §"Deferred to a follow-up"), held back until Phase 6 stdlib migration so the two changes ship together.

🟡 Phase 4 is in progress: the pre-phase spike and Phase 4a-prereq (the 1-line whitelist for `effect` declarations in module bodies) have landed. The next sub-commit is Phase 4a (parser hooks + Rule 1 walker). See the Phase 4 §"Status" block above for the full sub-commit breakdown.

Each row is a release-shippable checkpoint. If scheduling pressure forces a cut, the minimum viable slice for the proposal is **phases 1 + 2 + 6**: those three alone deliver module-scoped classes with coherence guarantees and a migrated stdlib. Phases 3, 4, 5, 7 are all genuine improvements but are not load-bearing for the core story.

---

## Test Plan

The test plan is organized by compiler subsystem and phase, matching the layout of [tests/](tests/). Every new diagnostic (`E447`–`E454`) must have at least one positive test (diagnostic fires) and one negative test (diagnostic does not fire on a look-alike).

### Parser / validation

Location: [tests/parser_tests.rs](tests/parser_tests.rs), snapshot fixtures under [tests/fixtures/](tests/fixtures/).

- Module body accepts `public class`, `class`, `public instance`, `instance`.
- Module body accepts a mix of `public class`, `public instance`, `public fn`, `public type`, and private variants in any order.
- Top-level `class` / `instance` still parses and emits the deprecation warning.
- Invalid non-member statements in module bodies still rejected.
- `class Foo { ... }` without a type parameter, when method signatures mention a free type variable, produces a parse/infer-time error (inherited rule, regression test).
- `public class Foo exposing (...)` is rejected (partial class export is not supported).

### Class identity

- Two modules each define a class called `Foldable`; both compile and mangled names do not collide.
- Ambiguous short-name constraint without qualification emits `E448`.

### Orphan rule

- `instance` in the class's module → accepted.
- `instance` in the head type's module → accepted.
- `instance` in a third module → rejected with `E447`.
- Legacy top-level instance checked against the implicit file module.

### Visibility

- Private class's methods not callable as `M.method` from another module.
- `public instance` of a private class rejected with `E450`.
- Private instance not in the resolution set from another module (E444 when unavoidable).

### Resolver

- Qualified `Foldable.fold([1,2,3], ...)` resolves to `__tc_Flow_Foldable_Foldable_List_fold`.
- `exposing (fold)` lets unqualified `fold([1,2,3], ...)` resolve via the class path.
- Collision between exposed `fold` and a local top-level `fold` emits `E449`.
- Inside `Flow.Foldable`, bare `fold` in a default method body resolves to the local class method.

### Interface files

- `.flxi` round-trips a `public class` and a `public instance`.
- Private classes / instances not present in `.flxi`.
- Downstream `.fxc` cache invalidates when a directly-imported module adds, removes, or modifies a `public class` or `public instance`.

### Effects on instance methods

Location: new file [tests/class_effects_tests.rs](tests/class_effects_tests.rs), fixtures under [tests/fixtures/class_effects/](tests/fixtures/class_effects/).

Positive:

- Row-polymorphic class method: `Foldable.fold` with a pure callback infers `with <>`; with an IO callback infers `with <IO>`; snapshot the inferred type.
- `with <>` class (`Eq`, `Ord`, `Show`): pure instance body accepted.
- `with _` class (`Foldable`): IO instance body accepted and the pinned row propagates to callers.
- Instance with `instance Logger<StdoutHandle> with <IO> { ... }` — the pinned row shows up in `.flxi` and in call-site effect rows.
- `with <IO, ..>` class: instance that performs `<IO, Net>` is accepted.
- Row propagation across constraint: `fn f<a: Foldable, e>(xs: a<Int>) -> Int with e` type-checks end-to-end and lowers correctly.

Negative:

- `E452` — pure class, effectful instance body. Fixture: `instance Eq<MyType> { fn eq(x, y) { print("called"); ... } }`.
- `E453` — pinning a row on a `with <>` class. Fixture: `instance Eq<MyType> with <IO> { ... }`.
- `E454` — instance pinned row does not satisfy class minimum. Fixture: class `with <IO, ..>`, instance `with <>`.

### Aether / Perceus interaction

Location: new file [tests/class_aether_tests.rs](tests/class_aether_tests.rs), snapshots under [tests/snapshots/aether/](tests/snapshots/aether/).

Invariant A — visibility does not hide mangled methods from the borrow registry:

- Fixture with `public class C<a> { fn m(x: a) -> a }` and `public instance C<Int> { ... }`: snapshot the borrow registry, assert `__tc_*_C_Int_m` is present with non-`Unknown` modes.
- Fixture with `class C<a> { fn m(x: a) -> a }` (private class, private instance): snapshot the registry, assert the mangled method is still present with inferred modes. Regression for Invariant A.

Invariant B — dictionary globals are borrowed:

- Fixture that forces polymorphic dispatch (`fn apply<a: Foldable>(xs: a<Int>) -> Int { Foldable.fold(xs, 0, ...) }`), snapshot the dup/drop insertion output, assert no `dup(__dict_*)` / `drop(__dict_*)` appears.

Invariant C — polymorphic meet:

- Two instances of the same class, one with `[borrowed, borrowed]`, one with `[owned, borrowed]`. Snapshot the computed `meet_mode` and assert it is `[owned, borrowed]`. Assert the polymorphic call site uses the meet; the two monomorphic call sites each use their own tight modes.

Snapshot-only regression:

- User-defined `class Sum<a> { fn add(x: a, y: a) -> a }` with `instance Sum<Int>` and `instance Sum<Float>`, exercised inside [examples/aether/](examples/aether/) alongside `borrow_calls.flx`. Add `class_borrow_calls.flx` and assert the Aether dump matches the snapshot. This fills the existing gap identified in the Aether investigation (current tests only cover built-in classes).

### Symbol encoding / mangling

Location: new module [src/types/mangle.rs](src/types/mangle.rs) with inline `#[cfg(test)] mod tests { ... }`.

Covers the ten normative test cases from §Symbol Encoding, plus:

- **Cross-language parity spot check:** feed a handful of module/class/type/method combinations through the encoder and assert the output matches a frozen golden file. The golden file is the authoritative ABI record; updating it requires a changelog fragment flagged as an ABI break.
- **Proptest round-trip:** generate arbitrary segment strings matching Flux's identifier grammar and assert `decode(encode(s)) == s`.
- **Proptest distinctness:** generate arbitrary *pairs* of distinct source triples and assert their mangled forms are also distinct. This is the collision-freeness property made executable.
- **Show_Show regression fixture:** explicit test reproducing the worked-example table in §Symbol Encoding, asserting that `Flow.Show.Show<Secret>.show` and `Flow.Show_Show<Secret>.show` produce distinct mangled names.

### Unit tests for new ClassEnv data structures

Location: [src/types/class_env.rs](src/types/class_env.rs) `#[cfg(test)] mod tests { ... }`.

- `ClassId` equality and hashing: same short name in two modules → distinct keys, distinct entries.
- `ClassEnv::collect_from_statements` on a program with a private class: `ClassDef.visibility == Private`, class is not in the exported member set.
- `ClassEnv::lookup_by_class_id` returns the correct `ClassDef` regardless of which module the caller is in.
- Orphan rule unit test: build a minimal program with three modules (`C` defines class, `T` defines type, `X` defines an instance of `C<T>`). Expect `E447` at the instance in `X`. Move the instance to `C` or `T`; expect no error.
- Short-name ambiguity unit test: two modules in scope each export `Foldable`; a constraint `<a: Foldable>` without qualification fires `E448`.
- `meet_mode` unit test: construct a synthetic `ClassEnv` with two instances of different borrow signatures, assert `meet_mode` matches the mathematical lattice join.

### Interface file round-trip

Location: new file [tests/class_flxi_tests.rs](tests/class_flxi_tests.rs).

- Write a module with a `public class`, a `public instance`, a private class, and a private instance. Serialize to `.flxi`. Parse the `.flxi`. Assert:
  - Public class present with correct `ClassId`, superclasses, method sigs, default-method flags, and row annotations.
  - Public instance present with `ClassId`, head types, context, method names, and pinned row (if any).
  - Private class **not** present.
  - Private instance **not** present.
- Cache invalidation test: compile module A that imports module B, snapshot the `.fxc` hash of A. Add a `public instance` to B. Recompile. Assert A's `.fxc` hash changed.
- Cache stability test: same as above but add a *private* instance to B. Assert A's `.fxc` hash is unchanged (private additions are invisible).

### Mixed module body integration

Location: new fixture [examples/classes/mixed_module_body.flx](examples/classes/mixed_module_body.flx), driven from [tests/examples_snapshot_tests.rs](tests/examples_snapshot_tests.rs).

- Compile the canonical mixed example from §6a (class + instances + types + functions + private helpers in one module). Assert:
  - The program compiles, runs, and produces expected output on both VM and LLVM backends (parity check).
  - The public function `sum_foldable` is reachable; the private helper `__smoke_test` is not in the `.flxi`.
  - Unqualified `fold` inside the class's default method body resolves to the class's own method, not any legacy global.

### Parity across backends

- Add the mixed-module example and the effect-row examples to [scripts/check_parity.sh](scripts/check_parity.sh) input directories so both VM and LLVM backends are validated in CI.

### Regression

- Existing global `fold`, `length`, `to_list` behavior unchanged.
- Existing `Functor<List>` 0150 tests remain green.
- Existing `tests/snapshots/aether/aether__aether__borrow_calls__dump_core.snap` updated only for the mangled-name prefix change (`__tc_Semigroup_*` → `__tc_Flow_Semigroup_Semigroup_*`), and the update reviewed line-by-line.
- No regressions in `test_runner_cli` and examples snapshots (other than intentional mangled-name snapshot updates).

---

## Error Code Allocation

Reserved in this proposal (extends 0145's E440–E446):

- `E447` — orphan instance violation.
- `E448` — ambiguous short-name class constraint; qualification required.
- `E449` — `exposing` collision with a local top-level identifier at the import site.
- `E450` — `public instance` of a private class, or other class-visibility contradiction.
- `E451` — `public class` method signature mentions a private type of the same module.
- `E452` — instance body performs an effect not permitted by the class method's declared row.
- `E453` — instance pins a row on a pure (`with <>`) class.
- `E454` — instance's pinned row does not satisfy the class's minimum row.
- `E455` — `public instance` of a public class for a private type (would leak the type through the instance).
- `E456` — file-level and module-body imports both bind the same short name to different targets.

---

## Resolved Questions (previously open)

- **Does `public class` export all methods automatically?** Yes. Classes are atomic; partial exports are not supported.
- **Do top-level `class` / `instance` remain supported?** Yes, with a deprecation window: warning on next release, hard error one release after.
- **Do module-qualified class methods participate in `exposing (...)` imports?** Yes, with `E449` on collision.
- **Are instances visible transitively?** No. Direct import only. The orphan rule guarantees this is enough.
- **How is class identity represented?** `ClassId = (ModulePath, Identifier)`.
- **Does the mangled-name ABI change?** Yes. Mangled names and dictionary globals gain the module path prefix. Caches invalidate on upgrade.

## Open Questions

- Should `exposing (..)` (star-exposing) ever be allowed? This proposal says no; revisit only if ergonomics demand it.
- Should `deriving` clauses on ADT declarations be supported in this proposal or deferred to a separate one? Deferred — `deriving` interacts with the orphan rule and deserves its own design pass.
- Should re-exports (`export module Flow.Foldable` inside another module) be introduced alongside this proposal? Deferred — preserving class identity under re-export needs a careful spec and is not blocking.

## Recommendation

Implement the module-scoped, Rust-orphan-rule model:

- Classes live at module scope with explicit `public` / private visibility.
- Class identity is `(module_path, class_name)`.
- Instances obey the orphan rule and are visible only by direct import.
- Methods are accessed through the module namespace, with optional `exposing (...)` escape hatch.
- Instance selection remains type-directed.
- Mangled names and `.flxi` carry the module path; `.fxc` caches invalidate on upgrade.

That gives Flux a coherent path for `Flow.Foldable`, `Flow.Functor`, and future HKT classes while keeping the incremental cache sound and sidestepping the orphan-instance debt GHC still pays.
