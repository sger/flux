# Known issues

Confirmed bugs, accepted limitations, and open design questions that outlive the
proposal or PR that raised them.

**Why this file exists.** Proposals go to `docs/proposals/implemented/` once they
ship, taking their unanswered "Unresolved questions" with them — 27 implemented
proposals currently carry 114 such questions, and nobody re-reads an implemented
proposal. Anything still live belongs here instead, where it is one file to scan.

## What goes here

- **Bugs** — reproducible defects not yet fixed. A reproduction is mandatory.
- **Limitations** — behaviour that is working as designed but will surprise
  someone, including backend divergences.
- **Questions** — design decisions deliberately deferred, where deferring has an
  ongoing cost (a compatibility commitment, a workaround people keep writing).

Not here: anything with an obvious fix (just fix it), speculative future work
(that is `docs/roadmaps/`), or a question with no consequence if it goes
unanswered for another year.

## Conventions

Each entry gets a stable `#KI-nnn` anchor so code comments and commit messages
can point at it (`// see docs/known_issues.md#ki-003`). Numbers are never
reused; resolved entries move to the bottom rather than being deleted, so a
stale reference still resolves to an explanation.

Every entry states **how it was verified and when** — an unverified issue report
ages into misinformation. Re-verify before acting on one.

Severity is about consequence, not effort:

| | |
|---|---|
| **High** | Silent wrong answers, or a workaround is required to proceed |
| **Medium** | Loud failure, or a limitation with a documented workaround |
| **Low** | Papercut, or a question with no present cost |

---

## Open

### KI-004 — Native subprocess execution is POSIX-only

**Severity:** Medium · **Area:** Native backend, `Flow.Process` · **Verified:** 2026-08-22 · **From:** [0178](proposals/implemented/0178_os_capabilities_for_tooling.md) Q8

`Flow.Process.run` spawns via `posix_spawnp` in the C runtime. The Windows
branch returns an `IoError` (`ENOSYS`) instead of spawning. The **VM** backend
works on Windows, since it goes through Rust's `std::process::Command`.

This is the only deliberate behavioural difference between the two backends in
the OS-capability surface, and it must close before Windows is a supported
target for Flux tooling.

### KI-006 — I/O uses `String`, so binary data cannot round-trip

**Severity:** Medium · **Area:** `Flow.Fs`, stdlib API surface · **From:** [0178](proposals/implemented/0178_os_capabilities_for_tooling.md) Q7

File I/O reads and writes `String`. `Flow.Http` already uses `Bytes` for
response bodies, so the stdlib is inconsistent — and a binary file cannot survive
a `read_file` / `write_file` round-trip.

This is a **compatibility commitment that hardens with age**: every program
written against the `String` API is one more thing to migrate. Worth deciding
before the surface has real users, not after.

### KI-007 — No streaming reads for large files

**Severity:** Low · **Area:** `Flow.Fs` · **From:** [0178](proposals/implemented/0178_os_capabilities_for_tooling.md) Q6

`Flow.Fs.read_file` returns a whole `String`, so a file's full contents must fit
in memory. `Crypto.sha256_file` streams internally and shows the shape a public
streaming API could take.

### KI-009 — TCP operations block the fiber scheduler

**Severity:** Medium · **Area:** Runtime, `Flow.Tcp` · **Tracked by:** [0174](proposals/0174_async_effect_concurrency.md)

TCP operations use blocking stdlib calls with no fiber-scheduler integration, so
concurrent TCP tests are not yet possible. Needs the mio reactor wiring.

### KI-011 — Re-wrapping `Err(e)` into a `Result` with a different success type fails inference

**Severity:** Medium · **Area:** HM inference · **Verified:** 2026-08-24

The standard error short-circuit — match a `Result`, pass `Err` through
unchanged, produce a different success type — does not infer:

```flux
fn inner() -> Result<Int, String> { Ok(1) }

fn outer() -> Result<Bool, String> {
    match inner() {
        Ok(_)  -> Ok(true),
        Err(e) -> Err(e),      // error[E430]: Could Not Infer Concrete Type
    }
}
```

The declared return type of `outer` fixes both parameters, so the `Err(e)` arm
should unify against `Result<Bool, String>`. Arm order makes no difference, and
annotating the re-wrapped value via an intermediate `let` does not help either.

**Workaround:** chain with `Flow.Result.and_then_result` / `map_result`, which
thread the error without an explicit re-wrap:

```flux
Result.and_then_result(step_one(), \x ->
    Result.map_result(step_two(), \y -> combine(x, y)))
```

**Why it matters:** this is the ordinary shape of any multi-step parser, so it is
hit constantly rather than rarely. `Flume.Version.parse` is written in the
combinator form for exactly this reason. The workaround reads acceptably for two
or three steps and poorly beyond that, which will press harder in
`Flume.Manifest`, where a TOML parser chains many more.

**A module can carry the defect invisibly (verified 2026-08-24).** For a
function in an *imported* module the check runs only under `flux --test`, so a
module full of `Err(e) -> Err(e)` compiles, runs, and produces correct output
under `flux run`, then fails to compile the moment a test fixture imports it:

```sh
$ flux run demo.flx            # imports Mini, which re-wraps Err — works
$ flux --test demo.flx         # error[E430] at Mini.flx, in the imported module
```

`--test` turns on strict mode, and `effective_module_strictness` exempts only
`Flow.*`; every other imported module is validated. `--strict` on a plain run
does *not* reproduce it, so strict mode alone is not the trigger — the test path
is. The practical consequence is that the workaround must be applied when the
code is written, because a green `flux run` is not evidence the module is clean.

Reconstructing the payload rather than forwarding it also avoids the error
(`Err(message) -> Err(message + "")` infers), which is further evidence the
failure is about the *forwarded binding* and not the surrounding types.

---
### KI-023 — `exposing` cannot rename, so two modules' same-named types cannot both be used

**Severity:** Low · **Area:** Module system · **Verified:** 2026-08-24

A type may be written in a signature only by its bare name: a qualified type
(`Manifest.Dep`) is a parse error in type position, so the only way to name an
imported type is `import M exposing (Dep)`. But `exposing` has no rename form,
so when two modules each export a type of the same name, a signature can name at
most one of them:

```flux
import Flume.Manifest exposing (Dep)   // Manifest's Dep
import Flume.Resolve exposing (Dep)    // shadows it — no way to name both
```

`Flume.Manifest.Dep` (a manifest entry) and `Flume.Resolve.Dep` (a solver
requirement) are exactly this case.

**Workaround:** keep the two types out of one module. `Flume.Plan` takes the
manifest's dependencies pre-split into `(name, requirement)` pairs rather than
taking `Manifest.Dep` values, so only the resolver's `Dep` is ever named there.
That is a reasonable shape on its own — the caller decides which dependency
kinds enter the build graph — but it was forced, not chosen.

**Why it matters:** it is a naming collision that cannot be worked around
locally; it has to be designed around at the module boundary. The cost grows
with the number of modules, since "some other module might export this name"
is not checkable from where the signature is written.

Two candidate fixes: allow `exposing (Dep as ManifestDep)`, or accept qualified
type names in type position. The second also removes the reason most `Flume`
modules carry a second `exposing` import beside their `as` alias.

### KI-025 — A stale bytecode cache can desync effect-operation symbols

**Severity:** Medium · **Area:** Bytecode cache, effect handlers · **Verified:** 2026-08-24

Effect operations are identified at runtime by interned `Identifier` symbols.
The cache stores raw symbol ids, which are meaningless in a fresh interner, so
`Compiler::remap_cached_constant_symbols` re-interns `PerformDescriptor` and
`HandlerDescriptor` constants by *name* when cached module bytecode is
hydrated.

When a cache entry survives from a **different compiler build**, that remap can
leave the `perform` site and the handler arms holding different symbols for the
same operation, and the handler silently stops matching:

```
error[E1009]: unhandled operation: Fail.fail
```

Instrumenting the arm lookup showed the mismatch directly — the perform site
carried `Symbol(9)` while the only handler arm carried `Symbol(10)`, for the
same name `fail`.

**Reproduced during compiler development**, by running a program against a
cache written by a previous build of `flux`. It does **not** reproduce from a
clean cache: with caches cleared, the same sources run correctly cold, warm,
after editing the entry file, and after adding an operation to the effect
declaration. `--no-cache` always works.

**Consequence.** Rare for users, but a real trap while working on the compiler:
the failure looks like a language limitation ("custom effects don't work across
modules") when it is a stale artifact. `CACHE_EPOCH` guards format changes, not
interner-layout changes.

**Workaround:** `--no-cache`, or clear the cache after changing the compiler.

**Note.** Custom module-declared effects *do* work: an effect declared inside a
module, imported with `import M as M`, and handled with `expr handle M { ... }`
resolves correctly, including the abort path where the handler never calls
`resume`. The handler must scope a call expression directly — wrapping it, as
in `Ok(f(x)) handle E { ... }`, does not install the handler over `f`.

### KI-032 — An unannotated wrapper over a row-polymorphic function does not infer

**Severity:** Medium · **Area:** Effect rows, inference · **Verified:** 2026-08-24

Making `Flow.List.map` / `filter` effect-row polymorphic
(`f: ((a) -> b with |e)`) let them accept effectful functions, but it broke
*unannotated* wrappers around them:

```flux
import Flow.List as L
fn map(xs, f) { L.map(xs, f) }    // error[E419]: Unresolved Effect Row
```

```
error[E419]: I cannot resolve the effect variable `e` introduced by this call.
  this call leaves an effect variable unconstrained
```

With no annotation on the wrapper there is nothing to fix `|e`, so the
obligation escapes unresolved. Two `vm_tests` cases regressed this way
(`test_list_comprehension_with_guard`, `test_list_comprehension_cons_list`);
they pass against the pre-change `Flow.List` and fail after it — 119/119
versus 117/119.

The two `vm_tests` cases turned out to carry the wrappers as vestigial
scaffolding — list comprehensions are lowered natively, not desugared to
`map`/`filter`/`flat_map` — so removing them restored 119/119. The underlying
inference limitation is unchanged.

**Workaround:** annotate the wrapper, propagating the row explicitly:

```flux
fn map<a, b>(xs: List<a>, f: ((a) -> b with |e)) -> List<b> with |e { L.map(xs, f) }
```

Verified working.

**The trade.** Row polymorphism is what allows an effectful reader to be mapped
over a list at all, which is why `Flow.List` now uses it; the cost is that
point-free wrappers over those functions must carry a signature. Whether
inference should instead default an otherwise-unconstrained row to empty is the
open question — that would restore the unannotated form without giving up the
polymorphism.

### KI-029 — A lambda cannot carry its enclosing function's effects

**Severity:** Medium · **Area:** Effect rows, closures · **Verified:** 2026-08-24

A lambda body is checked against the empty effect row, so calling an effectful
function inside one fails even when the enclosing function declares that
effect. Passing the same function *by reference* works:

```flux
fn check(n: Int) -> Int with Fail { ... }

// Works — direct reference, row propagates through `List.map`'s `|e`.
fn all_direct(xs: List<Int>) -> List<Int> with Fail { List.map(xs, check) }

// error[E400]: Call to `check` requires effect `Fail` in this function signature
fn all_lambda(xs: List<Int>) -> List<Int> with Fail { List.map(xs, \x -> check(x)) }
```

**Consequence.** Any effectful map/filter has to be expressed as a named
function rather than a lambda, so a call needing extra arguments cannot be
written inline — it needs a helper that takes them as parameters. This is the
main source of small named helpers in `Flume.Index` and `Flume.Lock`.

**Workaround:** pass a direct function reference, adding a named helper when
the lambda existed only to close over extra arguments.

### KI-030 — Non-tail recursion overflows the VM at a depth the native backend survives

**Severity:** Low · **Area:** VM, native backend, parity · **Verified:** 2026-08-24

Flux has no loops, so every iteration is recursion. Tail-recursive functions are
optimised on both backends, but *non-tail* recursion — the natural shape for
building a list, `[f(head) | recurse(rest)]` — consumes a stack frame per
element, and the two backends tolerate different depths:

| Form | Depth | VM | Native (`--native`) |
|---|---|---|---|
| non-tail | 300k | works | works |
| non-tail | 400k | **`E1009` stack overflow** | works |
| tail + accumulator | 200k | works | works |

The tail-recursive form survives every depth tested on both backends, including
through an effect row (`with Fail`), so tail-call optimisation is applied even
when the function performs effects.

**Consequence.** A program that builds a very long list by non-tail recursion
can pass a native build and fail under the VM. The boundary is high enough
(between 300k and 400k frames) that ordinary programs do not reach it.

**What actually binds.** Not an allocation failure: the VM stack already grows
on demand. `ensure_stack_capacity_with_headroom` (`src/vm/mod.rs`) resizes by
1.5x or `STACK_GROW_MIN_CHUNK` (4096 slots), whichever is larger, from an
`INITIAL_STACK_SIZE` of 2048. Overflow is the ceiling being reached:

```rust
const MAX_STACK_SIZE: usize = 1 << 20; // 1,048,576 slots
```

The observed boundary fits — roughly 350k frames at about 3 slots each. With
NaN-boxing that ceiling is about 8 MB.

**Prior art.** GHC has the same divergence in kind, and treats it as a constant
factor rather than a defect. GHCi and compiled code share one stack mechanism —
a heap-allocated `StgStack` grown in 32 KiB chunks by `threadStackOverflow`
(`rts/Threads.c:645-706`), bounded by `-K`, which defaults to 80% of physical
memory (`rts/RtsFlags.c:139-150`); 8 MB is only its fallback when memory
detection fails. There is no interpreter-specific limit; a program can still
overflow in GHCi and not when compiled, because bytecode frames are fatter and
unoptimised, so the same depth consumes more of the same budget. Flux's VM/native
gap has the same character.

GHC's chunk *chain*, linked by `UNDERFLOW_FRAME`, exists because a live
`StgStack` cannot be moved once heap pointers refer to it. The VM's stack is a
`Vec` that can be reallocated wholesale, so contiguous growth is both simpler
and sufficient; there is nothing to copy there.

**If the limit ever binds** the change is to `MAX_STACK_SIZE`, not to frame
management — and the part of GHC's design worth taking is that `-K` is a *flag*.
Ours is a hard-coded constant, so the policy is not the user's to set. Raising
it trades a diagnostic with a stack trace for OOM-killer behaviour, which is why
a ceiling is deliberate rather than a limitation.

**Guidance.** For any list whose length is driven by input rather than by a
fixed small bound, use the accumulator form and reverse at the end:

```flux
fn all(xs: List<a>, acc: List<b>) -> List<b> {
    match xs {
        [] -> List.reverse(acc),
        [h | t] -> all(t, [f(h) | acc]),
    }
}
```

Flume's own recursions (dependency lists, lockfile entries, index lines) are
far below the limit and are left in the direct form, which reads better.

**Correction.** This entry previously reported the VM overflowing between 80k
and 120k frames. That was measured with a harness whose `main` never ran — the
test file wrapped `main` in a named module, so the program produced no output
at any depth and every run was read as a failure. Re-measured with a top-level
`main`, the VM reaches 300k. The severity is lowered from Medium accordingly.

### KI-027 — Match on a tuple of two `Result`s is reported non-exhaustive

**Severity:** Low · **Area:** Exhaustiveness checking · **Verified:** 2026-08-24

```flux
match (ra, rb) {
    (Ok(a), Ok(b)) -> Ok(a + b),
    (Err(e), _)    -> Err(e),
    (_, Err(e))    -> Err(e),
}
```

```
error[E015]: Non-Exhaustive Match — 1 missing pattern
```

The three arms do cover every inhabitant: `(Ok, Ok)`, then any `Err` in either
position. The checker does not see that the wildcards close the space.

**Workaround:** nest the matches, or express the combination with
`and_then_result` / `map_result`.

### KI-015 — A class whose variable appears only in the return position cannot dispatch

**Severity:** Medium · **Area:** Type classes / dispatch · **Verified:** 2026-08-23

Dispatch selects an instance from the *first argument's* type. A class whose
type variable appears only in the return position therefore resolves against the
parameter type and fails:

```flux
class Parse<a> { fn from_text(text: String) -> Result<a, String> }
instance Parse<Int> { fn from_text(text) { Ok(len(text)) } }

fn read_int(t: String) -> Result<Int, String> { from_text(t) }
// error[E444]: No instance for `Parse<String>`   ← the parameter type, not `Int`
```

`Flow.Json`'s `Decode` works only because `try_resolve_class_call` special-cases
it by name (`src/core/lower_ast/mod.rs`), selecting from the inferred result
type. A general return-type-directed rule would subsume that special case.

**Workaround:** give the class an argument mentioning the type variable, or
reify it as a value — `Flume.Manifest` uses a `Reader<a>` record, which also
composes further than an instance head can (`array_of(element: Reader<a>) ->
Reader<List<a>>` needs no higher-kinded types).

---

### KI-035 — `Flow.Http` has no TLS, so no HTTPS host is reachable

**Severity:** High · **Area:** `Flow.Http`, runtime · **Verified:** 2026-08-25 · **From:** [0177](proposals/implemented/0177_package_manager.md) Phase 2 fetching

`parse_url` ([../src/runtime/http/mod.rs:332](../src/runtime/http/mod.rs#L332))
rejects any URL that does not begin with `http://`:

```
error[E1009]: AsyncError: ProtocolError(0, "only http:// URLs are supported in this phase")
```

Plain HTTP works end to end — `Http.get("http://example.com")` returns
`status=200` with a 559-byte body — so this is a missing TLS layer, not a
broken client. There is no `https://` support anywhere in the runtime.

The consequence is that **no real code-hosting or package host is reachable**:
GitHub, GitLab, and crates.io-style registries are all HTTPS-only and either
redirect or refuse plain HTTP. Anything that must fetch over the network has to
shell out to a program that brings its own TLS.

This is why [0177](proposals/implemented/0177_package_manager.md)'s git dependencies fetch
by invoking `git` through `Flow.Process` rather than over `Flow.Http`: the
system `git` binary supplies TLS. The same restriction blocks a future
HTTP-based registry client, which cannot be built until TLS lands.

Two adjacent gaps matter to any future fetcher: `Response.body` is `Bytes` but
I/O is `String`-based ([KI-006](#ki-006)), so an archive cannot round-trip
through the filesystem; and `Flow.Async.run_async` discharges `AsyncFail` only
for a named function, not for a lambda literal.

---

### KI-036 — Linked modules used incompatible effect identities

**Status:** Fixed · **Area:** effects, module linker · **Verified:** 2026-08-25 · **From:** [0177](proposals/implemented/0177_package_manager.md) `flux update`

The VM linker originally preserved the compiler-local numeric identifiers in
`HandlerDescriptor` and `PerformDescriptor` constants. Each module has its own
symbol interner, so a `Console` or `Fail` operation compiled in a dependency
could not match the handler installed by the entry module. The runtime then
reported:

```
error[E1009]: unhandled effect: Console (no matching handle block)
```

This was exposed by `flux update`: a cold file-backed git fetch performs
`Console` several calls below `Flume.Build.Graph.update`. The handler was
present; only its numeric identity differed. The linker now interns descriptor
effect and operation names in the shared linked-module interner before the VM
executes the program. A module-linker regression test covers mismatched local
identifiers, and the end-to-end update path moves a dependency pin from v1 to
v2 successfully.

The package CLI also decodes quoted progress lines before reading the final
`ok`/`err` record, so fetch progress does not corrupt the update reply.

---

### KI-037 — `--no-cache` loads modules under bare names, colliding with local types

**Severity:** Medium · **Area:** module graph, caching · **Verified:** 2026-08-25

`flux --no-cache <file>` resolves the Flume module graph to 34 modules named
`Cli`, `Roots`, `Plan`; the ordinary cached path resolves 36 named `Flume.Cli`,
`Flume.Roots`, `Flume.Plan`. Under the bare naming, `Flume.Plan`'s public
`Outcome` collides with the `Outcome` that `Flume.Cli` declares for itself, and
every match on the local type reports spurious missing patterns:

```
error[E015]: Non-Exhaustive Match
Match is not exhaustive: 2 missing patterns.
  lib/Flume/Cli.flx:35:9
   |
35 |         match o { Outcome { failed, message: _ } -> failed }
```

`lib/Flume/Cli.flx` therefore fails to compile under `--no-cache` while
building cleanly without it. Reproduced on `b839e8b5` with no local changes, so
it predates the `flux update` work that surfaced it.

The practical trap is that `--no-cache` is the flag reached for when a cache
problem is suspected, and here it manufactures errors that do not otherwise
exist — a misleading signal exactly when the cache is under suspicion.

---

## Resolved

### KI-005 — `Flow.Fs` is async-aware — FIXED 2026-08-26

`Flow.Fs` now routes reads, predicates, mutations, directory listing, and
metadata through the configured blocking filesystem pool while running inside
`Async.run_async`. Calls outside an async boundary retain their synchronous
behavior. Cancellation suppresses completion delivery while allowing the
underlying OS operation to finish safely.

Verified with the VM/native `tests/parity/fs_async.flx` fixture, synchronous
filesystem regression tests, native async runtime tests, and the configured
filesystem-pool path. See [`changes/2026-08-26-flow-fs-async.md`](../changes/2026-08-26-flow-fs-async.md).

### KI-022 — Forwarding an imported constructor's payload failed strict types — FIXED 2026-08-25

Matching a value whose constructor comes from another module and returning the
bound payload unchanged failed to infer, but only when strict types run — which
for an imported module means only under `flux --test`:

```flux
// Mini.flx
import Flume.Value exposing (Toml, TString)

module Mini {
    public fn unwrap(item: Toml) -> Result<String, String> {
        match item {
            TString(text) -> Ok(text),   // error[E430] under `flux --test`
            _ -> Err("not a string"),
        }
    }
}
```

**Root cause.** [KI-014](#ki-014) gave `ModuleInterface` a `public_ctor_types`
map so an imported constructor infers as its ADT, but only
`preload_module_interface` — the *cached* `.flxi` path — ever populated it. Two
paths that compile a dependency fresh in the same run did not:
`preload_dependency_program` collected field *names* but no field types, and the
test runner compiles the whole module graph through one `Compiler` with no
interface step at all. So `TString` was absent from `adt_constructor_types`, its
pattern bound `text` to an unresolved variable, the enclosing `Ok(...)` never
got an argument type, and strict types rejected the residue.

The original report blamed the declared return type for not fixing it. The
return type *was* fixed; the unresolved half was the argument, arriving from the
pattern side. That is also why reconstructing the payload (`Ok(text + "")`)
worked as a workaround — it supplies the constraint the pattern failed to.

**Fix.** `Compiler::preload_ctor_types_from_program` seeds the constructor field
types of a dependency's public ADTs straight from its in-session AST, called
from both fresh-compile paths. `CACHE_EPOCH` bumped 22 → 23. Regression test:
`tests/type_inference/imported_ctor_payload_forwarding_tests.rs`, covering the
positional, generic, and named-field constructor shapes — each forwarding its
payload unchanged, since a fixture that reconstructs passes against the unfixed
compiler.

### KI-034 — A `perform` in another module corrupted every native caller — FIXED 2026-08-25

A user-defined effect performed in one module and handled in another crashed
the native backend with SIGSEGV (or SIGBUS, depending where the bad value
landed). The VM was always correct, so this was also a parity divergence.

```flux
// Effectful.flx — the effect and the perform
module Effectful {
    effect MyFail { boom: String -> Unit }
    public fn abort_it(msg: String) -> String with MyFail {
        perform MyFail.boom(msg)
        ""
    }
}

// main.flx — the handler
fn parse_it(s: String) -> String with MyFail { Effectful.abort_it("bad") }
fn go(line: String) -> Result<Int, String> {
    Ok(from_line(line)) handle MyFail { boom(resume, m) -> Err(m) }
}
```

The same code with the effect and the handler in **one** module always worked —
the module boundary was the trigger.

**Root cause.** A yield unwinds by returning `FLUX_YIELD_SENTINEL` (the raw
value `10`) up the stack, so every caller must test `flux_is_yielding` after a
call that can suspend and propagate instead of using the result. Three separate
places decided that per module, and each was blind across the boundary:

1. `effect_expr_contains_async` (`src/lir/lower.rs`) marked a function's
   indirect call sites `async_capable` only when its row named `Async`. A
   user-defined effect left every caller marked `async_capable: false`.
2. `is_direct_async_extern_symbol` (`src/lir/mod.rs`) answered "can this
   cross-module call suspend?" from a hardcoded 55-entry allowlist of
   `Flow_Async_*` / `Flow_Task_*` / `Flow_Channel_*` symbols. No user effect
   could ever match. `direct_async_func_ids` computes a transitive fixpoint,
   but only over `LirFuncId`s, which are unique within one module.
3. `program_has_yield_sites` (`src/lir/emit_llvm.rs`) enabled yield checks for
   a module only if it contained a `YieldTo` of its own, i.e. a literal
   `perform`. A module that merely *called* a suspending import got none.

So the caller ran straight on with the sentinel in hand. In the reported case
`Flume.Index.from_line` passed it through `field_string` into `Version.parse`
into `trim`, where `flux_string_len(10)` dereferenced address `0xe`.

**Fix.** Make "can suspend" a property that travels with the import.
`ImportedNativeSymbol` gains `can_suspend`, derived from the declared effect row
(`Compiler::native_scheme_can_suspend`); `LirProgram` carries the resulting
`suspending_extern_symbols`; and all three sites consult it. The name-based
allowlist stays as a fallback for stdlib async symbols, which reach lowering
without a scheme.

The rule matches the one lowering already used for local `perform`: every
effect suspends except `Console`, `FileSystem`, `Stdin`, and `Clock`, which
lower to plain C calls that always return a real value. An open row (one with a
tail variable) counts as suspending — over-reporting costs a redundant
`flux_is_yielding` test, under-reporting corrupts memory.

`CACHE_EPOCH` bumped 23 → 24: cached native objects lack the new checks.
Regression test: `tests/native_llvm/native_cross_module_effect_tests.rs`.

**Found by** bisecting a SIGSEGV in `flume_index_tests`, which surfaced after
the qualified-resolution fix removed the SIGBUS that had been masking it.

### KI-033 — A deep stack trace printed one line per frame — FIXED 2026-08-24

A runtime error raised deep in a recursion rendered every frame it unwound. A
stack overflow at ~350k frames produced **11.7 MB** of output, almost all of it
the identical line `at build (Deep.flx:2:29)`, burying the error message and the
source snippet above it.

**Root cause.** `render_stack_trace` looped over the whole frame vector with no
cap (`src/diagnostics/rendering/renderer.rs`), and the driver formatted a
`String` per frame before that (`src/driver/reporting/runtime_errors.rs`).

**Fix.** The renderer keeps the first 20 frames and the last 10 — the raise site
and the entry point, which are the informative ends — and replaces the middle
with `... N more frames ...`. The 11.7 MB case now renders in 1,764 bytes.

GHC caps the same way, and for the stated reason: `MAX_DEPTH = 10` guards `-xc`
CAF chains with the comment *"don't print gigantic chains of stacks"*
(`rts/Profiling.c:934`), and libdw unwinding stops at 5000 frames
(`rts/Libdw.c:19`). Both truncate silently; reporting the elided count is a
small improvement on that.

**Found by** re-measuring KI-030, whose depth table was itself wrong.

### KI-028 — An imported effect was invisible under `flux --test` — FIXED 2026-08-24

A module that imported an effect from another module compiled and ran
correctly, then failed to compile the moment a test fixture imported it: every
`with <Effect>` annotation raised `E407` and every `handle` block `E405`.

**Root cause.** `collect_effect_declarations` resets `effect_ops_registry` from
`preloaded_effect_ops_registry` at the start of each compile. The run drivers
populate that preload by walking each dependency's AST
(`preload_dependency_program`), but the test runner compiles every module
through a single `Compiler` with no preload step — so a module's own effect
declarations were discarded as soon as the next module began compiling. A
`.flxi` interface does not close the gap: it records the effect *rows* on
signatures, not the effect *declarations*.

Instrumenting the registry showed the effect being registered while its own
module compiled and absent immediately afterwards.

**Fix.** `Compiler::promote_effect_declarations` copies the compiled module's
effect registry into the preloaded set, and `run_tests.rs` calls it after each
module. This is the same promotion the REPL already performed between lines
(previously gated on `repl_mode`), for the same reason.

**Consequence of the fix.** A shared effect module now works, so `Flume.Lock`,
`Index`, `Plan`, `Home`, and `Roots` all import `Flume.Fail` instead of each
declaring a private copy — about 20 duplicated lines removed per module.

### KI-031 — Native tests gated on `native` instead of `llvm` fail under `--features native` — FIXED 2026-08-24

`Cargo.toml` declares `llvm = ["native"]`, so `llvm` implies `native` but not the
reverse. Two test files that spawn `flux --native` gated themselves on
`#![cfg(feature = "native")]`, so `cargo test --features native` compiled them
and then failed at runtime with the driver's own refusal:

```
Error: native backend features require `llvm`.
```

Four tests in `backend_representation_runtime_tests.rs` and two in
`non_zero_type_tests.rs` were affected. Every other native-running test in the
tree already gates on `llvm`.

**Fix.** Both files now use `#![cfg(feature = "llvm")]`, with a comment
recording why. Under `--features native` they compile out (0 tests); under
`--features llvm` all six pass.

### KI-026 — `Flow.Result` had no applicative or traversal combinators — FIXED 2026-08-24

`Flow.Result` exposed `map_result`, `map_err_result`, `and_then_result`, and
`or_else_result` — functor and monad, but no applicative and no traversal.
Combining several independent fallible reads into one record could only be
written as a nesting of `and_then_result`, one level per field, and every
"read many, fail once" loop was a hand-written `collect` (three near-identical
copies existed across `Flume.Lock`, `Flume.Index`, and `Flume.Plan`).

**Fix.** Added `map2`, `map3`, `apply`, `sequence`, and `traverse`. All are
defined in terms of the existing combinators and short-circuit on the first
`Err`. `traverse` is written directly rather than as `sequence(List.map(..))`
because `Flow.Result` is a stdlib base module and does not import `Flow.List`.

**Related fix.** `Flow.List.map` and `Flow.List.filter` were monomorphic in
their effect row (`f: (a) -> b`), so they rejected an effectful function with
`E300 Parameter Type Mismatch`. They are now effect-row polymorphic
(`f: ((a) -> b with |e)`), which is what lets a `Fail`-carrying reader be
mapped over a list. The language already supported row polymorphism; the
stdlib simply was not using it. `CACHE_EPOCH` moved to 22, since both stdlib
interfaces changed.

### KI-024 — The resolved-roots cache ignored `flux.lock` — FIXED 2026-08-24

The Phase 1 roots cache fingerprinted every manifest in the dependency graph,
which was complete while path dependencies were the only kind. Once registry
dependencies were resolved through the lockfile, the lockfile became an input
the cache did not track: deleting `flux.lock`, editing it, or publishing a new
version left the previous resolution in place, so a build silently used a
version the lockfile no longer named.

**Reproduction (before the fix).** With `json = "^1.0"` in `flux.toml` and
`1.2.0` locked, publish `1.5.0` to the index and delete `flux.lock`:

```sh
$ rm flux.lock && flux run     # still built json 1.2.0, and wrote no lockfile
```

**Fix.** `write_cached_roots` records a `lock<TAB><hash|absent>` line and
`read_cached_roots` re-checks it. The absent marker matters as much as the
hash: a resolution made without a lockfile must not be replayed once one
appears, and one made with a lockfile must not survive its deletion.
`CACHE_EPOCH` moved to 21, because entries written before the fix carry no
`lock` line and would otherwise keep validating.

Verified in both directions: deleting the lock re-resolves to the newest
matching version and writes a new lock; restoring a lock pinning an older
version rebuilds against that version.



Entries move here with the resolving commit rather than being deleted, so
existing `#KI-nnn` references still explain themselves.

### KI-020 — `flux test` only runs the entry file's tests — FIXED 2026-08-24

`collect_test_functions` matched only a bare `test_*` name or the special-cased
`Tests.test_*` module, so a test declared in any other module was never
discovered even though it was compiled and present in the symbol table. A
package could add whole modules of tests and see a green summary that never
touched them.

**Fix.** Discovery now matches on the *last* dot-separated segment, so
`test_parses`, `Tests.test_parses`, and `Json.Parse.test_parses` are all found,
and every compiled module in the graph contributes. Results are reported by
qualified name. The check is deliberately on the function name rather than the
path: a module named `test_utils` does not make all its members tests.

`--test-filter` matches against the qualified name, so it can now select a
whole module's tests as well as a single function.

### KI-021 — `flux test` does not see path dependencies — FIXED 2026-08-24

The test path resolved modules with unscoped roots: `run_tests.rs` called
`collect_roots` where the run path called `collect_module_roots`, so a
project's package roots never reached it. A package that built and ran failed
to compile under `flux test` with `E012 Unknown Module Member`.

**Fix.** `load_test_file` now resolves the cache layout and calls
`collect_module_roots`, and the graph is built with
`build_with_entry_and_module_roots`, matching the run path. The native test
backend forwards only the user's explicit `--root` flags to its subprocess,
which resolves the manifest itself — forwarding the resolved package roots
would have re-added them unscoped, defeating the namespace rule.

Script mode and `--root` under `--test` are unaffected, and the native
`--test --native` path was verified against a package with a path dependency.

### KI-019 — Some CLI commands exit 0 after failing — FIXED 2026-08-24

`run_command` ends with an unconditional `ExitCode::SUCCESS`, so a command that
printed an error still exited `0` unless its own arm exited first. Scripts and
CI could not detect the failure.

**Fix.** The run and `fmt` paths exit non-zero where they report the error,
matching the `std::process::exit(1)` convention already used across the driver.
The cache commands instead *return* a success flag that the CLI turns into an
exit code: they are called directly from unit tests, and exiting in-process
would kill the test harness. Nine command paths were affected or unguarded:

- the implicit `flux <file.flx>` run path and `flux eval`, whose shared
  frontend error arm printed and fell through;
- `fmt`, on both an unreadable and an unwritable file;
- `cache-info`, which returned early on a missing file;
- `module-cache-info` and `native-cache-info`, which had no existence check at
  all and reported an empty cache for a path that did not exist.

Success paths still exit `0`; `tokens`, `bytecode`, `lint`, `interface-info`,
compile errors, and failing `--test` runs were already correct.

Note `flux <file>.flx` for a program evaluating to `None` is *not* this bug:
`[1, 2, 3][99]` is `None` by design since `Array.get` began returning `Option`
(cache epoch 15), so exiting `0` is correct there.

### KI-013 — `List.map` over a list of tuples yields a null element natively — FIXED 2026-08-24

The minimal TOML reproduction now renders identically on the VM and native
backends:

```text
{a={x="1"},b={y={p="2"},z="3"}}
```

**Root cause.** `Flume.Document.assoc_set` reconstructed a list cell from
pattern-extracted `candidate` and `existing` fields. Aether treated those
fields as borrowed views but allowed them to enter owning tuple/list
constructors without a `Dup`. Its reuse/drop path then invalidated the nested
table list, leaving a null list element. `List.map` was only where the damaged
value became observable.

**Fix.** Pattern fields are duplicated when transferred into owning collection
constructors, Aether-only ownership nodes survive CFG lowering, and tuple
matching applies the existing pointer/sentinel guard before extraction. The
Flume TOML and manifest fixtures now run through VM/native parity assertions.

See the [native-backend debugging guide](debugging-native-backend.md), which
uses KI-013 as a worked example.

### KI-018 — Recursive rebuild mutates a list still held by the caller — FIXED 2026-08-24

The native backend now preserves the caller's list when a recursive rebuild is
performed through a borrowed argument. The regression is covered by
`tests/flux/aether_collection_ownership.flx`, including independent rebuilds
from the same source list.

**Root cause.** A borrowed call argument does not add an owning reference. A
callee containing `DropSpecialized` could nevertheless observe RC==1 and take
its unique reuse arm, even though the caller still retained the value (often as
a field of another list cell). The callee then mutated that shared cell in
place. The existing `flux_rc_is_unique` check was correct; the ownership
information at the call boundary was incomplete.

**Fix.** Aether now marks borrowed call arguments whose caller-side binder or
scrutinee remains live, following chained field aliases and conservatively
guarding non-variable expressions. Native lowering emits a temporary `Dup` and
matching `Drop` only for those mask entries, so the callee sees the true shared
reference count without disabling reuse for genuinely linear borrowed calls.
`--dump-aether` reports the planner-level `Reuses`/`FBIP` separately from the
guard count and debug locations.

### KI-003 — A class-constrained stdlib function accepts the wrong container type — FIXED 2026-08-23

Re-diagnosed and fixed 2026-08-23. The original report — "the bare `contains`
builtin returns false on primop-returned arrays" — was accurate as a symptom but
wrong about the cause, and understated the scope. There is **no bare `contains`
builtin**: `Flow.List` is auto-exposed by the prelude, so bare `contains` is
`List.contains`, and it returned `false` on an array because the array holds no
cons cells. Array provenance was irrelevant — an array *literal* failed the same
way, so `Fs.list_dir` was never implicated.

The real defect was that the call type-checked at all. `List.contains` is
declared `(List<a>, a) -> Bool`, so passing an `Array` must be an E300:

```flux
let arr = [|1, 2|]
contains(arr, 1)         // was: false   → now: E300
not_elem(arr, 1)         // was: true    → now: E300  (wrongly said "absent")
nub(arr)                 // was: []      → now: E300
contains(42, "x")        // was: false   → now: E300  (not even a container)
```

`nub` and `not_elem` are `Eq`-constrained and exist only in `Flow.List`, so this
was never a `Flow.List` / `Flow.Array` name collision — the common factor was a
**class constraint on the element type**.

**Root cause.** `infer_call_fixed_arity_path` emitted its argument-mismatch
diagnostic only when *both* the expected and actual types were
`is_concrete()`. Unification genuinely failed (`List<Var>` vs `Array<Int>`), but
an `Eq`-constrained element type leaves a free variable in the expected type, so
`is_concrete()` was false and the failure was silently discarded. The guard was
added for numeric defaulting, where a not-yet-defaulted `Num` variable can look
transiently mismatched.

**Fix.** `InferType::heads_conflict` reports a definitively incompatible pair of
outermost type constructors, which stays decidable while free variables remain —
no substitution turns `Array<Int>` into `List<a>`. The diagnostic now fires when
both types are concrete *or* their heads conflict. Two guards keep the original
suppression intact where it was load-bearing:

- the callee must have no definition span, i.e. an already-generalized imported
  scheme — a local function may still be having its own parameter types
  inferred, so its provisional head is not yet fixed;
- the argument type must be concrete, so an approximation is never reported as
  the offending value;
- the conflict is tested against the **unsubstituted** parameter type, so the
  head must be written in the signature. `List<a>` in `nub`'s signature cannot
  move; the `a` in `assert_eq<a>(a: a, b: a)` has no written head, and
  substitution may have filled it from an approximation.

All three guards were found by regression rather than by design, each from a
different false positive: unannotated local functions, `List.first` piped into
`upper`, and `assert_eq(List.first(xs), 1)`. The common source of the last two is
that `List.first` returns `h` *or* `None` — a mixed return the stdlib
deliberately leaves unannotated (`lib/Flow/List.flx:225`) — so inference
approximates it as `Option<_>` while the runtime value is a bare `a`. Code that
relies on this is ill-typed but works, and must not start failing to compile.

Verified: the four cases above now error; numeric defaulting, float/string
element types, every `examples/guide/` program, and all 51 `tests/flux/`
fixtures still behave as before.

### KI-002 — `println` prints nothing for cons lists and arrays — NOT REPRODUCIBLE 2026-08-23

Retested 2026-08-23 on both backends. `println` prints lists and arrays
correctly, including arrays returned from a primop (`Fs.list_dir`):

```
[1, 2, 3]
[|1, 2, 3|]
[]
["a", "b"]
```

VM and native agree. No fix was made and no defect was found; the entry is kept
so the `KI-002` reference still resolves.

The original report most likely came from a filtered terminal transcript: a
list prints as `[1, 2, 3]`, and a `grep -v '^\['` intended to drop the
compiler's `[ 1 of 12] Compiling` progress lines removes every list output
line too. `to_string(xs)` appeared to "work" because its output is quoted
(`"[1, 2, 3]"`) and so survives that filter. Verified against raw stdout bytes
rather than filtered output.

### KI-008 — The stdlib is found via a CWD-relative `lib/Flow` — FIXED 2026-08-23

**Severity:** High · **Area:** Driver, tooling · **Verified:** 2026-08-23 · **From:** [0177](proposals/implemented/0177_package_manager.md)

`inject_flow_prelude` resolved the stdlib as the bare relative path `lib/Flow`
against the process CWD and **returned silently** when it was missing;
`collect_roots` did the same for `src` and `lib`. Running Flux from anywhere
but this checkout produced an empty module root list:

```
Looked for module `Flow.List` under roots:  (imported from uses_stdlib.flx).
```

— no diagnosis, just nothing found. That blocked installing Flux as a tool or
shipping a binary.

Resolution is now `find_flow_dir` in `src/driver/frontend.rs`, tried in order:

1. `$FLUX_LIB_DIR/Flow` — explicit override.
2. `lib/Flow` walking up from the **entry file** — a project checkout, and the
   workspace case where the prelude sits above the inner crate.
3. `lib/Flow` walking up from the **executable** — an installed
   `<prefix>/bin/flux` with `<prefix>/lib/Flow`, and a dev binary run from
   `target/debug`.

`collect_roots` also resolves `src`/`lib` relative to the entry file (keeping
the CWD-relative pair, so invocations from a project root still work) and adds
the stdlib's parent as a root.

Verified end to end: a binary copied to `<prefix>/bin/flux` with the stdlib at
`<prefix>/lib/Flow`, invoked from an unrelated directory with no environment
variable, runs a program that imports `Flow.List`. Regression tests:
`tests/integration/stdlib_discovery_tests.rs`.

---

### KI-010 — The test suite is flaky under load: tests share one on-disk cache — FIXED 2026-08-23

**Severity:** High · **Area:** Test harness · **Verified:** 2026-08-23

`cargo test --all --all-features` intermittently failed targets that passed in
isolation. The failures moved between runs and looked unrelated, which made them
read as separate bugs. They were one bug: test binaries drove the `flux` CLI
against a shared `target/test-scratch/` and the shared compilation cache under
`target/`, so concurrent targets read and wrote each other's `.flxi` interfaces
and bytecode.

Two symptoms, both from that cause: a compiler assertion escaping as
`parallel VM compilation failed: missing global mapping for local index N`, and
a native fixture producing no output so the harness could not parse a summary
(`no native summary for <fixture>`).

`--no-cache` was not sufficient. The native backend writes shared artifacts
under the cache root regardless of it, which is why `*_native_tests` lost these
races most often despite passing the flag; a private `--cache-dir` is what
actually isolates. Verified: with `--cache-dir`, a native run left the shared
`target/flux/native` tree untouched and wrote its 27 artifacts to the private
root instead.

**Fixed** by routing every test that spawns the `flux` binary through the
`Scratch` guard in `tests/support/scratch.rs`, which gives each run its own
directory *and* its own cache root, and removes the directory on drop. All 44
such targets are now isolated — the three shared support runners
(`flux_runner.rs`, `primop_parity.rs`, `semantic_runtime.rs`) cover 26 of them,
and the rest were converted individually. `tests/aether/cli_snapshots.rs`
already had an equivalent private-cache scheme of its own.

**Note for new tests.** A test that spawns `flux` must pass
`Scratch::cache_args()`. `--no-cache` alone does not isolate it.

There is a separate toolchain pitfall: `CARGO_BIN_EXE_flux` points at the
shared `target/debug/flux` path. A plain `cargo build` can replace an
LLVM-enabled binary with one built without `llvm`, causing native tests to fail
before execution with `native backend features require llvm`. Always run native
tests through the same command that builds their binary, for example:

```text
cargo test --features llvm --test native_json_tests
```

This is a stale-binary problem, not a JSON backend failure.

---

### KI-016 — An exported constructor field type kept its transparent alias — FIXED 2026-08-23

**Severity:** High · **Area:** Module interfaces / HM inference · **Verified:** 2026-08-23

A `public data` field declared with a transparent alias was exported with the
alias unexpanded, so an importing module saw the alias where inference had
produced the underlying type:

```flux
import Flow.Http as Http
print("x" + Http.ok("hi").body)  // error[E300]: String and Bytes
```

`Bytes` is `public alias Bytes = String` in `lib/Flow/Http.flx`, so this
compared a type against itself. It made `Flow.Http`'s response API unusable
from any other module, and failed three `native_http_client_tests`.

Aliases are expanded syntactically in the declaring program, before inference.
Exported *schemes* are therefore already structural, but the `public_ctor_types`
field metadata added for [KI-014](#ki-014) is collected from the **raw AST**,
which still names the alias — so the two disagreed. Introduced with that field;
before it, no unexpanded type crossed the boundary.

Fixed by expanding aliases in the exported field types at interface-build time.
`public_type_aliases` is also recorded so that changing an alias body
invalidates importers whose field types were expanded through it; it is written
for the fingerprint rather than read back. Covered by the
`imported_constructor_types` fixture.

---

### KI-014 — A constructor imported from another module infers as a type variable — FIXED 2026-08-23

**Severity:** High · **Area:** HM inference / module interfaces · **Verified:** 2026-08-23

A constructor applied in a module other than the one declaring its ADT inferred
as an unresolved type variable rather than the ADT type, so anything keyed on
that type failed. Class dispatch was the visible casualty:

```flux
import Flume.Value exposing (Toml, TString)
render(TString("hi"))  // error[E1009] panic: No instance of Render.render ...
```

Constructor applications route through `adt_constructor_types` in
`infer_call_expression`, and that map was populated only from *local* `data`
statements (`register_data_constructors`). An imported constructor missed the
lookup, fell through to the ordinary function-call path, and produced a fresh
type variable. Dispatch, which keys on the argument's type, then had nothing to
select on.

The metadata to fix it was simply absent: `ModuleInterface` carried
`ctor_field_names` — field *names*, added for the named-field fix — but no field
types and no owning ADT, which is why the named-field path could be fixed
earlier while the positional path could not. `ModuleInterface` now also carries
`public_ctor_types`, which seeds inference on import.

Two details in the original report were wrong: the symbol it named,
`preloaded_adt_constructor_types`, does not exist (the map is
`adt_constructor_types`), and the failure was a routing-guard miss rather than a
constructor scheme that failed to concretise.

Fixed by adding `public_ctor_types` to `ModuleInterface` (fingerprinted, so a
changed field type invalidates importers) and seeding imported ADTs into
inference; `CACHE_EPOCH` bumped 18 → 19. Regression test:
`tests/type_inference/imported_constructor_types_tests.rs`.

---

### KI-001 — A `let` read inside a `match` arm is `Uninit` after the match — FIXED 2026-08-23

**Severity:** High · **Area:** VM backend, bytecode compiler · **Verified:** 2026-08-23

On the VM backend, a `let` binding read inside a statement-position `match` arm
was the VM's `Uninit` sentinel for the rest of the function — correct inside the
arm, corrupt after it. `Int` printed `<uninit>` with no diagnostic; `String`
aborted with `E1009: Cannot add Uninit and String values`. The LLVM backend was
always correct, so this was also a parity divergence.

```flux
fn main() with IO {
    let n = 42
    match Some(1) {
        Some(_) -> println(to_string(n)),   // "42"       — correct
        None -> println("none"),
    }
    println(to_string(n))                   // "<uninit>"  — no error
}
```

**Root cause.** The VM has two opcodes for reading a local: `OpGetLocal` copies,
and `OpConsumeLocal` *moves* — `stack_take` replaces the slot with `Uninit` so
`Rc::try_unwrap` can succeed downstream without a clone. The compiler picks the
move when a binding's use count is exactly 1.

`compile_match` (`src/compiler/expression.rs`) merged each arm body's use counts
into the enclosing map with `or_insert`. For the arm's *own* pattern bindings
that is right — the arm body is their whole lifetime. For a binding declared
outside, it is not: the arm is one branch of the function, and `or_insert`
silently installed the arm-local count of 1 as the whole-function count whenever
the enclosing map had no entry for that symbol. The read after the match then
found an emptied slot.

**Fix.** Merge a symbol's arm-body count only when the symbol belongs to the
arm's own scope (`exists_in_current_scope`, which is accurate at that point
because `enter_block_scope` and `compile_pattern_bind` have already run). Arm
bindings still compile to `OpConsumeLocal`, so the optimisation is narrowed
rather than disabled.

**A sharper trigger than originally recorded.** The bug reproduced *only in
`main`*, whose body is not compiled under an enclosing use-count map; the same
code inside a called helper was always correct. The first version of the
regression fixture put its cases in helper functions and passed against the
unfixed compiler. `tests/parity/match_arm_outer_binding.flx` therefore writes
every case directly in `main`, and both its assertions were confirmed to fail
without the fix.

### KI-012 — Class dispatch fails for a value built by a named-field constructor — FIXED 2026-08-23

**Severity:** High · **Area:** Type classes / dispatch · **Verified:** 2026-08-23

A class method applied to a value written with named-field syntax reached the
no-instance panic stub instead of its instance:

```flux
data Colour { Red { on: Bool }, Blue { on: Bool } }
class Describe<a> { fn describe(value: a) -> String }
instance Describe<Colour> { fn describe(value) { ... } }

describe(Red { on: true })
// error[E1009] panic: No instance of Describe.describe for the given type
```

**Root cause.** `rewrite_named_constructor` in `src/ast/desugar_named_fields.rs`
rewrites `Red { on: true }` into an ordinary constructor call, and stamped the
synthesized `Expression::Call` with `ExprId::UNSET`. That pass runs *after*
inference, so `hm_expr_types` already held the constructed value's type keyed by
the original `NamedConstructor`'s id — and discarding the id stranded it.
`try_resolve_class_call` (`src/core/lower_ast/mod.rs`) looks the first argument's
type up by id, found nothing, and fell through to the stub whose body is
`panic("No instance ...")`.

The symptom read as "only the first instance dispatches", but neither instance
resolved at compile time; positional constructors were unaffected, which is what
made one arm appear to work.

**Fix.** Carry the original id onto the synthesized call.

---
### KI-038 — VM slow-path arithmetic is unchecked

**Severity:** High · **Area:** VM backend · **Verified:** 2026-08-26

`src/vm/binary_ops.rs` uses plain `+`, `-`, and `*` on its slow path, while
`src/vm/dispatch.rs` uses wrapping operations on the fast path. With Cargo's
current `overflow-checks = false` the difference is hidden; enabling checks
would make integer semantics depend on the emitted opcode shape. Reproduce with
an integer expression that forces the slow path and compare it with the fast
path equivalent.

### KI-039 — Native integers truncate values that the VM boxes

**Severity:** High · **Area:** VM/native parity · **Verified:** 2026-08-26

The VM nanobox falls back to a heap box for integers outside its inline payload,
but native `flux_tag_int` has no equivalent fallback. For example, `2^62`
round-trips on the VM and becomes truncated garbage natively. Reproduce by
printing that value under both backends.

### KI-040 — `i64::MIN / -1` is unguarded

**Severity:** High · **Area:** arithmetic · **Verified:** 2026-08-26

The VM integer division path and native `sdiv` both leave the two's-complement
minimum divided by `-1` unguarded. Reproduce with `(-9223372036854775807 - 1) /
-1` and compare the backend behavior.

### KI-041 — E1010 and E060 describe unwired overflow behavior

**Severity:** Medium · **Area:** diagnostics · **Verified:** 2026-08-26

`INTEGER_OVERFLOW` (E1010) and `CONST_OVERFLOW` (E060) are registered and
documented, but have no construction sites. Their documented behavior is not
currently observable. Reproduce by searching the source for constructors of
both diagnostics; either wire the codes to defined semantics or remove them.

### KI-042 — Package-store garbage collection policy is undecided

**Severity:** Low · **Area:** package store · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

The content-addressed store supports explicit `flux clean --store`, but it has
no automatic LRU or reachability-based garbage collection. The policy can be
chosen after the store has real usage data.

### KI-043 — Workspace field inheritance is incomplete

**Severity:** Medium · **Area:** package manager, workspaces · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

Workspace member discovery, shared cache placement, shared lockfiles, and
version inheritance are implemented. Members cannot yet inherit `license` or
common dependencies from the workspace root.

### KI-044 — Registry hosting and package naming policy are deferred

**Severity:** Medium · **Area:** package registry · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

Flux has no hosted registry yet, so registry ownership, naming, squatting rules,
and the initial hosting policy remain undecided. This is intentionally deferred
until Flux is released and has users; local index and Git workflows remain
available.

### KI-045 — Registry yanking semantics are undecided

**Severity:** Medium · **Area:** package registry · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

The future registry has not settled whether and how versions can be yanked,
including how a yanked version behaves when it is already present in a lockfile.

### KI-046 — Registry publish-age preference is undecided

**Severity:** Low · **Area:** package registry, supply chain · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

No default policy exists for preferring or delaying newly published versions.
This remains a registry design question rather than a local package-manager
requirement.

### KI-047 — Lockfile v1 serialization needs external compatibility validation

**Severity:** Low · **Area:** package manager, lockfiles · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

The v1 lockfile has deterministic ordering, inline checksums, and format
preservation, but it has not yet been validated against real multi-contributor
merge-conflict patterns. The format should remain stable while this evidence is
collected.

### KI-048 — Windows store path limits are not validated

**Severity:** Medium · **Area:** package store, Windows · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

The store layout has not been tested against long package names, compiler ABI
segments, and nested target paths on Windows. A path-shortening strategy may be
needed before Windows becomes a supported package-manager target.

### KI-049 — One-version-per-package remains a linker limitation

**Severity:** Medium · **Area:** package resolver, linker · **Verified:** 2026-08-26 · **From:** [0177](proposals/implemented/0177_package_manager.md)

The resolver permits only one version of a package name in a graph because the
current linker uses flat global names. Per-package symbol mangling would be
required before semver-incompatible duplicate versions can be supported safely.

### KI-050 — Same-class contextual instances dispatch the element to themselves — FIXED 2026-08-30

**Severity:** High · **Area:** type classes, dictionary dispatch · **Verified:** 2026-08-30 · **From:** [0179](proposals/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

When an instance context names the *same* class as its head — the
`instance Encode<a> => Encode<Option<a>>` shape used throughout
[`lib/Flow/Json.flx`](../lib/Flow/Json.flx) — a call to the class method inside
the instance body is ambiguous: it may recurse on the container or dispatch on
the *element*, and only the element case should go through the context
dictionary.

```flux
class MyEq<a> { fn my_eq(x: a, y: a) -> Bool }
instance MyEq<Int> { fn my_eq(x, y) { x == y } }
instance MyEq<a> => MyEq<List<a>> {
    fn my_eq(xs, ys) {
        match xs {
            [h1 | t1] -> match ys { [h2 | t2] -> my_eq(h1, h2) && my_eq(t1, t2), _ -> false },
            _ -> true
        }
    }
}
fn main() with IO { print(my_eq([1], [2])) }   // printed `true`; should be `false`
```

**Root cause.** `rewrite_instance_self_calls` in
[`src/types/class_dispatch.rs`](../src/types/class_dispatch.rs) ran before
inference and matched on the method name alone, so it rewrote *both* calls into
a self-call. `my_eq(h1, h2)` on two `Int`s was sent to
`__tc_MyEq_List<a>_my_eq`, missed the cons arm, and silently answered `true`.
`Json.encode` on a `List`/`Option`/`Array` failed with
`E1001 Cannot call non-function value` for the same reason. The
*different*-class context form (`instance Eq<a> => MyEq<List<a>>`) was
unaffected because the element call resolved to another method name.

**Fix.** The name-directed rewrite is gone. Contextual dictionary resolution is
now type-directed: `ClassEnv::resolve_instance_context_dictionary_requests`
reports each dictionary a matched instance needs, and a request whose type is
still polymorphic is satisfied from the current function's contextual
`__dict_*` parameter by `InferType::same_shape`. Container calls use the current
mangled instance method and forward that dictionary; element calls extract the
method from it. Covered by
[`tests/parity/contextual_instance_eq_list.flx`](../tests/parity/contextual_instance_eq_list.flx)
on both backends and the `Flow.Json` `Encode<Option<Int>>` / `Encode<List<Int>>`
/ `Encode<Array<Int>>` VM regression in `tests/integration/vm_json.rs`.

Three regressions surfaced while landing the type-directed path and are fixed
alongside it, each guarded by a parity fixture:

- Multi-parameter instances (`Convert<Int, String>`) are matched positionally
  on the first argument, so the context-dictionary resolver cannot match the
  full head. It now keeps the direct call with no dictionaries instead of
  abandoning it to the panicking generic stub
  (`tests/parity/multi_param_class_direct_call.flx`).
- A class default method is cloned into every instance. With name-directed
  self-calls gone, typed dispatch keys on `hm_expr_types[expr_id]`, so the
  clones needed fresh `ExprId`s or the last instance inferred decided dispatch
  for all of them (`tests/parity/class_default_method_recursion.flx`).
- Stdlib modules now generate their declared instance methods, but must not
  also synthesize built-in instance bodies: the runner compiles them through
  one shared interner, and interning `__tc_Num_Int_add` there made a later
  user file resolve its own `add(5, 10)` to a function never generated for it
  (`DispatchGenerationOptions::include_builtin_instances`).

The `Flow.Json` case is covered by the cross-backend and cached-path regression
fixture in [KI-051](#ki-051).

### KI-051 — Cross-module contextual instances — FIXED 2026-08-31

**Severity:** High · **Area:** type classes, module linking, native backend · **Verified:** 2026-08-31 · **From:** [0179](proposals/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

```flux
import Flow.Json as Json
import Flow.Json exposing (encode)
fn main() with IO { print(Json.encode_json(encode([1, 2]))) }
```

**Cached parallel VM — FIXED.** The run failed with
`missing imported global __tc_Encode_Int_encode`. A module body defines its
generated instance methods under the module-qualified name
(`Flow.Json.__tc_Encode_Int_encode`), but `preload_imported_instance_schemes`
marked the *bare* name `Imported`, so the linker demanded a global no module
defines. The bare name is only ever written at load time by
`emit_instance_method_aliases`, which emits an `OpSetGlobal` — a store, not a
definition the linker can satisfy.

The importer now imports the qualified symbol the defining module actually
exports and leaves the bare name a local definition for the alias to fill.
Covered by
[`tests/parity/imported_contextual_instance_cached.flx`](../tests/parity/imported_contextual_instance_cached.flx),
which runs the cached ways the `--no-cache` path could not catch, and by
`imported_contextual_instances_link_on_the_cached_path` in
[`tests/integration/vm_json.rs`](../tests/integration/vm_json.rs). The parity
fixture deliberately uses imported *concrete* instances so it runs unskipped on
all five ways.

Note when re-testing: a `.fxc` written before the fix replays its own stale
`Imported` classification through `hydrate_cached_module_bytecode`, so clear
`target/flux` before concluding the fix did not work.

**Native — FIXED.** `flux --native` now handles imported contextual
`Flow.Json` instances for `Option`, `List`, and `Array`; the parity fixture
[`tests/parity/imported_contextual_instances_native.flx`](../tests/parity/imported_contextual_instances_native.flx)
runs fresh and warm VM/native paths and checks the encoded `"42"` and
`"[1,2]"` results.

The earlier entry said the gap was in "how the native backend constructs or
applies a contextual dictionary". That is wrong. Core is byte-identical between
the backends (verified: 3016 lines, zero diff), and the fault is *in that
shared Core*. `Flow.Json`'s `List` instance lowers to

```
letrec __tc_Encode_List<a>_encode =
λ__dict_Encode, values.
    let %t28 = (λvalue.
      __tc_Encode_List<a>_encode(__dict_Encode, value))    // wrong
```

The inner `encode(value)` encodes an *element* of type `a` and must go through
the context dictionary. Instead it resolves to the enclosing List instance,
so encoding a list recurses on each element as though the element were itself
a list. The same program written outside the standard library lowers correctly:

```
    let %t3 = (λv.
      let %t4 = __dict_Enc.0
      %t4(v))
```

Both backends receive the broken Core; only native executes it. The VM reaches
the same call through the AST lowering path in
[`compiler/expression.rs`](../src/compiler/expression.rs), which resolves it
correctly — the same twin-path split that Stage 4 had to keep in lockstep. So
this is a mis-lowering that the VM happens to bypass, not a native gap.

The shared Core defect was duplicate expression IDs when explicit instance
method bodies were cloned. The generated copy now refreshes every expression
ID, so `hm_expr_types` cannot reuse the source body's type entry; inner calls
in contextual instances therefore resolve through the dictionary rather than
recursing through the enclosing container instance.

The native linker defect for dotted user modules is fixed in the same change:
generated `__tc_*` methods belonging to a module remain inside that module,
emitting the qualified symbol imported by native callers. Unscoped instances
remain at file scope, and the bare-name aliases used by VM dispatch are still
emitted. The native multi-module regression
[`dotted_module_instance_dispatches_on_vm_and_llvm` in
`tests/native_llvm/native_typeclass_tests.rs`](../tests/native_llvm/native_typeclass_tests.rs)
covers this path.

### KI-052 — A generic wrapper over a contextual instance loses its dictionary — FIXED 2026-08-31

**Severity:** High · **Area:** type classes, dictionary elaboration · **Verified:** 2026-08-30 · **From:** [0179](proposals/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

Calling a contextual instance directly works, but forwarding to it through a
generic function fails at runtime:

```flux
class Enc<a> { fn enc(x: a) -> String }
instance Enc<Int> { fn enc(x) { to_string(x) } }
instance Enc<a> => Enc<List<a>> {
    fn enc(xs) { match xs { [h | t] -> enc(h), _ -> "e" } }
}

fn show_all<a: Enc>(xs: List<a>) -> String { enc(xs) }

fn main() with IO {
    print(enc([5, 6]))        // "5"  — direct call works
    print(show_all([5, 6]))   // E1004 tuple field access expected Tuple, got None
}
```

The wrapper's obligation is `Enc<a>`, but the call inside it needs
`Enc<List<a>>`. Elaboration passes the wrapper's own dictionary rather than
constructing the `List` instance's dictionary from it, so the method extraction
reads a field off `None`.

Both constraint spellings fail identically — `<a: Enc>` and
`where Enc<List<a>>` — so this is not a `where`-clause defect. Verified against
`main` at `c02b680b` before the Stage 3 branch, so it predates the constraint
work: Stage 3 changed which obligations are *retained*, not how a retained
obligation's dictionary is *built*.

**Fixed 2026-08-31 (Stage 4).** Two defects compounded, and the diagnosis in
the paragraph above was wrong: elaboration was not passing the wrapper's own
dictionary, it was passing nothing at all.

`show_all` legitimately holds two dictionaries — `Enc<a>` from its bound, and
`Enc<List<a>>` for the call it makes. Every dictionary parameter was named
`__dict_{Class}` with no occurrence suffix, so both were `__dict_Enc` and the
second shadowed the first. Meanwhile `current_context_dictionary` and its AST
twin have always computed `__dict_Enc_1` for a second occurrence — a name
nothing ever created.

The reference those lookups emit is an *unresolved* variable, because at
AST-lowering time the enclosing function has no dictionary parameters yet;
`dict_elaborate` adds them afterwards. Nothing bound the two together, so the
name escaped to global scope, where no `__dict_Enc` definition exists, and the
call received `None` — hence a field access on `None` rather than on the wrong
dictionary.

The fix names dictionary parameters with the per-class occurrence suffix the
lookups already assumed, pre-interns those names where `&mut Interner` is
available (elaboration only has `&Interner`, and an un-interned name silently
degraded to the bare class name), and binds a dictionary reference to the
parameter that holds it. Both constraint spellings are covered.

Guarded by
[`tests/parity/contextual_dictionary_through_generic_wrapper.flx`](../tests/parity/contextual_dictionary_through_generic_wrapper.flx)
— verified to report MISMATCH with the fix reverted — and by
`a_generic_wrapper_forwards_the_right_contextual_dictionary`, which asserts
stdout so it still catches the bug if both backends were to agree on the wrong
answer.
