# What's New in Flux v0.0.7

Flux v0.0.7 is an **abstraction** release.

v0.0.6 was about using Flux — a language server, a REPL, a real async runtime. This one is about writing code that doesn't repeat itself. Type classes go from a working prototype to a complete story with associated types, superclasses and `deriving`. A package manager arrives, so code can live outside your own checkout. And an unannotated function that takes a parameter is finally generic, the way Hindley–Milner always said it should be.

## Highlights

- **Type classes grow a hierarchy** (proposal 0179) — associated types, superclass evidence, checked `deriving`, and the standard classes (`Eq`, `Ord`, `Semigroup`, `Monoid`, `Functor`, `Applicative`, `Monad`) moved into the prelude
- **Flume, the package manager** (proposal 0177) — `flux.toml`, `flux.lock`, path / registry / git dependencies, a package store, workspaces, and `publish`
- **Generics without signatures** — `fn identity(x) { x }` now works at every type it's used at, with no annotation
- **Five new standard-library modules** — `Flow.Process`, `Flow.Env`, `Flow.Crypto`, `Flow.Result` and `Flow.Path` — plus a larger, async-aware `Flow.Fs`
- **Windows is tested** — CI now runs lint and tests on Windows alongside Linux and macOS

## Type classes

The single largest theme. v0.0.5 shipped type classes that worked; this release makes them a system you can build a hierarchy in.

**Associated types.** A class can declare a type that each instance chooses. Declarations are parsed and collected, equations are validated against the class that declares them, reduction keeps a stuck application stuck rather than guessing at it, and the resolved types travel across the module interface — so an associated type means the same thing in the module that defines it and the module that imports it.

**Superclasses.** A dictionary now carries its superclass evidence in leading slots, superclass obligations are checked against the whole program instead of one module at a time, and superclass identities survive a `.flxi` round-trip. `Ord` declares `Eq` as its superclass in the prelude, so a function constrained by `Ord` can call `eq`.

**`deriving` is checked before it runs.** A clause no body can be generated for is now rejected at compile time rather than failing at run time, derived instance evidence is pinned rather than re-derived, and a parameterized derived instance gets a structural `Eq` body.

**The standard hierarchy lives in the prelude**, written in Flux rather than special-cased in the compiler: `Eq`, `Ord`, `Semigroup` with its container instances, `Monoid`, `Functor`, `Applicative`, `Monad`. `Eq` over built-in containers has a real dictionary now, and `+` has its own class, so `String` can instantiate it like any other type.

**Dictionaries can be chosen by result type.** When a call's arguments don't determine which instance is meant, the type the result is required to have does. That reaches the common case for a multi-parameter class without functional dependencies.

**Classes are identified by their owning module**, so a class you declare locally wins over a same-named prelude class in a bound.

New diagnostics, each replacing something that used to fail later or silently: `E454` (overlapping instances), `E442` (an instance omitting a required method), `E449` / `E455` (head-type rules), `E487`, and a report for a class parameter no call site determines. A stale class interface is now reported rather than quietly dropping the class.

## Flume — the package manager

Flux code can now depend on Flux code it doesn't own (proposal 0177).

- `flux init` / `flux new` to start a package, described by **`flux.toml`**
- `flux add` / `flux remove` / `flux update` to manage dependencies, pinned in **`flux.lock`**
- **Path, registry and git dependencies** — git fetches from GitHub and GitLab, pinned by lockfile
- `flux build` / `flux run` / `flux test` / `flux check` across the whole dependency graph
- `flux tree` to see what you actually depend on
- A **package store**, **workspaces**, package metadata, and `flux publish`
- **`dev` and `release` build profiles**

The resolver is a pure core with effectful interpreters wrapped around it, so a dependency graph is planned as a value before anything touches the filesystem — which is what makes `flux tree` and the lockfile agree with what a build will do.

## Generics

An unannotated definition that takes parameters and raises no class constraint is now generic:

```flux
fn identity(x) { x }

let a = identity(42)        // Int
let b = identity("hello")   // String
```

That used to require `fn identity<a>(x: a) -> a`. The annotation is no longer what makes a function generic — taking a parameter is, which is the Hindley–Milner rule and what GHC does.

Alongside it:

- **Tuple projection is a solver predicate**, not a tuple shape guessed at the access site. An out-of-range index or an undetermined receiver is now reported (`E491`, `E492`) instead of silently widened.
- **Field access raises a constraint** instead of a hole.
- **Binding groups are inferred in dependency order**, not source order, so mutually recursive definitions type the same way regardless of how they're arranged in the file.
- **Specialisation** — a generalized definition whose call sites all agree on one concrete type is lowered under that substitution, so generalizing a function doesn't cost the representation Aether needs to prove in-place reuse.
- `FLUX_DBG_EVIDENCE` dumps the solver's evidence per call site.
- `examples/generics/` — 48 snapshot-pinned programs organised by what the compiler actually does with each one.

**What is not in this release:** a *constrained* definition still needs a signature. `fn double(x) { x + x }` raises `Num` and stays monomorphic without one. That is 0.0.8's work, and it needs the evidence translation underneath it.

## Standard library

- **`Flow.Process`** — run subprocesses
- **`Flow.Env`** — process arguments and environment variables
- **`Flow.Crypto`** — `sha256` and `sha256_file`
- **`Flow.Result`**
- **`Flow.Path`**
- **`Flow.Fs`** — gains `list_dir` and `metadata`, and is now async-aware

## Other changes

- **Windows** — POSIX assumptions removed from the Flume boundary, the C runtime, and the tests; CI now runs lint and tests on `windows-latest` alongside Linux and macOS.
- **The stdlib is found outside the source checkout**, so an installed `flux` works away from the repo. Project root and module roots are computed without consulting the working directory.
- **Cached artifacts are keyed on the compiler build**, not its version, so a stale entry stops being *found* rather than being read and mistaken for current.
- **A deep stack trace elides its middle** instead of printing every frame.
- **The parity harness gained `// requires:`** — a fixture making a claim about the host OS skips off-platform instead of reporting a mismatch that looks like a compiler defect.

## Migration notes

- **The cache epoch runs 43 → 53.** Inferred types changed, so `.flxi` contents changed. Cached artifacts from 0.0.6 are invalidated automatically; nothing to do by hand.
- **The standard classes moved into the prelude.** If you declared your own `Eq`, `Ord`, `Semigroup`, `Monoid`, `Functor`, `Applicative` or `Monad`, a locally declared class still wins in a bound — but check that you meant to shadow it.
- **`deriving` now rejects at compile time** what it used to fail on at run time. A clause that was quietly producing no usable body is now an error.
- **An instance that omits a required method is `E442`.** This was previously accepted and trapped later.
- **Generalization is more aggressive.** A function you did not annotate may now be inferred at a more general type than before. Where that surfaces an ambiguity, the fix is usually the signature you would have written anyway.

## Recommended first things to try

```flux
// Generic without a signature
fn swap(p) { (p.1, p.0) }

// A class with a superclass, from the prelude
fn largest<a: Ord>(xs: List<a>) -> Option<a> { ... }
```

```sh
flux new my_package
cd my_package
flux add some_dep
flux tree
flux build
```

And set `FLUX_DBG_EVIDENCE=1` on a program that uses classes to watch the solver pick dictionaries.

## In short

v0.0.7 is the release where Flux code stops being written once per type and stops living in one directory. Type classes are complete enough to build a hierarchy in, generics no longer require you to say what the compiler can work out, and packages let code travel.

The next release closes the gap these three leave between them: a *constrained* function that you didn't annotate.
