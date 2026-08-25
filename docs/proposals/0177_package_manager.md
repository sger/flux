- Feature Name: Flux package manager (`flux.toml`, `flux.lock`, `flux build/test/add`)
- Start Date: 2026-08-20
- Status: Partially implemented (Phases 0 and 1 shipped 2026-08-24; Phase 2 resolution and lockfile shipped 2026-08-24; git dependencies, their lockfile entries, `tree`, `add`, `remove`, and the `--offline` / `--locked` / `--frozen` flags shipped 2026-08-25; registry fetching deferred until there is a registry to fetch from; Phase 3 not started)
- Proposal PR:
- Flux Issue:
- Supersedes: [0015_package_module_workflow_mvp.md](0015_package_module_workflow_mvp.md) (the MVP sketch; this proposal subsumes and completes it)
- Requires (for a Flux implementation): [0178_os_capabilities_for_tooling.md](implemented/0178_os_capabilities_for_tooling.md)
- Builds on: the module graph ([../../src/syntax/module_graph/module_resolution.rs](../../src/syntax/module_graph/module_resolution.rs)), module interfaces ([../../src/types/module_interface.rs](../../src/types/module_interface.rs)), the cache layout ([../../src/shared/cache_paths.rs](../../src/shared/cache_paths.rs)), and the CLI driver split ([0154](implemented/0154_cli_driver_split.md))
- Relates to: [0163_flux_language_server.md](0163_flux_language_server.md), [0175_interactive_repl.md](0175_interactive_repl.md), [0011_phase2_module_system_enhancements.md](0011_phase2_module_system_enhancements.md)

# Proposal 0177: Flux Package Manager

## Summary
[summary]: #summary

Give Flux a first-class package manager: a `flux.toml` manifest, a resolved
`flux.lock` lockfile, a content-addressed package store, and a `flux build` /
`flux test` / `flux add` command surface. The design rests on four commitments:
a declarative manifest, a real resolved lockfile, immutable fetched sources, and
fingerprint-based incremental rebuild — combined with a content-addressed store
keyed on the full build-input closure. That last piece matters for Flux
specifically, because Flux already emits per-module interface files (`.flxi`)
carrying interface fingerprints.

The package manager is **written in Flux**. The work is staged across four phases,
specified in the [Reference-level explanation](#reference-level-explanation):

| Phase | Scope | Unblocks | Status |
|---|---|---|---|
| **0** | Manifest parser, version arithmetic, resolver, `Flow.Path` — pure Flux, no I/O | The interesting logic, testable immediately | Shipped |
| **1** | `flux.toml`, `init/build/run/test`, path deps, namespacing, stdlib-as-package | Flux works outside its own checkout | Shipped 2026-08-24 |
| **2** | Registry index, semver resolution, `flux.lock`, `add/update/tree` | Third-party libraries | Resolution + lockfile shipped 2026-08-24; git dependencies and `tree` shipped 2026-08-25; registry fetching deferred, `add`/`remove`/`update` not started |
| **3** | Content-addressed store, `publish`, workspaces, `metadata` | An ecosystem | Not started |

Phase 0 has no dependency on [0178](implemented/0178_os_capabilities_for_tooling.md) and can
begin immediately. Phase 1 is independently valuable and should land on its own.

### What Phases 0 and 1 shipped

Phase 0 lives in `lib/Flume/`: `Toml`/`Parse`/`Value`/`Document` (the TOML
parser), `Manifest` (the schema and `derive_namespace`), `Version` (version
arithmetic), and `Resolve` (the backtracking resolver), alongside
`lib/Flow/Path.flx`.

Phase 1 rests on two Flux modules and a thin Rust layer, keeping every
packaging decision in Flux:

- `lib/Flume/Roots.flx` reads `flux.toml`, walks path dependencies
  transitively, derives each package's namespace, and emits one record per
  resolved package. Registry and dev dependencies are rejected here with a
  "not supported until Phase 2" message rather than being silently ignored.
- `lib/Flume/Cli.flx` owns the manifest template, the scaffolding layout, and
  entry-point selection (`[[bin]]`, `[lib]`, then the conventional layout).
- `src/driver/manifest_roots.rs` runs the resolver and turns its records into
  scoped module roots; `src/cli/package.rs` forwards the commands. Neither
  parses TOML.

Three diagnostics were added: `E469` (two packages claim one namespace),
`E470` (a manifest exists but does not resolve), and `E471` (a package declares
a module outside its own namespace). `find_project_root` now keys on
`flux.toml`, and `collect_roots` computes roots from the entry file and its
project root rather than the process working directory.

The resolver's output is cached against the content of every manifest that
produced it, so a warm package build does not re-spawn it. (The `--filter <s>`
slot in the CLI surface is covered by the existing `--test-filter <s>`, which
matches the qualified test name.)

### What Phase 2 shipped

Registry dependencies resolve, lock, and build. What a project declares in
`[dependencies]` as a semver requirement is now settled by the Phase 0
resolver against a local registry index, recorded in `flux.lock`, and turned
into a scoped module root — the same kind of root a path dependency produces.

Four Flux modules, all of them either pure or confined to I/O:

- `lib/Flume/Lock.flx` reads and writes `flux.lock`: the format-version
  preserve-what-you-find protocol, inline checksums, deterministic ordering,
  and the rule that a registry entry without a checksum is rejected rather
  than trusted. Pure.
- `lib/Flume/Index.flx` reads the append-only index — one file per package,
  one JSON object per line — into resolver candidates, tagging every
  requirement with the package that declared it. Pure.
- `lib/Flume/Plan.flx` joins manifest, index, resolver, and lockfile, and
  decides whether the resolution changed. Pure.
- `lib/Flume/Home.flx` is the only module here that touches the disk:
  `$FLUX_HOME` layout, index reads, and an atomic lockfile write through a
  temporary file and a rename.

`lib/Flume/Roots.flx` no longer rejects registry dependencies. It walks path
dependencies as before, collects registry requirements, resolves them once
against the whole graph, writes the lockfile when it changed, and emits a root
per resolved package. Dev dependencies are excluded from the build graph
rather than rejected.

**Minimal-change updating** is implemented as two resolution attempts, not as
an ordering: first against a candidate set narrowed to what the lockfile
already chose, then — only if that fails — against the full set. Ordering does
not work, because the resolver sorts candidates highest-version-first
internally and discards any order it is handed. A locked version therefore
survives a newer publication, while an edited manifest re-resolves instead of
failing.

**Git dependencies shipped 2026-08-25**, and are how a third-party package is
obtained today. A dependency may name a repository with `git`, pinned by `rev`,
`tag`, or `branch`; checkouts live in `$FLUX_HOME/git/checkouts/<slug>/<commit>`
keyed by resolved commit, and the commit is recorded in `flux.lock` as
`source = "git+<url>#<commit>"`, so a locked build is reproducible and needs no
network. `flux tree` renders the resolved graph.

Fetching drives the `git` binary rather than `Flow.Http`, which speaks no TLS
and therefore cannot reach any HTTPS host
([KI-035](../known_issues.md#ki-035)). That restriction is what defers the
registry client rather than the registry itself: a hosted index is not worth
standing up before the language has users, and git repositories on GitHub or
GitLab need no hosting at all.

**Not yet shipped from this phase:** registry network fetching (the index and
unpacked sources are read from `$FLUX_HOME`, never downloaded), `add` /
`update`, and index-state pinning.

`flux add` and `flux remove` shipped 2026-08-25. The format-preserving
requirement is met by editing the manifest's source lines directly
(`Flume.Edit`) rather than round-tripping through the Phase 0 parser, which
discards comments — exactly the "separate editing path" this phase anticipated.

`--offline`, `--locked`, and `--frozen` shipped 2026-08-25 for git
dependencies. The registry path still relies on `Flume.Plan.unsatisfied`, which
implements the same check for semver requirements but is not yet called.

One defect was found and fixed in the process: the Phase 1 roots cache
fingerprinted manifests only, so once resolution depended on `flux.lock` the
cache replayed stale resolutions
([KI-024](../known_issues.md#ki-024)). `CACHE_EPOCH` is now 21.

Three language limitations were hit and documented rather than worked around
silently: [KI-011](../known_issues.md#ki-011) (extended — the defect is
invisible under `flux run` and appears only under `--test`),
[KI-022](../known_issues.md#ki-022), and
[KI-023](../known_issues.md#ki-023).

## Motivation
[motivation]: #motivation

### Flux currently cannot be used outside its own source checkout

This is not a missing convenience; it is a hard blocker, and it is the single
strongest argument for doing this work now. Three concrete defects, each verified
against the current tree:

1. **The stdlib is found by a CWD-relative path.** `inject_flow_prelude`
   ([../../src/driver/frontend.rs:92](../../src/driver/frontend.rs#L92)) does
   `Path::new("lib").join("Flow")` and silently returns if it does not exist.
   Running a program that imports `Flow.List` from any directory other than the
   compiler checkout fails:

   ```
   $ cd /tmp/demo && flux run uselist.flx
   error[E018]: Import Not Found
   Cannot find module `Flow.List` to import.
     Looked for module `Flow.List` under roots:  (imported from uselist.flx).
   ```

   Note the empty root list. An installed `flux` binary has no way to find its own
   standard library.

2. **Module roots are CWD-relative too.** `collect_roots`
   ([../../src/driver/frontend.rs:146](../../src/driver/frontend.rs#L146)) pushes
   `Path::new("src")` and `Path::new("lib")` — relative to the *process working
   directory*, not to the entry file or project root. `flux run foo/bar.flx` and
   `cd foo && flux run bar.flx` therefore see different module roots.

3. **Project root detection keys on `Cargo.toml`.** `find_project_root`
   ([../../src/shared/cache_paths.rs:74](../../src/shared/cache_paths.rs#L74))
   walks up looking for `Cargo.toml` to place `target/flux`. In a Flux user's
   project there is no `Cargo.toml`, so every build silently falls back to a
   `.flux/cache` directory beside the entry file.

Each of these is individually small. Together they mean Flux has no notion of "a
project," which is exactly the notion a package manager must introduce.

### Dependencies cannot coexist today

Verified experimentally: two module roots each containing a `Json.flx` that
declares `module Json` is a hard error.

```
$ flux run main.flx --root ./a --root ./b
error[E027]: Duplicate Module
Duplicate module declaration: `Json`.
  Found: /tmp/a/Json.flx, /tmp/b/Json.flx
```

Since dependencies are supplied as module roots, **any two dependencies that
happen to ship a module with the same name are mutually incompatible.** In a flat
module namespace, `Json`, `Utils`, and `Config` will collide almost immediately.
A package manager that cannot install two unrelated libraries is not a package
manager.

The good news — also verified — is that the fix requires **no language change**.
Namespaced modules already work:

```flux
// deps/Acme/Json.flx
module Acme.Json { public fn tag() -> String { "from-acme" } }
// deps/Widget/Json.flx
module Widget.Json { public fn tag() -> String { "from-widget" } }
```
```flux
import Acme.Json as AJ
import Widget.Json as WJ
fn main() -> Unit { println(AJ.tag()); println(WJ.tag()) }
```
```
[ 8 of 14] Compiling  Acme.Json
[ 9 of 14] Compiling  Widget.Json
"from-acme"
"from-widget"
```

So the design below makes **each package own a module namespace segment**, which
turns the collision problem into a naming convention the manifest enforces.

### The compiler already has the hard parts

Flux is unusually well positioned. `ModuleInterface`
([../../src/types/module_interface.rs:102](../../src/types/module_interface.rs#L102))
already records `source_hash`, `compiler_version`, `semantic_config_hash`,
`interface_fingerprint`, and `dependency_fingerprints`. That is exactly the pair
of hashes a build system needs — an *input* hash over the build inputs and an
*output* hash over the emitted interface — and it means **interface-preserving
changes to a dependency need not rebuild dependents**, the highest-value
incremental-build win, already available. The package manager should consume
this, not reinvent it.

### Roadmap position

[`roadmap_to_1_0_0.md`](../roadmaps/roadmap_to_1_0_0.md) lists "a coherent
standard library and package/module workflow" as a 1.0 requirement, and
[`roadmap_v0.0.6.md`](../roadmaps/roadmap_v0.0.6.md) M5 scopes a package MVP.
The existing [0015](0015_package_module_workflow_mvp.md) is a normalization-damaged
stub (duplicated headings, elided body text, no resolver/lock/store design); it is
not implementable as written. This proposal replaces it.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Starting a project

```
$ flux init hello
     Created binary package `hello`
$ tree hello
hello
├── flux.toml
├── src
│   └── main.flx
└── tests
    └── basic.flx
```

```toml
# flux.toml
[package]
name = "hello"
version = "0.1.0"
edition = "2026"

[dependencies]
```

`flux build` compiles, `flux run` runs, `flux test` runs every `test_*` function
across the package.

### Adding a dependency

```
$ flux add json --version 1.2
      Adding json v1.2.0 to dependencies
    Updating flux.lock
```

```toml
[dependencies]
json = "1.2"
shared = { path = "../shared" }
parser = { git = "https://github.com/org/parser", tag = "v2.0" }
```

### The namespace rule (the one new concept)

**A package named `json` owns the module namespace `Json`.** Every module it
exports is `Json.something`, stored at `Json/something.flx` under the package's
`src/`. The manifest may override the namespace when the package name does not
capitalize cleanly:

```toml
[package]
name = "http-client"
namespace = "HttpClient"   # default derived: http-client -> HttpClient
```

This is what makes two dependencies coexist. It is checked at build time: if a
package declares a module outside its namespace, that is an error at *its* build,
not a mysterious `E027` at some downstream consumer's build.

```
error[E0xx]: module escapes package namespace
Package `json` may only declare modules under `Json`, but `src/Utils.flx`
declares `module Utils`.
  |
  = help: rename to `module Json.Utils` and move the file to `src/Json/Utils.flx`
```

The root module `Json` itself (at `src/Json.flx`) is the package's public face, so
`import Json` works as users expect.

### Reproducibility

`flux.lock` is committed. It records the exact resolved graph with content hashes.
`flux build` uses it verbatim when it is consistent with `flux.toml`; `--locked`
makes any change to the lock an error (for CI); `--offline` forbids network access.

### What users should carry away

Three ideas. A **package** is a unit of versioning and distribution that owns a
module namespace. The **lockfile** is the reproducible record of a resolution, not
a cache. The **store** is immutable and content-addressed, so builds never
destroy each other's artifacts.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Cross-cutting design decisions

These four choices constrain every phase and are settled here so the phase
proposals do not relitigate them.

**One version per package name.** A package manager may permit semver-incompatible
duplicates only if the compiler gives each instance a distinct symbol namespace.
Flux cannot: the VM linker keys globals by flat string name —
`VmAssemblyContext::global_map_for`
([../../src/compiler/module_linker.rs:73](../../src/compiler/module_linker.rs#L73))
looks up `binding.name` in a `HashMap<String, usize>` and, on a `Defined`
collision, **silently reuses the existing global**. Two versions of one package
would not error; they would silently alias each other's functions. The resolver
therefore forbids duplicates outright, with a real diagnostic. The activation key
is written as `(name, SemverCompatibility)` from the start so that relaxing this —
should per-package symbol mangling ever land — is a policy change rather than a
rewrite.

**Version requirements are conjunctions of intervals.** Caret, tilde, comparison,
and `,`-separated conjunction. A full boolean lattice including `||` union is
excluded: it buys little and costs both normalization and solver complexity.

**No features / conditional compilation.** Optional, additively-unified
compilation flags are the single most commonly regretted part of existing package
managers: once features from different dependency kinds and excluded target
platforms unify into one set, enabling a feature in one place silently changes
what is compiled elsewhere, and retrofitting the fix requires a second resolver
pass that never fully recovers the intended semantics. If conditional compilation
is needed later, it must be designed against a resolver that already exists, and
designed knowing that **the lockfile must remain feature-independent** — otherwise
resolution has to run twice.

**Constraint provenance is recorded.** Every constraint is labeled with its origin
— manifest, lockfile, CLI `--constraint`, stdlib pin — because "why is this version
pinned?" must be answerable. Provenance is cheap to record at construction and
effectively impossible to reconstruct afterwards.

### Phase 0 — Core libraries in Flux

Build the package manager's pure logic — path manipulation, manifest parsing,
version and range arithmetic, and the dependency resolver — as Flux libraries
with no I/O, tested against in-memory fixtures. This phase writes no files, opens
no sockets, and adds no primops. It depends on nothing in
[0178](implemented/0178_os_capabilities_for_tooling.md).

It is worth doing first for three reasons. It is **unblocked** — recursive ADTs,
exhaustive matching, generics, HAMT maps, and the string primops all work today,
and a resolver's core traversal was written and run to confirm it. It is **pure,
so it is testable** without temporary directories or network. And it is the **real
test of the language**: examples exercise a happy path, whereas a version-range
comparator and a backtracking resolver exercise ergonomics.

Every function in this phase should have an **empty effect row**. A manifest parser
that provably cannot touch the filesystem is a property most ecosystems cannot
assert about their own build tooling; if any signature here needs an effect, that
is a design smell worth investigating.

#### `Flow.Path`

Path manipulation is string manipulation; it needs no OS access. This module is
specified in [0178 Item 2](implemented/0178_os_capabilities_for_tooling.md) and is listed
there as stage 0 precisely because it belongs to this phase.

```flux
Path.join(a, b)      Path.parent(p)     Path.file_name(p)
Path.extension(p)    Path.normalize(p)  Path.is_absolute(p)
```

It ships here rather than in 0178 because Phase 0 is its first consumer and
because it is the natural warm-up exercise: small, pure, and immediately useful
elsewhere.

#### The `Flume` namespace

The package manager lives under a single `Flume` namespace, shipped at
`lib/Flume/` alongside the `Flow` standard library. The name shares the Latin
root of *flux* and *flow* (*fluere*, to flow); a flume is an engineered channel
that carries material to where it is needed, which is what a package manager
does.

Namespacing is a correctness requirement, not a preference. Module names are
flat: two roots that each contain a bare `Version.flx` collide with
`error[E027] Duplicate Module`. A namespaced `Flume.Version` coexists with a
user's own `Version` module, so the package manager must not squat on bare
names — and using the namespace itself dogfoods the mechanism Phase 1 hands to
every third-party package.

Two mechanical constraints apply, both verified: a dotted module requires the
brace-block form (`module Flume.Version { ... }`; the bare header form raises
E034), and every segment must start uppercase, which is what forces the
package-name-to-namespace derivation rule discussed under Phase 1.

#### `Flume.Version`

```flux
data Version { Version { major: Int, minor: Int, patch: Int, pre: Option<String> } }

data Range {
  Exact(Version)         // = 1.2.0
  | Caret(Version)       // ^1.2 — compatible-with
  | Tilde(Version)       // ~1.2 — patch-level
  | GreaterEq(Version)
  | LessThan(Version)
  | And(Range, Range)    // comma-separated conjunction
}

Version.parse(s: String) -> Result<Version, String>
Version.compare(a: Version, b: Version) -> Ordering
Range.parse(s: String) -> Result<Range, String>
Range.matches(r: Range, v: Version) -> Bool
```

Ranges are **conjunctions of intervals only** — no `||` union, per the
cross-cutting decisions above: union ranges buy little and cost both normalization
and solver complexity.

`SemverCompatibility` — "same leftmost nonzero digit," so `Major(1)`, `Minor(0.2)`,
`Patch(0.0.3)` — is defined here, because the resolver's activation key depends on
it and because encoding it as a value type is what lets the single-instance rule
be a data-structure invariant rather than a scattered check.

**Pre-release ordering is the subtle part** and deserves explicit tests: `1.0.0-alpha`
precedes `1.0.0`, and pre-release versions are excluded from ranges unless the
range itself names a pre-release.

#### `Flume.Manifest`

A three-stage funnel — raw TOML → schema types → normalization → domain model —
with the schema types kept separate so they remain a stable serialization
contract.

```flux
Manifest.parse(text: String) -> Result<Manifest, ManifestError>
```

This phase includes **a TOML subset parser written in Flux**. Flux has no TOML
dependency today (verified: no `toml` crate in `Cargo.lock`), and the codebase
hand-rolls even `hex`. Hand-writing a subset is viable for a small schema but will
not survive Phase 3's richer manifests; that tension resolves cleanly here, since
the Phase 0 parser targets the manifest schema only — tables, string/integer/boolean
values, arrays, and inline tables — and is a genuinely good exercise in Flux
(recursive descent over a string, ADT output, error positions).

It must reject what it does not support with a clear error rather than silently
mis-parsing. If the schema later outgrows the subset, replacing this parser is a
contained change behind `Manifest.parse`.

`ManifestError` carries a position so diagnostics can point at the offending line.

#### `Flume.Resolve`

The resolver specified in Phase 2's Resolution section, minus all I/O. It is a pure
function from a dependency graph and a set of candidate packages to either a
resolution or a conflict explanation:

```flux
Resolve.resolve(roots: List<Dep>, available: List<Package>) -> Result<Resolution, Conflict>
```

Phase 0 implements the **full backtracking resolver**, not a placeholder:
highest-version-first, most-constrained-goal-first, with a conflict cache and the
one-version-per-package activation key. Doing it now, while it is pure, is far
easier than retrofitting it around a registry client in Phase 2.

`Conflict` must carry a **minimized conflict set** with constraint provenance —
which package required what, and from where — since constraint provenance is a
cross-cutting requirement (see above).

The candidate set is a parameter, not a fetch. Phase 2 supplies it from a
registry; Phase 0 supplies it from fixtures. This is the seam that keeps the
resolver testable forever.

#### Testing (Phase 0)

This phase should carry the densest test suite in the package manager, because
everything is pure and nothing needs a fixture directory.

- **Version/range property tests**: parse-then-render round-trips; `compare` is a
  total order; `Caret` matching agrees with a hand-written oracle on a large
  generated version set; pre-release ordering against the semver spec's examples.
- **Manifest parser**: snapshot tests for valid manifests; explicit error tests
  for unsupported TOML constructs, duplicate keys, and missing required fields.
- **Resolver property tests against generated graphs** — the reason a
  backtracking resolver is trustworthy at all. Generate random
  dependency graphs, and assert that any returned resolution satisfies every
  constraint and contains exactly one version per package name.
- **Resolver conflict tests**: unsatisfiable diamonds produce a minimized conflict
  set naming the actual culprits, not the whole graph.
- **Purity check**: every public function in this phase compiles with an empty
  effect row. This is a real assertion in Flux, and it is the phase's headline
  property.

### Phase 1 — Projects and path dependencies

Introduce the notion of a *project*: a manifest, a standard layout,
package-namespaced modules, path dependencies, and the
`flux init` / `build` / `run` / `test` commands. This is the phase that makes Flux
usable outside its own source checkout. It ships no registry, no lockfile, and no
version solving — every dependency is a local path.

**This phase is independently valuable and should land on its own**, even if no
later phase is ever built: fixing stdlib discovery alone converts Flux from a
language that runs inside its own repository into one that can be installed.

Requires [0178](implemented/0178_os_capabilities_for_tooling.md) stages 1–4 (recoverable I/O,
`Flow.Fs`, `Flow.Env`).

#### Manifest (Phase 1 subset)

Phase 1 accepts a deliberately small schema, parsed by `Flume.Manifest` from
[Phase 0](#phase-0-core-libraries-in-flux):

```toml
[package]
name = "json"
version = "1.2.0"
edition = "2026"
namespace = "Json"          # optional; derived from name

[dependencies]
shared = { path = "../shared" }

[lib]
path = "src/Json.flx"

[[bin]]
name = "json-fmt"
path = "src/bin/fmt.flx"
```

Registry dependencies (`json = "1.2"`) and `[dev-dependencies]` parse but are
rejected with a "not supported until Phase 2" diagnostic rather than being
silently ignored.

#### Module namespacing and resolution

The namespace rule is enforced in the module graph. `resolve_import_path`
([../../src/syntax/module_graph/module_resolution.rs:165](../../src/syntax/module_graph/module_resolution.rs#L165))
currently searches a flat list of roots and errors on multiple matches. It gains a
package-aware layer:

- Each resolved package contributes exactly one root, and a package's root may
  only satisfy imports whose first segment is that package's namespace.
- A collision between two packages' namespaces is caught at *resolution* time
  ("packages `a` and `b` both claim namespace `Json`"), with a clear message,
  rather than surfacing as `E027 Duplicate Module` at parse time.
- `--root` remains, unscoped, as an escape hatch for scripts and tests.

`collect_roots` and `find_project_root` are fixed as part of this work: roots are
computed relative to the **manifest directory**, and `find_project_root` looks for
`flux.toml` (falling back to `Cargo.toml` only for the compiler's own test corpus).

Phase 1 enforces the namespace rule at build time even though only path
dependencies exist, because the rule is what makes Phase 2 possible and because
retrofitting it after packages exist would be a breaking change.

#### Stdlib as an implicit package

`Flow` becomes a package the toolchain always provides, resolved from a
toolchain-relative path rather than CWD-relative `lib/Flow`. Discovery order:
`FLUX_STDLIB` env override → path relative to the running executable
(`../lib/Flow`, `../share/flux/lib/Flow`) → the ancestor walk the LSP already
implements in `find_flow_dir`
([../../src/lsp_support.rs:342](../../src/lsp_support.rs#L342)) for
development checkouts. This single change is what makes an installed `flux`
binary work at all, and it is worth landing first even independently of the rest.

This single change is the highest-value item in the phase and could ship before
everything else.

#### Project root and cache location

`find_project_root` currently walks up looking for `Cargo.toml`
([../../src/shared/cache_paths.rs:74](../../src/shared/cache_paths.rs#L74)). It
must look for `flux.toml`, falling back to `Cargo.toml` only for the compiler's
own test corpus. Build artifacts land in `<project-root>/target/flux/`.

`collect_roots` ([../../src/driver/frontend.rs:146](../../src/driver/frontend.rs#L146))
must compute roots relative to the **manifest directory**, not the process working
directory, so that `flux run foo/bar.flx` and `cd foo && flux run bar.flx` behave
identically.

#### The Rust/Flux boundary

This is the first phase where the package manager must call into the compiler, so
the Rust/Flux boundary becomes concrete:

| Component | Language |
|---|---|
| Manifest loading, dependency graph, namespace validation, CLI | Flux |
| Supplying resolved module roots to the module graph | Rust |
| `find_project_root` / `collect_roots` fixes | Rust |
| Stdlib discovery | Rust |

The Flux side decides *what* to build and hands the compiler a resolved set of
roots. The Rust side is deliberately thin and does not grow in later phases.

#### CLI surface (Phase 1)

```
flux init [name] [--lib]      flux new <name>
flux build                    flux run [--bin <name>] [-- args]
flux test [--filter <s>]      flux check
flux clean
```

Existing invocations — `flux run <file.flx>`, `--root`, `flux fmt`, `flux repl` —
keep working unchanged on loose files with no manifest. That script mode is what
makes a language pleasant to try, and it must not regress.

#### Testing (Phase 1)

- **The regression that fails today**: a program importing `Flow.List` runs from a
  directory outside the compiler checkout.
- **The collision regression**: two path dependencies each shipping a `Json`
  module build and run together (currently `E027`).
- Namespace enforcement: a package declaring a module outside its namespace errors
  at *its own* build, with a message naming the expected path.
- `flux init` → `build` → `test` end-to-end on a two-package path-dependency
  scaffold, executed outside the compiler checkout.
- Root computation: `flux run foo/bar.flx` and `cd foo && flux run bar.flx` resolve
  identical module roots.
- Manifest-less script mode still works.

### Phase 2 — Registry, lockfile, and resolution

Add third-party dependencies: an append-only registry index, semver requirements
resolved by the Phase 0 resolver, a committed `flux.lock` recording the exact
resolved graph with content hashes, and `flux add` / `update` / `tree`.

Two properties matter more than the feature itself. **Reproducibility**: a
checked-in lockfile plus content hashes means a build today and a build in a year
resolve identically, and tampering is detected rather than executed.
**Immutability**: published versions never change — the axiom the entire download
cache rests on, which must be enforced by the index from the first published
package because it cannot be introduced later.

Requires [0178](implemented/0178_os_capabilities_for_tooling.md) stage 3 (`Flow.Crypto`), and
stage 5 (`Flow.Process`) for git dependencies.

#### Resolution

**The central policy decision: one version per package name, for now.**

A package manager may permit semver-incompatible duplicates of one package only
if the compiler gives each instance a distinct symbol namespace. Flux cannot do
this today. The VM linker keys globals by flat
string name — `VmAssemblyContext::global_map_for`
([../../src/compiler/module_linker.rs:73](../../src/compiler/module_linker.rs#L73))
looks up `binding.name` in a `HashMap<String, usize>` and, on a `Defined`
collision, **silently reuses the existing global**. Two versions of one package in
one program would therefore not error — they would silently alias each other's
functions. That is a correctness hazard, so the resolver forbids it outright with a
real diagnostic.

This is a *single-instance restriction*, and it is what makes resolution genuinely
hard: with duplicates allowed, a conflict is usually resolvable by admitting a
second copy, whereas under a single-instance rule the solver must backtrack.
Ecosystems that adopted this rule needed conflict-directed backjumping, weighted
heuristics, and backjump budgets to stay tractable. Accordingly:

- **Phase 1** has no solver: path dependencies only, resolved by graph walk.
  Duplicate package names are an error.
- **Phase 2** ships a backtracking resolver with a conflict cache, highest-version-first,
  most-constrained-goal-first. Version requirements are restricted to **conjunctions
  of intervals** (caret, tilde, comparison, `,`-separated). A full boolean lattice
  over ranges including `||` union is deliberately excluded: it buys little and
  costs both normalization and solver complexity.
- Every constraint is labeled with its **provenance** (manifest / lockfile / CLI
  `--constraint` / stdlib pin), because "why is this version pinned?" must be
  answerable. Provenance is cheap to record at constraint-construction time and
  effectively impossible to reconstruct afterwards.
- A resolution failure renders the minimized conflict set, not a raw dump.

If Flux later adds per-package symbol mangling, the restriction can be lifted;
the resolver's activation key is written as `(name, SemverCompatibility)` from the
start so that relaxation is a policy change rather than a rewrite.

**Features are deliberately omitted from Phase 1–3.** Optional, additively-unified
compilation flags are the single most commonly regretted part of existing package
managers: once features from different dependency kinds and excluded target
platforms are unified into one set, enabling a feature in one place silently
changes what is compiled somewhere else, and retrofitting the fix requires a
second resolver pass that still cannot fully decouple test-only dependencies
without multiplying build times. If conditional compilation is needed later, it
should be designed against a resolver that already exists, and designed knowing
that **the lockfile must remain feature-independent** — otherwise resolution has
to run twice, once to produce a flag-independent lock and once to build.

#### Lockfile

`flux.lock` is a real resolved graph with content hashes, not a file of pinning
constraints. The constraint-file alternative — record exact version and flag
constraints, then re-solve on every build — is reproducible only if the index is
append-only, and it still requires running the solver on every build, so the
resulting plan depends on the solver's own version. A resolved graph avoids both
problems: it is read, not recomputed.

```toml
version = 1

[[package]]
name = "json"
version = "1.2.0"
source = "registry+https://packages.flux-lang.org"
checksum = "sha256:9f2a…"
dependencies = ["core-utils"]

[[package]]
name = "shared"
version = "0.1.0"
# path packages carry no `source`: no portable representation
```

Design constraints, each of which is far cheaper to honor now than to retrofit:

- **Format versioning uses the preserve-what-you-find protocol.** Add support for
  a new version; do not change the default; preserve the on-disk version; flip the
  default only after the new version has been supported for several releases.
  Without this, a project's lockfile oscillates between toolchain versions and
  every commit churns.
- **Serialize to minimize merge conflicts** — inline checksums into the package
  entry, keep dependency lists compressed, sort deterministically.
- **Path packages have no `source`**, so path package names must be unique within
  a build.
- **No generic `[metadata]` string→string escape hatch.** An untyped catch-all
  table becomes a de-facto compatibility surface that can never be removed.

Minimal-change updating follows `register_previous_locks`: locked versions are
*preferred*, not hard-pinned, and editing a manifest unlocks that source's
packages. Conservatism comes from ordering preference, not from hard locks.

#### Sources, fetching, and the store

`$FLUX_HOME` (default `~/.flux`):

```
~/.flux/
  registry/index/<registry-hash>/      # cached index
  registry/cache/<registry-hash>/      # downloaded .flxpkg archives
  registry/src/<registry-hash>/        # unpacked sources
  git/{db,checkouts}/
  store/<compiler-abi>/<unit-hash>/    # Phase 3: built artifacts
```

The registry index is an **append-only, timestamped log**, one file per package,
one JSON line per version, sharded by name prefix. Append-only-ness is what makes
"published versions are immutable" true, which is what makes the entire download
cache sound. It also enables an `index-state = "<timestamp>"` pin, which is cheap
to implement given an append-only log and a useful complement to the lockfile: the
lock reconstructs one resolution, while an index-state reconstructs the entire
universe of packages that was visible at a moment in time.

Integrity is SHA-256 over the package archive, recorded in the lock. Fetches are
atomic: unpack to a temporary directory, then rename, with a sentinel file marking
completion. Concurrent toolchain processes coordinate with an advisory lock over
`$FLUX_HOME`.

`--offline`, `--locked`, and `--frozen` (= both) are global flags.

**Supply chain.** Because this is a new registry, two mitigations that are painful
to retrofit should be designed in now: a publish-age preference axis (so a
freshly-uploaded malicious version is not selected instantly by default), and
signed index metadata. Neither needs to ship in Phase 2, but the index format
must leave room for both.

Phase 2 uses `$FLUX_HOME` for the index, download cache, and unpacked sources. The
`store/` directory in that layout is Phase 3's; Phase 2 builds dependencies into
`target/flux/` alongside local packages.

#### Git dependencies

Git dependencies require subprocess execution
([0178](implemented/0178_os_capabilities_for_tooling.md) stage 5) to shell out to `git`. They
pin to a resolved commit hash in the lockfile, and — like path dependencies —
carry no registry `source`.

If stage 5 is not available when this phase lands, git dependencies can be
deferred without blocking the rest of Phase 2.

#### CLI surface (Phase 2)

```
flux add <dep> [--version|--path|--git]     flux remove <dep>
flux update [-p <pkg>]                      flux tree
```

plus the global `--offline`, `--locked`, and `--frozen` flags, shared by every
command through one common argument builder.

`flux add` must edit `flux.toml` **format-preservingly** — comments and field
order survive — which is a stronger requirement than round-tripping through the
Phase 0 parser and may need a separate editing path.

#### Testing (Phase 2)

- Resolver integration against a mock registry, reusing the Phase 0 property
  tests with a fetching candidate source substituted for fixtures.
- Lockfile round-trip; **minimal-diff tests** asserting that adding one dependency
  changes only the expected lines (merge-conflict behavior should be tested, not
  assumed).
- `--locked` fails when the manifest and lock disagree; `--offline` fails cleanly
  with no network.
- Checksum mismatch is detected and reported as a security error, not a parse
  error.
- Index-state pinning reconstructs an identical resolution.
- Conservative update: `flux update -p json` moves `json` and leaves siblings
  pinned.
- Concurrent `flux build` invocations sharing `$FLUX_HOME` do not corrupt the
  cache.

### Phase 3 — Store, publishing, and workspaces

Complete the package manager: a content-addressed store keyed on the full
build-input closure, `flux publish`, multi-package workspaces, machine-readable
plan output, and index-state pinning.

Phase 2 builds every dependency into the project's `target/flux/`, so the same
library at the same version is rebuilt once per project — and there is no way to
publish one. Three gaps close here: **shared immutable artifacts** (one built
dependency reused across every project, with concurrent builds safe because store
entries are never mutated), **publishing** (without which the Phase 2 registry has
nothing in it), and **workspaces** (how libraries are actually developed, needing
one lockfile and one resolution).

This is also where Flux's existing strengths pay off: `.flxi` interfaces already
carry `interface_fingerprint` and `dependency_fingerprints`, so an
interface-preserving change to a dependency need not rebuild its dependents.

#### Build orchestration and the store

The unit graph lowers the *package* graph to a *target* graph — a package has a
lib, bins, and tests, and the same module may need building more than once (with
and without test harness). Units are keyed by
`(package, target, mode, compiler-config)`.

Flux already fingerprints at module granularity, so the package manager's job is
to (a) supply the correct roots and configuration hash and (b) decide staleness at
package granularity, delegating to the existing `.flxi` machinery within a package.
Two rules matter enough to state as hard invariants, because violating either
produces cache bugs that are extremely hard to diagnose:

- **Never hash an mtime value.** Docker layers zero the nanosecond field of every
  mtime; hashing mtimes causes spurious full rebuilds. Track paths, compare
  dynamically.
- **Never hash an absolute path.** Renaming a project directory must not
  invalidate the cache. Hash relative paths and inject the root at runtime.

Dirtiness must be **explainable**: a `flux build --explain-rebuild` that reports
why each unit was considered stale, backed by a stored reason, is cheap to add
alongside the fingerprint and repays itself immediately.

Phase 3 introduces the content-addressed store, keyed by a hash over the full
build-input closure: package identity, source hash, the set of dependency unit
hashes (making the key recursive), compiler version **and ABI tag**, and every
flag that can change codegen or the emitted interface — `--strict`, `--optimize`,
and the existing `semantic_config_hash` inputs. Under-hashing configuration
silently yields artifacts that are incompatible with the configuration they get
reused under, and the symptom appears far from the cause — so the key should start
over-inclusive and be narrowed only with evidence.

Crucially, the store is **two-tier**:

- **Local/workspace packages build "inplace"** into `target/flux/` — not hashed,
  not stored, because they are edited constantly.
- **Immutable dependencies build into the store** and are shared across projects.

Store entries are immutable, so reads need no locking; writes go to an incoming
directory and are atomically renamed, and a process that loses the race **must
abandon its build and adopt the winner's artifact** (builds are not guaranteed
bit-reproducible).

The build queue is a topological work queue with a parallelism budget and
failure propagation, reusing the existing `rayon` dependency.

#### Machine-readable output

`flux metadata --format json` emits the workspace, resolved graph, and target
layout; `flux build --plan json` emits the unit plan. Both are versioned. The
Mature ecosystems converge on a stable machine-readable plan as the integration
point for editors and external tooling, and the Flux LSP
([0163](0163_flux_language_server.md)) is the immediate in-tree consumer — it
currently reimplements stdlib discovery itself.

#### Workspaces

A workspace has one lockfile and one resolution. Member packages are path
dependencies on one another implicitly, and all members share the resolved
dependency set — so two members cannot end up on different versions of the same
library, which would violate the one-version rule at link time anyway.

Inheritance keeps duplication down: a member may inherit `version`, `license`, and
common dependencies from the workspace root. Resolution of inherited fields
happens during manifest normalization ([Phase 0](#phase-0-core-libraries-in-flux)),
so the domain model a build sees is always fully resolved.

#### Publishing

`flux publish` packages the source into an archive, verifies it builds from a
clean checkout (a package that only builds in its author's working tree is the
classic publishing failure), computes the content hash, and uploads.

Rules that must hold from the first published package:

- **Published versions are immutable.** No re-uploads, no edits.
- **Yanking hides a version from new resolutions** but never removes it, so
  existing lockfiles keep working.
- The archive must contain everything needed to build, and packaging must not rely
  on the author's uncommitted files.

#### Index-state pinning

```toml
[registry]
index-state = "2026-08-21T00:00:00Z"
```

Because the index is append-only, a timestamp reconstructs the exact universe of
packages visible at that moment. This complements the lockfile: the lock pins one
resolution, index-state pins everything that *could* have been resolved.

#### Testing (Phase 3)

- Store key sensitivity: changing `--strict`, `--optimize`, or the compiler
  version produces a different key; changing an unrelated file does not.
- **Race safety**: two concurrent builds of the same dependency produce one store
  entry, and the loser adopts the winner's artifact.
- Interface-preserving change to a dependency does not rebuild dependents
  (exercises `dependency_fingerprints`).
- Cache-key hygiene: renaming the project directory does not invalidate the cache;
  a `touch` with no content change does not rebuild.
- Workspace resolution: all members share one lock; a member cannot pin a
  different version of a shared dependency.
- Publish verification rejects a package that builds only in a dirty working tree.
- Index-state pinning reconstructs a historical resolution exactly.

### Implementation language: building the package manager in Flux

**The package manager is built in Flux.** It is an attractive self-hosting target —
real, useful software that exercises parsing, data modeling, I/O, and networking
without requiring a compiler backend, and building it is how the project learns
what Flux can express.

The question of whether Flux is *ready* was investigated empirically against the
current toolchain. **Verdict: the resolver and manifest layers can be written
today; four OS capabilities are missing, and every one is a library/primop
addition rather than a type-system or language-semantics change.** Those four are
specified separately as [0178](implemented/0178_os_capabilities_for_tooling.md).

#### What already works (verified by running Flux programs)

| Capability | Status | Evidence |
|---|---|---|
| Manifest parsing | ✅ | `split`, `trim`, `substring`, `upper`, `replace` are user-callable; a `key = value` parser works today |
| Dependency graph modeling | ✅ | `data Dep { Dep(String, String) }`, `List<T>`, recursive walks with `[h \| t]` patterns all compile and run |
| Resolver-shaped recursion | ✅ | A recursive `collect(pkgs) -> List<String>` over a package graph runs correctly |
| Keyed lookup | ✅ | `Flow.Map` is a full HAMT (`get`/`set`/`delete`/`merge`/`keys`/`size`) |
| Reading and writing files | ✅ | `read_file` / `write_file` work |
| Registry downloads over HTTP | ✅ | `Flow.Http.get("http://example.com")` returned status `200` from a live request |
| JSON handling | ✅ | `Flow.Json` exists, and `deriving (Encode, Decode)` synthesizes codecs |
| Concurrency for parallel fetch | ✅ | `Flow.Async` / `Flow.Task` provide structured concurrency |

That is a substantial fraction of a package manager, and notably the *hard*
semantic parts — recursive graph modeling and the resolver's control flow — are
already expressible.

#### What is missing

Four gaps, in descending order of severity. Each is a **missing capability, not a
missing language feature**:

1. **Cryptographic hashing (blocking).** There is no SHA-256 anywhere in the
   stdlib or the primop table. Checksums, lockfile integrity, and the
   content-addressed store key all depend on it. Needs a `Flow.Crypto` module
   backed by new primops; the `sha2` crate is already a compiler dependency, so
   the runtime side is small.

2. **Directory and path operations (blocking).** The entire filesystem surface is
   `ReadFile`, `WriteFile`, `ReadLines`, `ReadStdin`. There is no way to list a
   directory, create one, delete a file, test existence, or manipulate paths — so
   a package manager cannot scaffold a project, walk `src/`, or populate a store.
   Needs `Flow.Fs` and `Flow.Path`.

3. **Recoverable I/O errors (blocking).** `read_file` on a missing path aborts the
   program with `E1009` rather than returning a value the program can inspect;
   the primop is classified only as `FileSystem`
   ([../../src/syntax/builtin_effects.rs:142](../../src/syntax/builtin_effects.rs#L142)),
   with no failure effect. "Does `flux.toml` exist here?" is therefore
   unanswerable in Flux today. The fix fits the existing effect system cleanly —
   classify fallible I/O under `Exn` and return `Result`-shaped values.

4. **Process environment and arguments (blocking for the CLI).** No `argv`, no
   environment variables, no process spawning. A CLI cannot read its own flags,
   find `$FLUX_HOME`, or shell out to `git` for git dependencies.

#### Assessment

The absence of any *type-system* gap is the notable result. Flux's ADTs,
exhaustive pattern matching, generics, HAMT maps, effect tracking, and structured
concurrency are already adequate to express a resolver — arguably better suited
than most languages, since effect signatures would let the manifest parser be
provably pure and the fetcher's I/O be visible in its type.

What is missing is entirely **OS surface area**, which is exactly what one would
expect from a language whose I/O story so far has been driven by async
networking (proposal 0174) rather than by tooling.

#### Plan

The four gaps are specified as [0178](implemented/0178_os_capabilities_for_tooling.md), which
this proposal depends on for Phases 1–3. Their cost is now measured
rather than estimated: a throwaway spike added a working `file_exists` primop
end-to-end in about nine edit sites, and the compiler's exhaustiveness checking
located every one of them (`error[E0004]: non-exhaustive patterns`). Extending
the OS surface is mechanical and type-safe; the real cost is settling the error
model once and maintaining native-backend parity.

The package manager is therefore built **in Flux**, staged against 0178:

| Layer | Language | Depends on |
|---|---|---|
| `Flow.Path` | Flux | nothing — writable today |
| Manifest parser | Flux | nothing — string primops suffice |
| Version/range parsing and comparison | Flux | nothing |
| Resolver | Flux | nothing — verified expressible |
| Lockfile read/write | Flux | 0178 stage 1–2 (recoverable I/O, `Flow.Fs`) |
| Checksums, store keys | Flux | 0178 stage 3 (`Flow.Crypto`) |
| CLI entry point | Flux | 0178 stage 4 (`Flow.Env`, argv plumbing) |
| Git dependencies | Flux | 0178 stage 5 (`Flow.Process`) |
| Module-graph and compiler integration | Rust | — |

Only the last row must remain Rust: supplying module roots, computing the
configuration hash, and reading `.flxi` fingerprints are compiler-internal. The
boundary is a narrow one — the package manager decides *what* to build and hands
the compiler a resolved set of roots.

Note that the top four rows have **no dependency on 0178 at all**. They are
exactly [Phase 0](#phase-0-core-libraries-in-flux): the manifest parser,
version arithmetic, and resolver — the parts with the most interesting logic and
the highest test density — can be written in Flux immediately against in-memory
fixtures, while the OS capabilities land underneath them. That is the natural
first milestone and the most informative one, since it exercises ADTs, exhaustive
matching, generics, and effect purity on real software rather than examples.

The remaining rows map onto the later phases: lockfile I/O and the CLI entry
point to [Phase 1](#phase-1-projects-and-path-dependencies) and
[Phase 2](#phase-2-registry-lockfile-and-resolution), checksums and store keys to
[Phase 3](#phase-3-store-publishing-and-workspaces).

This analysis is a snapshot of the current tree; it should be re-run before any
implementation begins.

## Drawbacks
[drawbacks]: #drawbacks

1. **It is a large, permanent surface.** Manifest schema, lock format, index
   format, and store layout are all compatibility commitments, and every one is
   hard to change after the first external package exists.
2. **The one-version restriction will bite.** The diamond where two dependencies
   need incompatible majors of a third is unresolvable until symbol mangling
   exists. Historically this constraint is the one that makes solvers hard to
   write and failures hard to explain.
3. **No features / conditional compilation** means no portability story yet for
   code that must differ across platforms.
4. **A registry is an operational commitment** — hosting, availability,
   moderation, and security response — not just code.
5. **A hand-written TOML subset parser is a liability** if the manifest schema
   grows faster than expected. Mitigated by keeping it behind `Manifest.parse`.
6. **Namespace enforcement is a breaking change** for existing multi-file programs
   using `--root` with flat module names. Script mode keeps working, but code
   converted to a package must adopt namespaced modules.
7. **The store is the most complex component**, and its keys will be wrong at
   first: under-hashing configuration silently reuses incompatible artifacts, and
   the symptom appears far from the cause.
8. **Publishing is irreversible**, so the archive format and validation rules must
   be right before the first package ships.
9. **Building in Flux adds a dependency and a bootstrap question.** Phases 1–3
   gate on [0178](implemented/0178_os_capabilities_for_tooling.md), and a Flux-written package
   manager cannot be distributed *by* the package manager — it ships with the
   toolchain. Both are accepted deliberately: the goal is to learn what Flux can
   express, and a tool the language cannot write is itself the more important
   finding.
10. **Opportunity cost.** The effect system, typed holes, and diagnostics are all
    live workstreams competing for the same attention.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why a resolved lockfile rather than a set of pinning constraints?** A resolved
graph is read; a constraint file must be re-solved. Re-solving means build
reproducibility depends on the solver's own version and heuristics, so a toolchain
upgrade can silently change the build plan of an unmodified project. A resolved
graph with content hashes is also simpler to implement and easier to diff.

**Why a content-addressed store rather than a single output directory?** Because
Flux emits interface files, and an interface is a *configuration-dependent*
artifact: the same source compiled under `--strict` or `--optimize` yields a
different interface. A plain output directory keyed on target name has nowhere to
record that distinction, so it must either rebuild constantly or risk reusing an
incompatible artifact.

**Why forbid two versions of one package?** Not preference — the VM linker keys
globals by flat name and silently aliases on collision, so permitting duplicates
would produce wrong programs rather than errors.

**Why implement the full resolver in Phase 0 rather than a stub?** Because it is
pure then and will not be later. A resolver written against fixtures has a clean
seam between "what versions exist" and "which to pick"; one grown inside a
registry client tends not to.

**Why enforce namespacing in Phase 1, when nothing collides yet?** Because it
cannot be added later without breaking published packages, and because the
collision is a hard error rather than a warning.

**Why an append-only index rather than a queryable API?** Append-only is what makes
published versions immutable and makes `index-state` pinning possible, and it is
trivially cacheable and mirrorable.

**Why one lockfile per workspace rather than per member?** Because the
one-version-per-package rule is global to a linked program; per-member locks could
express a resolution that cannot be linked.

**Alternatives considered:**

- *Curated snapshots* — one blessed, mutually-compatible version set, no solver,
  trivially reproducible. Genuinely attractive for a young ecosystem. Rejected as
  the *primary* model because it requires an ongoing curator and makes any
  off-snapshot package painful; but it remains expressible later as a constraint
  set, and `index-state` pinning already delivers much of the benefit.
- *Git-only, no registry* — no hosting burden, but no discovery, no immutability
  guarantee, and no way to yank a compromised release.
- *Vendor everything into the repository* — reproducible, but does not scale and
  makes updates invisible in review.
- *Write it in Rust* — the pragmatic choice for shipping quickly, and explicitly
  not the goal: the point is to learn what Flux can express by making Flux express
  it.
- *Do nothing* — leaves Flux unusable outside its own checkout, which is
  disqualifying regardless of the rest of this proposal.

**Could this be a library instead?** No. It requires changes to module resolution,
root computation, and stdlib discovery inside the compiler.

## Prior art
[prior-art]: #prior-art

Package managers for compiled languages have converged on a common core, and this
proposal adopts it deliberately rather than inventing: a declarative manifest
normalized into a domain model, a resolved lockfile committed to version control,
an append-only registry index of immutable published versions, content-addressed
build artifacts, and fingerprint-based staleness checking. The specific lessons
worth restating, each drawn from an ecosystem that learned it expensively:

**Mutable global installation is the original failure mode.** A single mutable
package database combined with a single-instance restriction means installing one
package can break every package already compiled against the previous version,
producing states only fixable by deleting the whole database. Content-addressed,
immutable storage exists precisely to eliminate this, and it is the reason the
store is specified this way from the start rather than added later.

**Optional-feature systems are the most commonly regretted subsystem.** Where
features unify additively across dependency kinds and inactive target platforms,
enabling one in a test-only dependency silently alters what ships. The fix, once
an ecosystem is committed, is an opt-in second resolution pass — which never
fully recovers the intended semantics. This is why features are excluded here.

**Lockfile formats need a migration protocol before the second version exists.**
Without "preserve whatever version you find, and change the default only long
after support is ubiquitous," a project's lockfile oscillates between toolchain
versions and every commit churns.

**Build caches must not hash mtimes or absolute paths.** Container image layers
normalize timestamps, and users rename directories; hashing either produces
spurious full rebuilds that erode trust in the cache.

**Escape hatches become permanent.** An untyped `[metadata]` table, or a
build-script hook that lets a package replace the build driver with arbitrary
code, defeats introspection, requires its own bootstrap resolution, and cannot be
withdrawn once published packages depend on it. Hence the strictly declarative
manifest proposed here.

**Retrofitted resolver semantics stay unfinished.** Distinctions the resolver must
enforce — public versus private dependencies being the standard example — are
hard to add once resolution exists, because they change what counts as a valid
solution. This is why the single-instance rule is encoded in the activation key
now, even though the relaxation is deferred.

**Build profiles are the right place for backend selection — but only if the
backend is part of the cache key.** Flux is unusual among the ecosystems studied
here in shipping *two* backends (bytecode VM and LLVM-native) from one frontend,
so "which backend does `flux build` use?" is a question neither Cargo nor Cabal
had to answer in quite this form. Their answers differ sharply, and only one is
worth copying.

Cabal models no code-generation backend at all. There is no `-fllvm` or `-fasm`
in its source; `GhcObjectCode` and `GhcByteCode` are defined in
`Distribution.Simple.Program.GHC` but never constructed. Bytecode is never a
build backend — `cabal repl` goes through `--interactive` (GHCi defaults to
bytecode implicitly, forced to `-O0`), and `--enable-library-bytecode` emits
`.gbc` files *alongside* native objects, with bytecode-only builds an explicit
error. Selecting LLVM means writing `ghc-options: -fllvm`, which Cabal passes
through as an opaque string; it invalidates the package hash only incidentally,
as part of `pkgHashProgramArgs`. There are also no dev/release profiles: a
single `OptimisationLevel` axis defaults to `-O1` identically for `cabal build`
and `cabal install`, and the "Build profile:" line in its output is cosmetic.

Cargo makes the backend a **profile field** (`Profile::codegen_backend`) and —
the part that matters — includes it in `Profile::comparable()`, which feeds both
the fingerprint and the `-C metadata` hash. Switching backends therefore
invalidates correctly and keeps artifacts separate rather than clobbering them.
Three further details are worth adopting wholesale: profiles are ordinary data
with only `dev` and `release` as roots (`test`/`bench`/`doc` merely `inherits`
them, with cycle detection); `[profile.dev.package."*"]` is skipped for
workspace members, which is what expresses "optimise my dependencies, not my
code"; and the output directory comes from the profile *name* while everything
else rides in the hash, so `dev` and `test` can share `target/debug` safely.

Flux already has the two hard prerequisites — `CacheLayout::vm_dir()` /
`native_dir()` keep the backends' artifacts apart, and `CACHE_EPOCH` plus
interface fingerprinting give a working invalidation story. What is missing is
the manifest surface. Two facts argue against simply defaulting `release` to the
native backend, though, and both are recorded in
[known_issues.md](../known_issues.md): the VM overflows on non-tail recursion at
a depth the native backend survives ([KI-030](../known_issues.md#ki-030)), so a
VM-dev / native-release split would put the *stricter* backend in the cheaper
configuration; and TCP has no native parity yet
([KI-009](../known_issues.md#ki-009)). Backend-per-profile should therefore land
only once parity is close enough that the two configurations agree on what runs.

**Alternative resolution strategies worth tracking:** minimal-version selection
(simpler and more predictable than backtracking, but it depends on an
import-compatibility discipline Flux does not enforce); nested dependency trees
(sidestep the single-instance problem entirely at the cost of duplication, which
Flux's flat global namespace cannot express); and PubGrub-style resolution, whose
main draw is markedly better conflict explanations and which is a strong candidate
if the Phase 2 resolver's error quality proves inadequate.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

**Spanning the whole proposal:**

1. **Is the one-version-per-package restriction acceptable for 1.0, or should
   per-package symbol mangling be scheduled first?** The highest-stakes question
   here. It is a *linker* decision, not a package-manager one, and it determines
   whether the resolver is easy or hard, whether diamond conflicts are reportable
   or fatal, and how much Phase 2 has to explain to users.
2. **Namespace derivation** for names that do not capitalize cleanly
   (`http-client`, `json2`, single-character names). It affects every published
   package name forever.
3. **How much of the toolchain should be Flux versus Rust over time.** Phases 0
   and 1 held the line: all manifest parsing, namespace derivation, dependency
   walking, and scaffolding are Flux, and the Rust side only runs the resolver
   and converts its records into module roots. Whether that holds through
   Phases 2–3 is still open.
4. **Sequencing against [0178](implemented/0178_os_capabilities_for_tooling.md).** Phase 0 is
   unblocked; Phases 1–3 gate on specific stages. Whether 0178 proceeds in
   parallel or as a prerequisite is a scheduling decision.

**Phase 0** — settled by the implementation:

1. ~~Which TOML subset the parser accepts.~~ `Flume.Toml` targets the manifest
   schema: tables, arrays of tables, and string/integer/boolean values.
2. ~~Strict semver or a Flux-specific relaxation.~~ `Flume.Version` accepts
   two-component requirements such as `1.2` in manifests.
3. ~~Whether the resolver's conflict type is shared with compiler
   diagnostics.~~ It stays package-manager-local. The compiler surfaces
   resolution failures through `E470`, carrying the resolver's own message.


**Phase 1** — settled by the implementation:

1. ~~Namespace derivation for names that do not capitalize cleanly.~~
   `Flume.Manifest.derive_namespace` handles the common cases
   (`http-client` → `HttpClient`), and the explicit `namespace` field covers
   the rest. No case has yet needed more.
2. ~~Root module `src/Json.flx` or `src/lib.flx`?~~ `src/Json.flx`: the
   namespace is visible in the tree, and `flux init --lib` scaffolds it.
3. ~~Whether `edition` should exist before there is a second edition.~~ It is
   parsed and written by `flux init`, and otherwise unused. Kept so manifests
   written today remain readable when a second edition exists.

4. ~~How `flux test` aggregates across files.~~ Discovery matches the last
   dot-separated segment of a global's name, so every compiled module in the
   graph contributes its tests and they are reported by qualified name
   (KI-020). `--test-filter` matches the qualified name, covering the
   `--filter <s>` slot in the CLI surface under an existing flag name.


**Phase 2:**

1. Registry hosting, naming policy, and squatting rules.
2. Exact lockfile v1 serialization, validated against real merge conflicts.
3. Whether yanking is available at launch, and what it means for an existing lock.
4. Whether git dependencies ship in this phase or wait for
   [0178](implemented/0178_os_capabilities_for_tooling.md) stage 5.
5. Publish-age preference defaults — the supply-chain mitigation the index format
   must leave room for.


**Phase 3:**

1. Store garbage collection: LRU by access time, explicit `flux clean --store`, or
   both.
2. Whether Phase 2's `target/flux/` dependency builds should be migrated into the
   store or left as a fallback.
3. Exact archive format and what it excludes (`target/`, VCS metadata, editor
   files).
4. Whether workspace field inheritance ships with this phase or later.
5. Path-length limits on Windows, which constrain how long store paths may be.


**Explicitly out of scope:** features/conditional compilation, build scripts,
cross-compilation, binary artifact distribution, and workspace-wide version
unification.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Per-package symbol mangling**, lifting the one-version restriction — the
  single change that most improves resolution.
- **Curated snapshots** expressed as constraint sets, giving snapshot-grade
  reproducibility without abandoning the solver.
- **PubGrub-style error reporting** for resolution failures.
- **`flux vendor`** for air-gapped builds; **`flux audit`** against an advisory
  database.
- **Distributed/shared build cache** — the content-addressed store makes this
  mostly a transport problem.
- **Documentation generation and hosting** for published packages.
- **Build profiles, with the backend as a profile field.** Flux ships two
  backends from one frontend, and `flux build` currently has no way to choose.
  Cargo's design is the one to copy (see *Prior art*): profiles as ordinary data
  with `dev`/`release` as the only roots, the backend included in the cache key
  so switching invalidates correctly, and the artifact directory keyed on the
  profile name. `CacheLayout::vm_dir()` / `native_dir()` and `CACHE_EPOCH`
  already provide the separation and invalidation this needs; what is missing is
  the `flux.toml` surface. Gated on backend parity — see
  [KI-030](../known_issues.md#ki-030) and [KI-009](../known_issues.md#ki-009).
- **Effect-aware metadata**: Flux knows the effect signature of every public
  function, so a package index could expose "this library performs IO" — genuinely
  novel, and a supply-chain-transparency story that package managers for
  effect-untracked languages cannot offer.
