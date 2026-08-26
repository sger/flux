- Feature Name: OS capabilities for tooling (`Flow.Fs`, `Flow.Path`, `Flow.Crypto`, `Flow.Env`, recoverable I/O)
- Start Date: 2026-08-21
- Status: Draft
- Proposal PR:
- Flux Issue:
- Required by: [0177_package_manager.md](0177_package_manager.md) — a Flux-written package manager is blocked on every capability here
- Builds on: the primop contract ([0164](0164_internal_primop_contract_and_stdlib_surface.md)), the effect system ([0161](0161_effect_system_decomposition_and_capabilities.md)), and the I/O effect-handler migration ([0165](0165_io_primop_migration_to_effect_handlers.md))

# Proposal 0178: OS Capabilities for Tooling

## Summary
[summary]: #summary

Give Flux the operating-system surface a program needs to be a *tool* rather than
only a computation: filesystem and path manipulation, cryptographic hashing,
process environment and arguments, subprocess execution, and — the one that is a
semantic change rather than an addition — **recoverable I/O errors**.

The motivating consumer is a Flux-written package manager
([0177](0177_package_manager.md)), but nothing here is package-manager-specific.
These are the capabilities that separate a language that can compute from one
that can write its own tooling.

## Motivation
[motivation]: #motivation

### The gap is real, and it is narrow

An investigation for [0177](0177_package_manager.md) asked whether a package
manager could be written in Flux. The result was sharper than expected: **there
is no type-system gap.** Flux's ADTs, exhaustive matching, generics, HAMT maps,
effect tracking, and structured concurrency are already sufficient to express a
dependency resolver — verified by writing and running one's core traversal.

What blocks it is entirely OS surface area. The complete filesystem vocabulary
today is four primops:

```
ReadFile = 50, WriteFile = 51, ReadStdin = 52, ReadLines = 53
```

There is no way to list a directory, create one, delete a file, test whether a
path exists, join two path segments, read an environment variable, read the
process's own arguments, run a subprocess, or hash a byte string. A Flux program
cannot answer "does `flux.toml` exist in this directory?"

This is not an oversight so much as a consequence of history: Flux's I/O story
was driven by async networking ([0174](../0174_async_effect_concurrency.md)), which
produced an excellent TCP and HTTP stack — `Flow.Http.get` performs real network
requests today — while leaving local-machine I/O at the level needed for example
programs.

### Recoverable I/O errors are the sharpest edge

`read_file` on a missing path does not return a value the program can inspect; it
aborts:

```
error[E1009]: read_file failed for '/tmp/nope.txt': No such file or directory (os error 2)
```

The primop is classified under `FileSystem` only
([../../src/syntax/builtin_effects.rs:142](../../src/syntax/builtin_effects.rs#L142)),
with no failure effect, so there is nothing to handle and nothing to match on.
Any program that must *probe* the filesystem — every build tool, every
configuration loader, every CLI — is unwritable.

This is the one item in this proposal that is a **semantics change** rather than
a new capability, and it is the one most worth getting right, because every
capability added below inherits the decision.

### Feasibility is established, not assumed

A throwaway spike added a `FileExists` primop end-to-end — enum variant, opcode
decode, name table, two registration tables, effect classification, ANF
representation, effectful-primop lists, display, IR lowering, and VM
implementation — and ran it from Flux:

```flux
fn probe(p: String) -> Bool with FileSystem { file_exists(p) }

fn main() -> Unit with FileSystem {
  println(probe("Cargo.toml"))            // true
  println(probe("definitely-not-here.txt")) // false
}
```

Two findings from that spike shape this plan:

1. **The cost per primop is small and mechanical** — roughly nine edit sites and
   a few lines each. The spike was reverted; nothing in the tree depends on it.
2. **The compiler enforces completeness.** Adding an enum variant produced
   `error[E0004]: non-exhaustive patterns` at exactly the sites that still needed
   handling. Extending the primop surface is type-safe: it is not possible to
   half-add a primop and discover it at runtime.

The native backend is the one place where care is needed — each primop also needs
a C implementation in [`runtime/c/flux_rt.c`](../../runtime/c/flux_rt.c) for
VM/native parity, and the existing `flux_read_file` shows the pattern (including
its `flux_panic` error path, which item 1 below revises).

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Recoverable I/O

Fallible operations return a value describing the outcome instead of aborting.
The shape follows `Flow.Http`'s existing precedent, where `HttpParseUrl` returns
`HttpUrlParsed | HttpUrlFailure` rather than failing the program:

```flux
import Flow.Fs as Fs

fn load_manifest(dir: String) -> Option<String> with FileSystem {
  match Fs.read_file(Path.join(dir, "flux.toml")) {
    Ok(contents) -> Some(contents),
    Err(_)       -> None
  }
}
```

The existing aborting `read_file` remains available and unchanged, so example
programs and teaching material keep working; `Flow.Fs.read_file` is the
error-returning sibling for programs that must handle failure.

### The new modules

```flux
import Flow.Fs as Fs        // read, write, list, create, remove, stat
import Flow.Path as Path    // join, parent, file_name, extension, normalize
import Flow.Crypto as Crypto// sha256
import Flow.Env as Env      // args, var, cwd
import Flow.Process as Proc // run a subprocess, capture output
```

Each is effect-tracked, so a function's signature states what it touches:

```flux
fn scaffold(name: String) -> Unit with FileSystem {
  Fs.create_dir_all(Path.join(name, "src"))
  Fs.write_file(Path.join(name, "flux.toml"), manifest_for(name))
}

fn manifest_for(name: String) -> String {   // no effects: provably pure
  "[package]\nname = \"" ++ name ++ "\"\n"
}
```

That last point is the reason this is interesting beyond unblocking a tool: in
Flux, a manifest *parser* can be statically guaranteed pure while the *fetcher*
that feeds it wears its I/O in its type. Few languages can express that
distinction about their own build tooling.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Item 1 — Recoverable I/O errors (do this first)

Everything else inherits this decision, so it must land before the capabilities
that would otherwise be specified twice.

**Design:** fallible operations return `Result<T, IoError>`, where `IoError` is
an ADT carrying a machine-readable kind and a human-readable message:

```flux
data IoError { IoError { kind: IoErrorKind, message: String, path: String } }
data IoErrorKind { NotFound | PermissionDenied | AlreadyExists | NotADirectory | Other }
```

A structured `kind` matters: "not found" and "permission denied" demand different
handling, and forcing callers to pattern-match on message text would bake error
strings into a compatibility surface.

**Mechanism:** the VM returns `Ok(Value::Adt(...))` for both the success and
failure cases rather than `Err(String)`, exactly as `vm_http_parse_url` does
([../../src/vm/core_dispatch.rs:4475](../../src/vm/core_dispatch.rs#L4475)). The
C runtime's equivalents return the same tagged shape instead of calling
`flux_panic`.

**Compatibility:** existing `read_file` / `write_file` / `read_lines` keep their
aborting behavior. New primops are new opcodes.

**Open question:** whether fallible I/O should additionally be classified under
the existing `Exn` effect so it can be intercepted by a handler. `Result` and
`Exn` are complementary, not alternatives, but the interaction should be settled
before the surface is large.

### Item 2 — `Flow.Path` (pure, no effects, no primops) — IMPLEMENTED

Path manipulation is string manipulation. It needs no OS access, and therefore no
primops and no effects — it can be written **entirely in Flux today**, on top of
the existing `split` / `substring` / `string_concat`.

```flux
Path.join(a, b)      Path.parent(p)     Path.file_name(p)
Path.extension(p)    Path.normalize(p)  Path.is_absolute(p)
```

This should be built first, before any primop work: it is immediately useful, it
is a genuine test of whether non-trivial library code is comfortable to write in
Flux, and it has zero dependency on the rest of the proposal. Separator handling
is a Windows-portability question, not a language question.

**Status: shipped** as `lib/Flow/Path.flx`.

One naming note for the later stages. The public splitter is called `components`
rather than `split` because a module-level `fn split` shadows the builtin
`split(s, delim)` for every function in the same module — so `Flow.Path` could
not have used the builtin internally had it defined its own `split`. This is
ordinary lexical shadowing behaving exactly as
[`primops_vs_base.md`](../../internals/primops_vs_base.md) specifies ("if `foo` is
shadowed by a local/function/global symbol, primop and Base-fastcall lowering are
both skipped"), not a compiler defect: locals correctly win over primops, and
over-applying the shadowing local reports `E056` against the *local* arity.

The practical consequence is only that a module cannot both define and call a
builtin of the same name. `Flow.Fs` (Item 3) is the case to watch, since it
declares `read_file` / `write_file` against same-named builtins — it must either
pick distinct names or route through the `__primop_*` intrinsics rather than the
bare builtin names.

Coverage is three-layered: `tests/flux/stdlib_path.flx` (28 behavioral tests),
`tests/vm_runtime/stdlib_path_tests.rs` (drives the fixture, and asserts the
module is neither auto-injected nor effectful), and unit tests in
`src/lsp_support.rs` guarding the duplicated `FLOW_PRELUDE_MODULES` list.

### Item 3 — `Flow.Fs` (filesystem)

| Operation | Signature | Effect |
|---|---|---|
| `exists` | `String -> Bool` | `FileSystem` |
| `read_file` | `String -> Result<String, IoError>` | `FileSystem` |
| `write_file` | `(String, String) -> Result<Unit, IoError>` | `FileSystem` |
| `list_dir` | `String -> Result<Array<String>, IoError>` | `FileSystem` |
| `create_dir_all` | `String -> Result<Unit, IoError>` | `FileSystem` |
| `remove_file` | `String -> Result<Unit, IoError>` | `FileSystem` |
| `remove_dir_all` | `String -> Result<Unit, IoError>` | `FileSystem` |
| `rename` | `(String, String) -> Result<Unit, IoError>` | `FileSystem` |
| `is_dir` / `is_file` | `String -> Bool` | `FileSystem` |
| `metadata` | `String -> Result<FileMeta, IoError>` | `FileSystem` |

`rename` is called out because atomic rename is what makes a content-addressed
store safe against concurrent writers ([0177](0177_package_manager.md)); it must
map to the platform's atomic primitive, not to copy-then-delete.

`FileMeta` should carry size and modification time. Note that
[0177](0177_package_manager.md) requires build caches never to *hash* an mtime —
exposing it is still correct, because comparing mtimes is fine; hashing them is
not.

### Item 4 — `Flow.Crypto` (hashing)

```flux
Crypto.sha256(data: String) -> String       // lowercase hex
Crypto.sha256_file(path: String) -> Result<String, IoError> with FileSystem
```

`sha256_file` exists separately so large files can be hashed incrementally rather
than read fully into memory.

The runtime cost is near zero on the VM side: `sha2` is already a compiler
dependency, and the codebase already hand-rolls hex encoding in three places
(`mod hex` in `module_interface.rs`, `bytecode_cache/module_cache.rs`, and
`llvm/module_cache.rs`) — which should be consolidated as part of this work. The
native backend needs a C implementation or a linked library.

Hashing is pure — same input, same output — so it carries **no effect**.

### Item 5 — `Flow.Env` (process environment)

```flux
Env.args() -> Array<String>                 with Env
Env.var(name: String) -> Option<String>     with Env
Env.cwd() -> Result<String, IoError>        with Env
Env.home_dir() -> Option<String>            with Env
```

This introduces a new fine-grained effect label, `Env`, coarsening to `IO`
alongside `Console`, `FileSystem`, and `Stdin`
([../../src/syntax/builtin_effects.rs:160](../../src/syntax/builtin_effects.rs#L160)).
Reading the environment is genuinely a capability — it is ambient input that
makes a function non-deterministic — and it should be visible in signatures for
the same reason `FileSystem` is.

`Env.var` returns `Option`, not `Result`: an unset variable is an ordinary
condition, not an error.

**Note:** `Env.args()` requires the driver to thread process arguments through to
the running program, which it does not do today. That plumbing is a prerequisite
for any Flux CLI and is the smallest item here that is *not* purely additive.

### Item 6 — `Flow.Process` (subprocess execution)

```flux
data ProcOutput { ProcOutput { status: Int, stdout: String, stderr: String } }

Proc.run(cmd: String, args: Array<String>) -> Result<ProcOutput, IoError> with Process
```

A distinct `Process` effect, coarsening to `IO`. Subprocess execution is strictly
more powerful than filesystem access — it can do anything the user can — so
collapsing it into `FileSystem` would understate what a signature permits.

This is deliberately last and deliberately minimal. It exists because git
dependencies require shelling out to `git`, and because it is the escape hatch
that keeps missing capabilities from being blocking. No shell interpretation: an
explicit argument vector, never a command string, so quoting bugs cannot become
injection bugs.

### Sequencing

| Stage | Contents | Rationale |
|---|---|---|
| **0** | `Flow.Path` — **implemented** (`lib/Flow/Path.flx`) | Pure Flux, no primops, no blockers. Immediately useful. |
| **1** | `IoError` + `Result`-returning I/O — **implemented** | Everything downstream inherits the error model |
| **2** | `Flow.Fs` — **implemented** (`lib/Flow/Fs.flx`) | The bulk of the capability gap |
| **3** | `Flow.Crypto` — **implemented** (`lib/Flow/Crypto.flx`) | Small, self-contained, unblocks checksums and the store |
| **4** | `Flow.Env` + argv plumbing — **implemented** (`lib/Flow/Env.flx`) | Unblocks writing a CLI at all |
| **5** | `Flow.Process` — **implemented** (`lib/Flow/Process.flx`) | Escape hatch; git dependencies |

All stages are implemented. Each capability has a Flux fixture under
`tests/flux/stdlib_*.flx` run on both backends, plus a Rust target asserting the
effect-annotation behaviour the fixture cannot check about itself.

Stages 2–5 are independent of one another once stage 1 lands, so they can proceed
in parallel or be reordered by need.

### Cost estimate

Per primop, from the measured spike: an enum variant and opcode, a decode arm, a
name-table entry, two registration entries, an effect classification, an ANF
representation, an effectful-primop list entry, a display arm, an IR-lowering
arm, a VM implementation (~5 lines), and a C runtime implementation for native
parity. The compiler's exhaustiveness checking makes the Rust-side sites
self-locating: add the variant, then fix every `E0004` until it builds.

The total is roughly 25–30 new primops. The dominant cost is not any single
primop but (a) settling the error model in stage 1 and (b) native-backend parity,
which doubles the implementation surface for every operation.

### Testing

- Per-capability unit tests in Flux, run on **both** the VM and native backends —
  backend parity is the main risk, and the existing `parity-check` harness covers
  exactly this.
- Error-path tests for every fallible operation: missing file, permission denied,
  wrong type (file where a directory is expected).
- `Flow.Path` property tests: `join` then `parent` round-trips; `normalize` is
  idempotent.
- `Crypto.sha256` against published test vectors, and cross-checked against the
  compiler's own `sha2` output.
- Effect-annotation tests: calling any of these without the required effect must
  be a compile error (the spike confirmed this works).
- A `Flow.Process` test asserting arguments are passed as a vector and never
  shell-interpreted.

## Drawbacks
[drawbacks]: #drawbacks

1. **This is a large, permanent surface.** Roughly 25–30 primops and five modules
   become compatibility commitments, and the primop table is already the widest
   part of the compiler.
2. **Native parity doubles the work** and is where divergence bugs will appear.
   Every operation needs a C implementation matching VM semantics exactly,
   including error shapes.
3. **`Flow.Process` is a capability escape hatch.** Once a program can run
   arbitrary subprocesses, the effect system's guarantees about what a program
   touches are bounded by what that subprocess does.
4. **Two error idioms will coexist** during and after the transition — aborting
   `read_file` and `Result`-returning `Fs.read_file`. That is the price of not
   breaking existing programs, but it is a teaching cost.
5. **Opportunity cost.** This is squarely tooling infrastructure, not language
   research, competing with the effect system and diagnostics workstreams.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why `Result` rather than making I/O failures an `Exn` effect?** A handler-based
design is defensible and arguably more idiomatic for an effect-oriented language.
`Result` is proposed because the failure is *local and expected* — "this file may
not exist" is control flow, not an exceptional condition — and because it forces
the caller to acknowledge the failure at the call site. The two compose: `Exn`
classification can be added later for callers who prefer to handle rather than
match. Deciding `Result` first and `Exn` second is reversible; the opposite order
is not.

**Why not one `Flow.Os` module?** Effect granularity is the reason to split.
`FileSystem`, `Env`, and `Process` grant materially different authority, and a
single module invites a single coarse effect that would make signatures less
informative.

**Why not write these in Flux on top of existing primops?** `Flow.Path` can be
and should be. The rest cannot: there is no primop that lists a directory, so no
amount of Flux code produces one.

**Alternatives considered:**

- *A generic FFI instead of individual primops* — far more general, and would
  make this proposal unnecessary. Rejected as the near-term path because a safe
  FFI is a larger design problem than the capabilities it would unblock, and
  because it would let arbitrary native code bypass the effect system entirely.
  Worth revisiting as a successor.
- *Ship only what a package manager needs* — tempting, but the resulting surface
  would be shaped by one consumer and would need widening immediately.
- *Do nothing and write tooling in Rust* — the pragmatic choice for shipping a
  package manager quickly, and explicitly not the goal here: the point is to
  learn what Flux can express by making Flux express it.

## Prior art
[prior-art]: #prior-art

Most languages expose these capabilities as an unremarkable standard-library
layer, and the interesting question for Flux is not *what* to expose but *how to
type it*.

The design choices worth borrowing are: a structured error kind rather than
message strings (so callers can branch without parsing prose); an explicit
argument vector for subprocess execution rather than a shell string (the standard
mitigation for injection); path manipulation as a pure library rather than a
syscall wrapper; and separating "hash a byte string" from "hash a file" so large
inputs stream.

Where Flux departs from the norm is effect tracking. In most languages, reading
an environment variable is invisible in a signature. Making `Env`, `FileSystem`,
and `Process` distinct, visible capabilities means a Flux tool's type signatures
document its authority — a build tool whose manifest parser is provably pure is
a claim most ecosystems cannot make about their own tooling. Capability-safe
languages pursue this at the object-reference level; effect rows get much of the
same benefit with less ceremony.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

**Before acceptance:**

1. `Result` versus `Exn` for fallible I/O, and whether both should be available.
   This is the load-bearing decision.
2. Whether `Env` and `Process` warrant distinct effect labels or should fold into
   an existing one. (Proposed: distinct — they grant different authority.)
3. Windows path semantics for `Flow.Path`: separators, drive prefixes, and
   whether `normalize` is purely lexical.
4. ~~Whether `Flow.Process` belongs in this proposal at all, or should be
   deferred until a concrete need beyond git dependencies exists.~~
   **Resolved: included.** Implemented as specified — one primop, a distinct
   `Process` effect, an argument vector with no shell. Deferring it would have
   left the capability set without an escape hatch, which is what keeps a
   missing capability from being a blocking gap.

**During implementation:**

5. ~~Whether `Flow.Fs` operations should be async-aware — tracked as
   [KI-005](../../known_issues.md#ki-005-flowfs-is-not-async-aware).~~
   **Resolved:** `Flow.Fs` automatically uses the async blocking filesystem
   pool inside `Async.run_async`; synchronous behavior remains unchanged
   outside an async boundary.
6. Streaming reads for large files — tracked as
   [KI-007](../../known_issues.md#ki-007-no-streaming-reads-for-large-files).
7. Whether `Bytes` should be the I/O currency instead of `String` — tracked as
   [KI-006](../../known_issues.md#ki-006-io-uses-string-so-binary-data-cannot-round-trip).
8. **Found during implementation:** `Flow.Process` is POSIX-only on the native
   backend — tracked as
   [KI-004](../../known_issues.md#ki-004-native-subprocess-execution-is-posix-only).

Questions 5–8 outlived the implementation and moved to
[docs/known_issues.md](../../known_issues.md) so they stay visible after this
proposal is filed under `implemented/`.

**Out of scope:** a general FFI, file locking, symbolic links, permissions and
ownership, file watching, and terminal control.

## Future possibilities
[future-possibilities]: #future-possibilities

- **A Flux-written package manager** ([0177](0177_package_manager.md)) — the
  immediate consumer, and the one that would demonstrate the whole stack.
- **Self-hosted developer tooling**: formatter, linter, and documentation
  generator, all currently Rust-side.
- **Capability-secure execution** — once `FileSystem`, `Env`, and `Process` are
  distinct effects, a handler could restrict a subcomputation to a subtree of the
  filesystem or deny subprocess execution outright. That is a genuinely
  compelling story for running untrusted build scripts, and it is reachable only
  because these capabilities are effects rather than free functions.
- **A general FFI**, which would subsume the primop-per-operation approach.
- **`Bytes`-first I/O**, unifying the filesystem and network currencies.
