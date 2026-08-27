- Feature Name: `flume_distribution_and_repository_split`
- Start Date: 2026-08-27
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Ship Flume as its own executable, and decide where its source lives. This
proposal recommends **building a separate `flume` binary from source that stays
in this repository**, and recommends **against** extracting `lib/Flume/` into a
standalone GitHub repository. It also specifies the prerequisite that neither
option can skip: a relocatable install layout, which does not exist today.

Three questions are answered separately because they have different answers:

| Question | Recommendation |
|---|---|
| Can Flume be a standalone binary? | Yes — it already works; productionize it |
| Should `flume` be a separate executable? | Yes, in phase 2, after the install layout lands |
| Should Flume move to its own repository? | **No** — not until the compiler stops co-changing with it |

## Motivation
[motivation]: #motivation

Flume is the package manager for Flux: `flux.toml`, `flux.lock`, path and Git
dependencies, workspaces, and the `add`/`remove`/`tree`/`update`/`publish`
commands, delivered by proposal 0177. Its logic is ~9,000 lines of Flux in
`lib/Flume/`; the user-facing commands are Rust in `src/cli/package.rs`, which
spawns the `flux` binary on a generated shim and reads a text record back.

Users reasonably expect the Rust arrangement: `cargo` is a program you run,
distinct from `rustc`. Today `flux build` is a compiler subcommand, and Flume is
invisible — it has no binary, no version of its own, and no way to be installed
or upgraded independently. Three concrete problems follow.

**1. There is no way to install Flux at all.** `scripts/release/` contains
`release_check.sh`, `release_cut.sh`, and `bench_aether.sh` — nothing that
produces an installable layout. The runtime already supports one:
`find_flow_dir` (`src/driver/frontend.rs:29`) resolves the stdlib through
`$FLUX_LIB_DIR/Flow`, then `lib/Flow` walking up from the entry file, then
`lib/Flow` beside the executable — the installed `<prefix>/bin/flux` +
`<prefix>/lib/Flow` convention. `find_project_root`
(`src/shared/cache_paths.rs:112`) keys on `flux.toml`, with `Cargo.toml` only as
a fallback for this repo's own test corpus. This was verified empirically: a
package built and run entirely outside the checkout works. The capability
exists and nothing exercises it, which means it can regress silently.

**2. The compiler and the package manager are entangled in a way that has no
design rationale.** `resolve_project_roots` (`src/driver/manifest_roots.rs:97`)
is called from `collect_module_roots` inside the compile path, so every
`flux run` of a package spawns Flume to learn its module roots. The dependency
runs `flux → flume`, the reverse of `cargo → rustc`. This is why
`src/cli/package.rs:308` calls `run_pipeline` in-process rather than invoking
anything: the two halves are one binary by construction.

**3. The Rust/Flux boundary is a stringly-typed protocol that has already
produced bugs.** Reviewing it for 0.0.7 found two: a failed command whose
message quoted a line beginning `ok<TAB>` was reported as success and exited 0,
and a decoder that rewrote `\t`/`\n` in message text, corrupting Windows paths
and fabricating an extra field in resolver records. Both are fixed, but they
are the symptom of a boundary that is sniffed rather than framed — and a
boundary that becomes a *supported* interface if Flume ships separately.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

After this proposal, a Flux installation looks like this:

```
<prefix>/bin/flux        the compiler
<prefix>/bin/flume       the package manager
<prefix>/lib/Flow/       the standard library (Flux source)
<prefix>/lib/Flume/      the package manager's source
```

`flume` handles everything about *packages*: reading manifests, resolving
dependencies, editing `flux.toml`, scaffolding, and drawing the tree. `flux`
handles everything about *compiling*. When `flume build` needs a compile, it
invokes `flux`, the way `cargo` invokes `rustc`.

The existing `flux build` / `flux run` / `flux test` subcommands keep working;
they become thin forwards to `flume`. No user-visible workflow changes, and no
manifest or lockfile format changes.

For contributors, the mental model becomes: **`lib/Flume/` is a Flux program
that this repository happens to build, exactly like the standard library in
`lib/Flow/`.** It is not a plugin and not a third-party package. That is the
same relationship GHC has with its boot libraries.

### Why Flume is not "written in a language that cannot run it"

A natural objection is the chicken-and-egg: Flume is written in Flux, so
running it requires a Flux compiler, so it cannot be standalone. This is
**false**, and it is worth stating plainly because it shaped earlier thinking.

The Flux source is compiled *into* the binary. The compiler is needed to
**build** `flume`, not to **run** it. Verified on 2026-08-27:

```sh
flux --native --emit-binary -o flume shim.flx   # shim imports Flume.Cli
```

produces an 18 MB native executable. Run outside the checkout, with no
`lib/Flow` present, it successfully performed `init`, `add`, `tree`, `entry`,
and `profile`, including path-dependency resolution. This is exactly cabal's
situation: cabal is written in Haskell, needs GHC to build, and ships as a
binary.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### The actual boundary

The split is **not** Rust-versus-Flux. It is manifest operations versus compile
operations:

| Standalone today (verified) | Requires the compiler |
|---|---|
| `init`, `new`, `add`, `remove`, `tree`, `entry`, `profile`, `metadata` | `build`, `run`, `test`, `check` |

Everything in the left column is pure manifest work and already runs in a
standalone binary. Only the right column needs `flux`, and only by invoking it.

### Phase 1 — Relocatable install layout (prerequisite)

Add `scripts/release/install.sh` producing `<prefix>/bin` + `<prefix>/lib`, and
a CI job that installs to a temporary prefix, then builds and runs a package
from a directory outside the checkout. This is the step that converts an
untested capability into a guaranteed one. `lib/Flume/` is currently found via
`flow_dir.parent()` (`src/driver/frontend.rs:244`) rather than through a
discovery path of its own; the install script must therefore place `Flow` and
`Flume` as siblings, and that expectation needs a test.

Phase 1 stands alone. If nothing else in this proposal is adopted, Flux still
needs it in order to be installable.

### Phase 2 — Build and ship a `flume` binary

Add a release step emitting `flume` via `--emit-binary`, and give it a
`--version` reporting both its own version and the `flux` it was built against.

`flume` gains the compile-driving commands by **spawning `flux`**, resolving it
in this order — matching cargo's `Tool::Rustc` resolution
(`~/Downloads/Github/cargo/src/context/mod.rs:540`):

1. `$FLUX` if set
2. `flux` beside the running `flume` executable
3. `flux` on `PATH`

The inversion this requires is the one piece of real design work.
`resolve_project_roots` must stop calling Flume from inside the compile path.
Instead `flume` resolves roots first and passes them to `flux` as explicit
`--root` arguments, which the compiler already accepts. That deletes the
recursion the `FLUX_SKIP_MANIFEST` guard (`src/driver/manifest_roots.rs:24`)
exists to break, and removes the roots cache
(`roots_cache_path`, `:213`) whose only purpose is to avoid re-spawning the
resolver mid-compile.

### Phase 3 — Harden the boundary into an interface

If `flume` and `flux` ship as separate binaries, their protocol becomes a
compatibility surface across versions. The current `ok<TAB>message` record must
be replaced with a framed, versioned format before that happens — the two bugs
found in review are what an unframed protocol produces when both halves ship
together, and version skew makes that strictly worse. This is a hard gate on
phase 2, not a follow-up.

### Repository split: recommended against

The evidence is in the commit history. Of the 15 commits touching `lib/Flume/`
in the last three months, **12 also touched `src/`**. The co-changing files are
not incidental — `src/cli/package.rs`, `src/cli/cmdline.rs`,
`src/driver/manifest_roots.rs`, `src/shared/cache_paths.rs` — the CLI surface
and the resolver bridge. Representative examples:

- `feat(flume): add store, workspace, metadata, and publish support` — 4 `src/`
  files alongside the Flux changes
- `feat(pkg): fetch git dependencies from GitHub/GitLab with lockfile pinning`
  — 4 `src/` files
- `refactor(flume): group the package manager into nested modules` — 3 `src/`
  files

A separate repository turns each of those single commits into a cross-repo pair
with a version bump between them, for a project where one person is making both
edits. That is the cost. The benefit — independent release cadence, external
contribution, a clean public boundary — accrues only once the interface is
stable, and the interface is provably not stable yet.

There is a second reason. Flume is the largest Flux program that exists, and it
is the compiler's most valuable test corpus. `docs/known_issues.md` records
limitations found *by writing it* — KI entries on `Flume.Version.parse`,
`Flume.Manifest`'s TOML chaining, and the `Flume.Manifest.Dep` /
`Flume.Resolve.Dep` import collision. 0177 made this explicit: *"the goal is to
learn what Flux can express, and a tool the language cannot write is itself the
more important finding."* In-repo, a compiler change that regresses Flume fails
CI immediately. Split out, that signal arrives whenever someone next bumps the
dependency.

**The condition to revisit:** when a release of Flume ships without a
corresponding `src/` change — that is, when the 12-of-15 coupling ratio has
gone to roughly zero — the argument reverses and the split becomes cheap.
Phases 1–3 are what drive the ratio down, so this proposal is the path to a
future split, not an argument against it forever.

## Drawbacks
[drawbacks]: #drawbacks

- **Two binaries can skew.** One `flux`, one `flume`, two versions, one
  protocol between them. The single-binary design has none of this. Phase 3
  exists to contain it, but containment is not elimination.
- **Phase 2 deletes a real optimization.** The roots cache avoids re-spawning
  the resolver on every compile. Passing roots down from `flume` removes the
  need, but any path still entering through `flux` directly loses it.
- **`--emit-binary` becomes release-critical.** It is currently an opt-in LLVM
  feature. Shipping `flume` from it makes the native backend a hard release
  dependency on every platform Flux targets.
- **An 18 MB binary for manifest editing** is large. It is mostly runtime and
  stdlib, and it will not shrink without work on native code size.
- **Phase 1 alone may be enough.** If the motivation is "users can install and
  use Flux", phase 1 delivers that and phases 2–3 are ergonomics.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not keep everything in one binary?** This is the serious alternative, and
it is what Flux has today. `go` does exactly this — `go build`, `go mod`, `go
get` are one binary, and the Go team considers it a feature. It eliminates
version skew and the protocol entirely. The case against is discoverability and
convention: users expect a package manager to be a thing, and vendoring every
packaging concern into the compiler binary means the compiler cannot be
released without also releasing packaging. Phase 1 is compatible with this
alternative; phases 2–3 are the ones being chosen against it.

**Why not extract Flume to its own repository now?** Covered above: 12 of 15
recent Flume commits also changed the compiler.

**Why not rewrite Flume in Rust?** It would remove the bootstrap question and
the protocol at a stroke. 0177 rejected this deliberately (line ~1249: it
"defeats introspection" and gives up the dogfooding), and it would discard the
compiler's best test corpus. Not recommended.

**Impact of doing nothing:** Flux stays uninstallable. That is the part that
should not wait, whatever is decided about phases 2–3.

## Prior art
[prior-art]: #prior-art

**Cargo (Rust)** is the model users will expect. Cargo invokes `rustc` as a
subprocess, resolving it via `$RUSTC`, then the sysroot, then `PATH`
(`src/context/mod.rs:540`). Critically, cargo is *not* written in Rust-the-
package-being-managed in any circular sense — it is an ordinary Rust program,
so it has no bootstrap problem. Flume does, which is why cabal is the closer
analogue.

**Cabal (Haskell)** is the precedent that matters, and it was examined
directly. `cabal-install` is written in Haskell and manages Haskell packages —
the same circularity as Flume. It solves it with `bootstrap/bootstrap.py`, a
521-line script driven by per-GHC-version JSON files pinning 39 dependencies,
used only to build cabal on a platform with no working cabal. The lesson is
that self-hosted package managers are normal and shippable, and that the cost
is a bootstrap path. **Flume's bootstrap is dramatically cheaper than cabal's**
— one `flux --emit-binary` invocation, because Flume depends only on the
standard library shipped beside it, with no third-party dependency graph to
resolve. Cabal also keeps `Cabal`, `Cabal-syntax`, and `cabal-install` in one
repository despite them being separately released packages, which is the
in-repo-with-separate-artifacts arrangement this proposal recommends.

**Go** ships one binary and is the strongest evidence that the split is a
choice rather than a necessity.

**GHC** treats its boot libraries as in-tree despite their being independently
versioned and released — the direct precedent for keeping `lib/Flume/` in this
repository while shipping it as its own artifact.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- What exactly replaces the `ok<TAB>message` record in phase 3 — a length-
  prefixed frame, JSON lines, or an exit-code-plus-structured-stderr
  convention? Phase 3 cannot start until this is settled.
- Does `flume` need to work with no `flux` present at all? Everything in the
  manifest column does today. Whether that is a *supported* configuration or an
  accident determines whether it needs tests.
- What is the version-compatibility policy between `flux` and `flume` — exact
  match, semver range, or a protocol version negotiated at startup?
- Is `--emit-binary` reliable enough on every target platform to be release-
  critical? Currently unproven outside macOS/arm64.
- Should `flux build` remain as an alias indefinitely, or be deprecated once
  `flume build` exists?

## Future possibilities
[future-possibilities]: #future-possibilities

- **A `flume`-managed toolchain**, the way `rustup` manages `rustc` — plausible
  once `flume` can spawn a chosen `flux`.
- **Extraction to its own repository**, once the coupling ratio approaches
  zero. This proposal is the path there.
- **Hosted registry transport**, still blocked on KI-035 (`Flow.Http` speaks no
  TLS). A separate `flume` release cadence would make registry work easier to
  ship independently of compiler releases — one of the strongest future
  arguments for the split.
- **Publishing Flume to its own registry once one exists**, which 0177 notes is
  impossible for a tool that ships with the toolchain — and becomes possible
  exactly when the split happens.
