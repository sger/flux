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

### KI-070 — A lambda's parameter or return annotation cannot name an enclosing rigid type parameter

**Severity:** Medium · **Area:** Type inference, annotations · **Verified:** 2026-09-02 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

```flux
fn outer<a>(x: a) -> a {
    let f = \y: a -> y      // `a` here is a nominal type, not `outer`'s parameter
    f(x)
}
```

The sibling of [KI-058](#ki-058), left open when that was fixed. `infer_lambda_expression`
([lambda.rs](../src/ast/type_infer/expression/lambda.rs)) builds an explicitly
empty `type_params` map and passes it to both `infer_and_bind_parameter_types`
and `infer_return_type_with_optional_annotation`; `check_lambda_expression`
([checked.rs](../src/ast/type_infer/expression/checked.rs)) does the same in
check mode. So a lambda annotation naming `a` converts to
`TypeConstructor::Adt("a")` rather than the enclosing signature's rigid
variable, exactly as `let` annotations did.

The fix is the one KI-058 used: read the top of
`InferCtx::signature_type_params` instead of an empty map. It was scoped out
deliberately rather than missed — the `let` path is what unblocked Stage 4, and
the lambda path has no known consumer waiting on it.

---


## Resolved

### KI-072 — A constructor pattern is never checked against the scrutinee's type — FIXED 2026-09-03

**Severity:** High · **Area:** Type inference / pattern matching · **Verified fixed:** 2026-09-03 · **From:** Flume typeclass conversion

A `match` arm could name a constructor belonging to a completely unrelated type.
It compiled clean and fell through to the wildcard.

```flux
data Colour { Red, Green }
data Shape  { Circle(Int), Square }

fn a(c: Colour) -> Int { match c { Circle(n) -> n, _ -> 0 } }   // compiled, returned 0
```

**The catch-all arm was the trigger.** Without one the mismatch was reported
correctly as `E300`; adding `_` silenced it. `should_isolate_match_arm_scrutinees`
([control_flow.rs](../src/ast/type_infer/expression/control_flow.rs)) binds each
arm against a *fresh* scrutinee variable when a match mixes pattern families, so
that structural constructors like `Some` and `Left` do not constrain one another
through the shared slot. A fresh variable unifies with anything, so an isolated
arm cannot report a mismatch.

The branch taken when no catch-all is present already excluded this case, keeping
the shared scrutinee when every constraining arm is an ADT family. The branch
taken when a catch-all *is* present was missing that rule. Both branches now
share it: isolation is limited to the built-in families it was introduced for,
and an ADT constructor — which names exactly one declaration — always keeps the
shared scrutinee and reports.

Regression coverage: `infer_constructor_pattern_*` in
[type_inference_tests.rs](../tests/type_inference/type_inference_tests.rs) and
[`examples/compiler_errors/constructor_pattern_wrong_adt_e300.flx`](../examples/compiler_errors/constructor_pattern_wrong_adt_e300.flx).

**Follow-up not taken here:** the diagnostic underlines the whole `match` rather
than the offending arm, because the scrutinee unification is reported at the
match's span. Worth narrowing to the pattern's own span.

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

**Severity:** High · **Area:** type classes, dictionary dispatch · **Verified:** 2026-08-30 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

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

**Severity:** High · **Area:** type classes, module linking, native backend · **Verified:** 2026-08-31 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

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
applies a contextual dictionary". That is wrong: the fault was *in the Core both
backends share*. `Flow.Json`'s `List` instance lowered to

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

Both backends received the broken Core; only native executed it. The VM reaches
the same call through the AST lowering path in
[`compiler/expression.rs`](../src/compiler/expression.rs), which resolved it
correctly — the same twin-path split that Stage 4 had to keep in lockstep. So
this was a mis-lowering the VM happened to bypass, not a native gap.

An earlier revision of this entry backed that claim with "Core is byte-identical
between the backends (verified: 3016 lines, zero diff)". Disregard it. That
comparison used `--dump-core`, which for a multi-module program is not a view of
anything the compiler builds — see [KI-053](#ki-053). The conclusion happened to
be right; the evidence for it was not, and trusting the same instrument again
will mislead the next person the same way.

The shared Core defect was duplicate expression IDs when explicit instance
method bodies were cloned. The generated copy now refreshes every expression ID,
and the companion qualified-class identity defect is fixed too:
semantic class and instance selection carries `ClassId = (owning module, class
name)`, so same-named classes with identical heads remain distinct. The same
module-aware identity drives `__tc_*` and `__dict_*` symbols on VM and LLVM;
unqualified ambiguous class and method references require qualification.
so `hm_expr_types` cannot reuse the source body's type entry; inner calls
in contextual instances therefore resolve through the dictionary rather than
recursing through the enclosing container instance.

A second, distinct native defect is fixed in the same change. A *dotted* user
module failed to link natively even with a purely concrete instance —
`module Data.Enc { public instance Encodable<Int> { ... } }` imported by an
entry file gave

```
ld: "_flux_Data_Enc___tc_Encodable_Int_enc", referenced from: _flux_main
```

because the reference was module-qualified while the definition had been hoisted
to file scope under the bare name. A single-segment `module Enc` linked fine.
Generated `__tc_*` methods belonging to a module now remain inside that module,
emitting the qualified symbol native callers import. Unscoped instances
remain at file scope, and the bare-name aliases used by VM dispatch are still
emitted. The native multi-module regression
[`dotted_module_instance_dispatches_on_vm_and_llvm` in
`tests/native_llvm/native_typeclass_tests.rs`](../tests/native_llvm/native_typeclass_tests.rs)
covers this path.

### KI-052 — A generic wrapper over a contextual instance loses its dictionary — FIXED 2026-08-31

**Severity:** High · **Area:** type classes, dictionary elaboration · **Verified:** 2026-08-30 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

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

### KI-053 — `--dump-core` and the other whole-program dumps report types from the wrong module

**Severity:** Medium · **Area:** driver, diagnostics tooling · **Verified:** 2026-08-31 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

Every dump flag that needs a whole-program view — `--dump-core`, `--dump-aether`,
`--dump-cfg`, `--dump-lir`, `--emit-llvm`, `--trace-aether` — concatenates the
modules into one `Program`:

```rust
// src/driver/pipeline/program.rs
fn merge_programs<'a>(programs: impl IntoIterator<Item = &'a Program>) -> Program
```

Each file is parsed by its own `Parser`, and every `ExprIdGen` starts at 1
([`src/syntax/expression.rs`](../src/syntax/expression.rs)), while inferred types
are keyed on that bare number with no module component:

```rust
// src/ast/type_infer/mod.rs
expr_types: HashMap<ExprId, InferType>,
```

So the merged view asks one flat map about ids that several modules each
allocated from 1. Measured on `import Flow.Json` plus a four-line entry file:
each module internally clean, the merge holding **2616 expressions across 987
distinct ids**, with `ExprId(1)` appearing 13 times.

The dumps therefore show expressions annotated with another module's types, and
lowering decisions taken from them — a dumped call can resolve to an instance
the compiler would never pick. Normal compilation is unaffected: modules are
compiled one at a time and `hm_expr_types` is replaced per module, never merged.

**Why this matters more than a cosmetic dump bug.** KI-051's recorded diagnosis
was derived from one of these dumps and was wrong as a result, which cost two
attempts. Any diagnosis that quotes a multi-module dump is unsound.

**A worked example, visible in the checked-in snapshots (2026-09-01).** Once
Stage 7 gave `Eq` a contextual instance over `List`, `--dump-core` began showing
`Flow.Array.contains` — which is generic over `a: Eq` and must dispatch through
its dictionary — statically resolved to the `List` instance:

```
letrec contains =
λ__dict_Eq, arr, x.
    let %t = (λv. __tc_Eq_List<a>_eq(__dict_Eq, v, x))   // wrong instance
    any(arr, %t)
```

The correct lowering, which `Flow.List.contains` still shows in the same dump,
projects the method out of the dictionary (`__dict_Eq.0`). If the dumped form
were what ran, `contains(#[1, 2, 3], 9)` would answer `true`, since an `Int`
matches neither list arm and the fallthrough returns `true`.

It answers `false` — on the VM and natively, with caches cleared. The compiled
program is correct; only the merged view is wrong. The `tests/snapshots/aether/`
files therefore contain a call that no compiled program makes, which is worth
knowing before reading one as evidence.

**Workaround while this is open:** dump a single module, or read the Core for the
module you care about from its own compile rather than the merged view.

Two candidate fixes: renumber `ExprId`s when concatenating so the merged program
is internally consistent, or refuse the merge and dump per module — the latter
also makes the output match what is actually compiled.

### KI-054 — An imported contextual instance method is declared one parameter short natively

**Severity:** Low · **Area:** type classes, native backend · **Verified:** 2026-08-31 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

`build_public_class_method_scheme`
([`src/compiler/mod.rs`](../src/compiler/mod.rs)) rebuilds an imported instance
method's scheme from the *class* method signature and ends in `generalize(...,
&HashSet::new())`, so `Scheme.constraints` is always empty — the instance's
`context` is dropped. `native_function_arity` derives an extern's arity from
exactly those constraints, so a contextual instance method is registered with
only its declared parameters:

```
extern __tc_Encode_Int_encode        arity=1 constraints=0   # correct
extern __tc_Encode_List<a>_encode    arity=1 constraints=0   # takes 2
```

The definition takes one leading dictionary per context entry, prepended by
`context_dict_param_names` in
[`class_dispatch.rs`](../src/types/class_dispatch.rs). Concrete instances have
an empty context and are unaffected.

**Not currently observable.** `resolve_external_symbol` ignores the `arity`
field for a direct extern call, and both the flat (`encode([1, 2])`) and nested
(`encode([[1, 2], [3]])`) cases were verified to behave identically with and
without a correction. It is recorded because it is a real disagreement between
what the extern declares and what the callee takes, in the area KI-051 came
from, and because the VM masks the same class of mismatch behind a deliberate
fixup for `__tc_*` closures in
[`vm/function_call.rs`](../src/vm/function_call.rs) — so nothing would catch it
if a code path started honouring the field.

### KI-055 — Native builds emit an unreferenced forwarding copy of every module-owned class method

**Severity:** Low · **Area:** type classes, native backend · **Verified:** 2026-09-01 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

A module-owned instance method is emitted twice: the real definition inside its
module, and a file-scope bare-name forwarding alias
(`generated_instance_method_alias` in
[`src/compiler/mod.rs`](../src/compiler/mod.rs)). The alias exists so VM dispatch
can keep using the canonical short name — `emit_instance_method_aliases`
([`compiler/passes/codegen.rs`](../src/compiler/passes/codegen.rs)) overwrites
the bare global with the module's function at load time via `OpSetGlobal`.

The native backend has no such load step, so the alias is compiled as a real
function. Both copies are exported, and their qualified names collide whenever a
single-segment module shares its file's stem — resolved by the name-claiming
de-duplication in [`src/lir/lower.rs`](../src/lir/lower.rs), which gives the
second claimant a `_1` suffix:

```
_flux_Alpha___tc_m5_416C706861_Render_Int_render
_flux_Alpha___tc_m5_416C706861_Render_Int_render_1     ← the alias
_flux_Alpha_render
_flux_Alpha_render_1                                   ← the stub's alias
```

**Measured: zero references to the `_1` copies across every object in the
build**, while each is still exported as `T`. `main.o` resolves straight to the
module-qualified definitions, as it should. So natively the alias is dead
exported code, and de-duplication is suppressing a symbol nothing wants.

GHC has no analogue. A dictionary function there is one top-level binding with
one name derived from `(module, unique)`; nothing resolves dfuns by short name,
so there is nothing to alias. Not emitting the alias natively is therefore the
more GHC-faithful shape, not a departure from it.

**Fix:** skip the alias in native lowering, where the module already exports the
qualified symbol, and keep the de-duplication as a backstop rather than the
mechanism. Marking generated aliases structurally — a flag on the def rather
than a name prefix — would let lowering handle them deliberately instead of
discovering the clash by string equality.

Not urgent: the emitted programs are correct, and the cost is object size plus
one call hop per native class-method dispatch. Recorded so the measurement above
does not have to be re-derived.

### KI-056 — Two module files sharing a stem collide natively

**Severity:** Medium · **Area:** native backend, module linking · **Verified:** 2026-09-01

Two modules in different directories whose *files* share a stem fail to link:

```
examples/.../QualifiedClassId/Alpha/Render.flx    module QualifiedClassId.Alpha.Render
examples/.../QualifiedClassId/Beta/Render.flx     module QualifiedClassId.Beta.Render
```

```
ld: 105 duplicate symbols
```

The module-qualified names are distinct (`QualifiedClassId_Alpha_Render_…` vs
`…_Beta_…`), so the module's own functions are fine. The collision is in the
*file-scope* definitions each module carries — chiefly its private copies of the
built-in instance methods (`__tc_Show_Int_show`, `__tc_Ord_Int_gt`, …), which
`collect_module_paths` in [`src/lir/lower.rs`](../src/lir/lower.rs) qualifies
with the entry file's **stem**. Both files are `Render.flx`, so both produce
`flux_Render___tc_Show_Int_show`.

The stem is a reasonable guard against clashing with C runtime primops
(`flux_sum` in `libflux_rt.a`) but is not unique across a project — only the
module path is. Deriving the qualifier from the module path, and falling back to
the stem only for a file that declares no module, would remove the collision.

Discovered while adding `examples/type_classes/qualified_class_id.flx`, which
was written with both modules in `Render.flx` and had to be restructured to
distinct stems to compile natively. The VM is unaffected.

### KI-057 — A method reachable from two dictionaries always uses the first — FIXED 2026-09-01

**Severity:** High · **Area:** type classes, dictionary elaboration · **Verified:** 2026-09-01 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

A function holding two dictionaries for the same class, over *different* type
variables, dispatches every call to the first of them:

```flux
class Root<a> { fn root(x: a) -> Int }

instance Root<Int>    { fn root(x) { x } }
instance Root<String> { fn root(x) { 7 } }

fn both<a: Root, b: Root>(x: a, y: b) -> Int {
    root(x) + root(y)
}

fn main() with IO {
    print(both(5, "hi"))       // prints 14; should print 12
}
```

`root(x)` must dispatch through `a`'s dictionary and `root(y)` through `b`'s.
Both go through one of them, so the answer is `7 + 7` rather than `5 + 7`. Wrong
output, not a crash, on **both backends**.

The cause is that a class method is mapped to a dictionary by *name alone*.
Natively that map is `method_map` in
[`dict_elaborate.rs`](../src/core/passes/dict_elaborate.rs), keyed on the method
identifier; on the VM it is `dictionary_path_to_method` in
[`expression.rs`](../src/compiler/expression.rs). Neither consults the type of
the argument at the call site, so the constraint a call belongs to is never
determined and the first matching dictionary parameter wins.

KI-052 fixed the *naming* half of this — a second dictionary for one class is
now a distinct parameter, `__dict_Root_1`. This is the *selection* half: the
parameters are distinct but nothing chooses between them.

**This predates Stage 5.** The reproduction above uses no superclasses and
prints `14` on `main` as well. Superclass evidence inherits the same limitation
— two constraints whose closures both reach one superclass pick that superclass's
evidence from whichever dictionary is found first — but does not cause it.

**Fixed** by selecting on the predicate rather than the method name. The map
from method to dictionary now holds every candidate a function's constraints can
reach, and the call's argument types choose between them:
`ClassEnv::dispatch_positions` says which argument reveals each class parameter
and `select_dictionary` matches what it finds against each candidate. Both are
defined once and used by the elaborator and by the check below, so selection
cannot drift between them.

Two rules make it correct rather than merely different:

- **Equal predicates are interchangeable.** There is at most one instance per
  type, so two constraints over the same type reach the same implementation
  whichever is picked. Without this, `fn f<a: Sizeable, b: Sizeable>` calling
  `size` would look ambiguous the moment a superclass gave the method a second
  route to the same dictionary.
- **A call that cannot decide is reported, not guessed.** A class parameter
  mentioned nowhere in its method's signature can never be named by any call
  site; that is now **E485**, not a silent first-match.

Covered by `two_dictionaries_one_class.flx`, `two_dictionaries_superclass.flx`
and `ambiguous_dictionary_e485.flx`, all on both backends.

Selection reads argument types only; a method dispatched on its result type
keeps its previous behavior.

A follow-up investigation looked for the case that would need more — a function
holding two dictionaries for one class, distinguished only by result type — and
found it cannot currently be written: result-directed dispatch is driven by the
return position alone, and a function has one of those. A `let` annotation
cannot supply a second, because annotating with a rigid type parameter is itself
rejected ([KI-058](#ki-058)). So the argument-directed rule covers every
reachable program today. Revisit if KI-058 closes.

The investigation also corrected two claims worth not repeating: `Flow.Json`'s
inner `decode` does **not** go through dictionary elaboration — it takes the
AST path in `compiler/expression.rs` — and it resolves through a single
`Decode<a>` dictionary, not by choosing between two.

---

### KI-059 — `deriving` on a parameterized ADT produced a dictionary nothing defines — fixed 2026-09-02

**Severity:** High · **Area:** type classes, dictionary elaboration · **Verified:** 2026-09-02 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

A `deriving` clause on a data declaration that takes type parameters compiled
its methods but not the evidence that reaches them:

```flux
data Box<a> { Box(a) } deriving (Eq)

fn main() with IO {
    print(eq(Box(1), Box(1)))   // error[E004]: I can't find a value named
}                               //              `__dict_Eq_Box<a>`
```

**Cause.** Not the mangling, and not the two-pass disagreement the first
diagnosis proposed. `builtin_method_body` generates `Eq`'s `eq` as
`__x0 == __x1` for every head. On a parameterized head the operands have type
`Box<a>`, so that operator desugars to an `Eq<Box<a>>` obligation — the
instance's *own* dictionary. A parameterized head derives a contextual instance
(`Eq<a> => Eq<Box<a>>`) whose dictionary is a constructor rather than a value,
so the reference resolved to nothing. The monomorphic case survived only
because a context-free dictionary is a plain tuple that does exist, which is
why `data Color { Red, Green } deriving (Eq)` always worked.

A hand-written contextual instance worked because its body destructures the ADT
and compares *fields*, whose types are rigid parameters discharged by the
context dictionary already in scope.

**Fix.** `derived_structural_eq_body` in
[class_dispatch.rs](../src/types/class_dispatch.rs) synthesizes that same body
from the declaration's own variants — the generalization of
`structural_container_eq_body`, which did this for `List<a>` and `Option<a>`
through a table keyed by head name that no user type could enter. `neq` inlines
the comparison rather than calling `eq(__x0, __x1)`, since that call is on the
head type and reintroduces the same self-reference.

Covered by `examples/type_classes/derived_parameterized_eq.flx`, which pins
positional, nullary, named-field and nested (`Box<Box<Int>>`) variants, both
by name and through an `Eq`-constrained function, on VM and native.

**Carve-out — a head with more than one type parameter is now rejected.** Such
a head carries one context constraint per parameter, so the generated body must
choose between two dictionaries of the same class. `choose_candidate` in
[dict_elaborate.rs](../src/core/passes/dict_elaborate.rs) makes that choice from
the argument's recorded type, and the fields being compared are bound by a
`case` pattern: only `Lam` records binder types, so a pattern binder carries
none and every field resolves through the first dictionary. `deriving (Eq)` on
`data Pair<a, b>` therefore compared the second field with `a`'s evidence.

That clause now reports **E486** naming the parameter count
(`examples/compiler_errors/deriving_multi_param_eq_e486.flx`) — a diagnostic
instead of a wrong answer. Lifting it means teaching the Core pass to type
`case` pattern binders, which needs the constructor's field types instantiated
at the scrutinee's type; the pass already has the `TypeEnv` that holds them.
A hand-written instance is unaffected, though the parser's one-constraint limit
means a two-parameter head cannot express one today.

**Still open: a self-recursive parameterized head.** `data Tree<a> { Leaf,
Node(Tree<a>, a, Tree<a>) }` fails with the same `E004` on
`__dict_Eq_Tree<a>`, because a field whose type *is* the head demands the
instance's own dictionary exactly as `==` did. This is not specific to
`deriving` — a hand-written `Eq<a> => Eq<Tree<a>>` fails identically — so it is
tracked separately as [KI-069](#ki-069).

---

### KI-069 — A contextual instance cannot compare a field of its own head type

**Severity:** Medium · **Area:** type classes, dictionary elaboration · **Verified:** 2026-09-02 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

A recursive parameterized ADT cannot get an `Eq` instance, derived or written
by hand:

```flux
data Tree<a> { Leaf, Node(Tree<a>, a, Tree<a>) }

instance Eq<a> => Eq<Tree<a>> {
    fn eq(x, y) {
        match x {
            Leaf -> match y { Leaf -> true, _ -> false },
            Node(l1, v1, r1) -> match y {
                Node(l2, v2, r2) -> eq(l1, l2) && eq(v1, v2) && eq(r1, r2),
                _ -> false
            }
        }
    }
    fn neq(x, y) { !eq(x, y) }
}
```

`error[E004]: I can't find a value named '__dict_Eq_Tree<a>'`.

The recursive calls `eq(l1, l2)` compare fields whose type is the head type
`Tree<a>`. That resolves to this very instance, whose dictionary is a
constructor rather than a value, so the reference has nothing to bind to — the
same shape as [KI-059](#ki-059), reached through a field instead of through
`==`. A call on a field of type `a` is fine: it is discharged by the context
dictionary in scope.

The fix is for a recursive reference to reuse the dictionary being constructed
rather than demanding it as a value, which is what a self-referential
(knot-tying) dictionary binding provides.

**Workaround:** none for a recursive parameterized head. A recursive
*monomorphic* ADT is unaffected, since its dictionary is a plain tuple.

---

### KI-060 — A module-scoped contextual instance cannot call its own method on its own head type — FIXED 2026-09-02

**Severity:** High · **Area:** type classes, module-scoped instances · **Verified:** 2026-09-02 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

Inside a `module { }` block, a contextual instance whose method recurses through
the class method *on the instance's own head type* reached the wrong dictionary:

```flux
module M {
    public class MyEq<a> { fn my_eq(x: a, y: a) -> Bool }
    public instance MyEq<Int> { fn my_eq(x, y) { x == y } }
    public instance MyEq<a> => MyEq<List<a>> {
        fn my_eq(xs, ys) {
            match xs {
                [h1 | t1] -> match ys { [h2 | t2] -> my_eq(h1, h2) && my_eq(t1, t2), _ -> false },
                _ -> match ys { [h | t] -> false, _ -> true }
            }
        }
    }
}
```

`my_eq(t1, t2)` compares two `List<a>` and must recurse through the `List`
instance applied to the element dictionary. It reached `MyEq<Int>` instead.

**Cause.** A module-owned instance method is emitted twice: the implementation
inside the module, and a bare forwarding alias outside it, because HM
predeclaration and VM dispatch resolve by the canonical bare name. That alias
forwards *every* parameter, dictionaries included, so its call is already
complete when Core lowering sees it.

`resolve_dict_args_for_module_member_call`
([lower_ast/mod.rs](../src/core/lower_ast/mod.rs)) then inserted evidence a
second time. It had no "the arguments already carry dictionaries" guard, which
the three other insertion sites all have —
`should_insert_source_dict_args_for_identifier`'s sibling branch,
`Compiler::looks_like_dictionary_argument_ast`, and `already_has_dict_args` in
[dict_elaborate.rs](../src/core/passes/dict_elaborate.rs). Resolving from
arguments already shifted by one froze a *concrete* dictionary into a generic
forwarder:

```
λ__dict_MyEq, xs, ys.
    M.__tc_…_List<a>_my_eq(__dict_MyEq_Int, __dict_MyEq, xs, ys)
```

That is the call site's evidence baked into a function that is supposed to be
generic — and a four-argument call into a three-parameter function. The alias
now forwards its own dictionary unchanged. `lib/Flow/Eq.flx` was affected too:
the aether Core dumps show the same `__dict_…_Eq_Int` removed from the
prelude's own `List<a>` and `Option<a>` forwarders.

**Fixed on both backends.** The AST compiler now materializes the runtime
dictionary parameters implied by filtered class constraints, reuses hidden
parameters supplied by the IR, and avoids duplicating dictionary parameters on
generated contextual methods. AST contextual lookup also reuses the method's
explicit leading dictionary, so recursive calls stay on the current context.
The repro prints `true`, `true`, `false` under both the VM and native backend.

Giving the callee those parameters changes its compiled arity, so the caller
side had to move with it — see [KI-061](#ki-061), fixed together with this.

**Correction to the earlier diagnosis.** This entry previously said the alias
*shadows* the real definition. It does not: the module's implementation and the
top-level alias are distinct symbols. The whole-program Core dump renders both
without the module prefix ([KI-053](#ki-053)), which reads as a duplicate
definition and is misleading.

**Workaround:** recurse through a private helper rather than through the class
method, and compare elements with the operator (see KI-061 for why the operator
and not `my_eq(h1, h2)`). `lib/Flow/Eq.flx` is written this way.

---

### KI-061 — A constrained function inside a module that calls a class method by name gets no dictionary — FIXED 2026-09-02

**Severity:** High · **Area:** type classes, dictionary elaboration, modules · **Verified:** 2026-09-02 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

```flux
import Flow.Eq exposing (..)
module M2 {
    public fn all_same<a: Eq>(xs: List<a>, x: a) -> Bool {
        match xs {
            [h | t] -> eq(h, x) && all_same(t, x),
            _ -> true
        }
    }
}
```

```
error[E1009]: panic: No instance of Eq.eq for the given type
  at Flow.Eq.eq
  at M2.all_same
```

The `eq(h, x)` call is compiled as a plain call to the class's runtime dispatch
stub instead of a projection out of `all_same`'s dictionary parameter, so it
panics for any non-primitive element. Three variations pin the shape:

| where the function lives | how it calls the method | result |
|---|---|---|
| top level of the entry file | `eq(h, x)` | works (`derived_eq.flx`) |
| inside `module M2` | `eq(h, x)` | **panics** |
| inside `module M2` | `h == x` | works |

The class may live in the same module or another; it makes no difference. The
operator form works because it takes a different route to the same obligation,
and that is why every constrained function in `lib/Flow` — `List.contains`,
`Array.sort`, `Assert.assert_gt` — has only ever used operators.

Not to be confused with KI-060, which is about an *instance method* recursing
on its own head; this is about a *free function*.

**Fixed together with [KI-060](#ki-060).** A constrained function's compiled
arity is its source arity plus one leading dictionary per runtime-bearing
constraint, on the AST path as well as the CFG path. KI-060 established that
for the callee; this entry is the caller half, which took three changes:

* `Module.f(..)` prepends the same evidence a local call does
  (`try_build_constrained_module_member_call`), and a bare-name call to a
  module sibling qualifies the name before looking its scheme up — otherwise a
  module's own recursion missed the dictionaries the function was compiled
  with.
* A constraint instantiated at one of the *enclosing* function's type
  parameters forwards that function's own dictionary instead of selecting an
  instance. `sort<a: Ord>` calling `sort_by<a, b: Ord>` at `b = a` has no
  instance to pick; the previous code defaulted the undetermined variable to
  `Int` and passed the wrong evidence.
* An imported instance's `__dict_*` global is declared by both preload paths
  and stored at load time by `emit_imported_dict_globals`. Only the *defining*
  module's Core carries the `__dict_*` def that `ir_lowering` declares from,
  and a cached dependency contributes no Core at all, so a cross-module
  reference otherwise failed with `E004` — or, once declared but unstored,
  `E1001 Cannot call non-function value`.

`CACHE_EPOCH` moved to 40: an epoch-39 `.fxc` was compiled to the old
convention, so a fresh caller would arrive one argument too many.

---

### KI-062 — The parity harness accepts a fixture that fails to compile on both backends

**Severity:** Medium · **Area:** parity harness · **Verified:** 2026-09-01

`tests/parity/primop_string_ops.flx` declares `expect: success` and uses `++`,
which is not a Flux operator:

```
error[E031]: Expected Expression
Expected expression, found `+`.
```

The five-way parity sweep nonetheless reports it as passing. The harness
compares the two backends' outputs; when both fail to compile with the same
diagnostic, the outputs match, and `expect: success` is never checked against
the compile result. A fixture can therefore pass parity without ever running —
the one thing a parity fixture exists to prove. The fixture is stale and the
harness should enforce `expect:`.

There is also dead code behind this: `infer_semigroup_operator`, the
`"++" => "append"` desugar arm and `CorePrimOp::Concat` all handle an operator
the lexer never produces. `Semigroup` is reached only as `append(x, y)`.

### KI-063 — A user function named after a class method broke `--no-cache` — **fixed 2026-09-02**

**Severity:** High · **Area:** type classes, compiler · **Verified:** 2026-09-02

A program defining a function named after a prelude class method failed to
compile whenever the standard library was compiled rather than loaded from
cache:

```
$ flux --no-cache file.flx
error[E001]: Duplicate Name
Duplicate binding: `add` is already defined.
```

`fn add`, `fn eq`, `fn show` — any of them. A warm or cold *cached* run was
fine, which is what hid it: only `--no-cache` compiles every stdlib module in
the same `Compiler` as the user's file. Four `examples/` fixtures were failing
in `examples_fixtures_snapshots` for this reason, and it was first mistaken for
a defect in that harness.

**Cause.** Proposal 0179 Stage 8 moved `Eq`, `Ord`, `Num`, `Show` and
`Semigroup` from Rust registration into Flux modules. `generate_dispatch_functions`
emits a polymorphic dispatch stub under each class method's *bare* name, in
every compilation unit that has the class in scope, skipping any name that
unit's own program already defines (`reserved_names`). Compiling
`lib/Flow/Num.flx` therefore defines a top-level `add`, and one `Compiler`
compiles every module of a program, so that stub was still defined when the
user's file declared `fn add`. Before Stage 8 there was no `Flow.Num` module to
compile and nothing generated the stub ahead of user code.

**Fix.** The compiler records the stub names it generates
(`Compiler::generated_dispatch_stub_names`), and `phase_predeclaration` lets a
real function declaration take one over instead of reporting a redeclaration.
That matches the resolution rule already in force: a name bound to a function
never dispatches as a class method, so the user's function was always going to
win — it just could not be declared. A genuine duplicate (two `fn foo` in one
file) still reports E001.

Regression coverage: `tests/parity/user_fn_named_like_class_method.flx`, which
the parity sweep runs with `--no-cache` in its `vm` and `llvm_strict` ways.

### KI-064 — A class over a partially applied type constructor could not be used across a module boundary — **fixed 2026-09-02**

**Severity:** High · **Area:** type classes, parser · **Verified:** 2026-09-02

`Either` takes two type parameters, so an `instance Functor<Either<l>>` head is
*partially applied* — the only head of that shape in the stdlib. Two unrelated
defects stood between it and working code.

**1. The predicate matcher rejected a partially applied head.** Matching the
pattern `f<a>` against the actual `Either<String, Int>` compared arities: the
pattern applies one argument and the actual has two, so both higher-kinded arms
in `match_type` (`src/types/class_predicate.rs`) fell through to `_ => false`.
`Option<Int>` lines up one-for-one, which is why nothing had hit this. Across a
module boundary the failure surfaced as `E004 Undefined Variable` on the method
name rather than `E444`, because instance resolution failed and the fallback
dispatch stub is not defined in the calling module. Fixed by binding `f` to the
partially applied head and matching the trailing arguments.

**2. A class method with an effect-row parameter lost its return type.** With
the matcher fixed, the cached path worked and `--no-cache` failed:

```
error[E1004]: Type Error
  expected type: Unit
  found type:    Right    runtime value: Right(3)
  at Flow.Functor.__tc_..._Functor_Either<l>_fmap
```

This was **not** specific to `Either`, or to partially applied heads, or to
`--no-cache` — it was a parser bug affecting every instance of every class
whose method takes an effect-row parameter. `parse_class_method` located the
parameter list's `)` by testing whether the current token was a `)` rather than
by whether any parameters had been parsed. A parameter whose type carries an
effect row is parenthesised (`g: ((a) -> b with |e)`), so parsing it leaves the
cursor on *that* type's closing paren, which was mistaken for the list's. The
real one stayed unconsumed, the `->` after it was never seen, and the method
took the "no return type" branch — becoming `Unit`.

Every `Functor` instance therefore carried a `Unit` return contract. Nothing
rejected it: `List`, `Option` and `Array` bodies tail-call into
`Flow.List.map` and friends, and the return check is not reached on that path.
`Either`'s body returns a constructed `Right(...)` directly, so it was the
first to trip the check. The `Unit` was equally wrong for the other three.

The guard came from the fix for zero-parameter methods (`fn mempty() -> a`),
which needed the cursor to be *on* `)` when the loop never ran. Discriminating
on `params.is_empty()` instead handles both.

Regression coverage:
`class_method_with_effect_row_parameter_keeps_its_return_type` in
`tests/integration/module_scoped_classes_tests.rs` asserts the parse directly,
and `tests/parity/either_instances.flx` runs the whole chain five ways.

### KI-065 — A module's own function was shadowed by a class-method dispatch stub — **fixed 2026-09-02**

**Severity:** High · **Area:** type classes, name resolution · **Verified:** 2026-09-02

A module that defines a function named after a class method reached the class's
*dispatch stub* instead of its own function, from inside its own body:

```flux
module Flume.Resolve.Version {
    public fn compare(a: Version, b: Version) -> Ordering { ... }
    fn lt(a: Version, b: Version) -> Bool {
        match compare(a, b) { Lt -> true, _ -> false }   // panics
    }
}
```

```
panic: No instance of Ord.compare for the given type
  at compare (lib/Flow/Ord.flx:0:1)
```

26 of the 60 tests in `tests/flux/flume_version.flx` failed this way, and every
other `flume_*` fixture with them.

**Cause.** `generate_dispatch_functions` emits a polymorphic stub under each
class method's bare name; compiling `Flow.Ord` therefore defines a global
`compare` whose body panics. Identifier compilation resolves the bare name
first and only then looks for a sibling member of the enclosing module, which
is stored under its qualified key. That order was harmless while no global of
the name existed — before Proposal 0179 Stage 8 there was no `Flow.Ord` module
to compile.

Type inference was never confused: `class_method_call_info` correctly declined
to treat the call as a class method, because the module's own `compare` is
bound with a real span. The misrouting happened later, at symbol resolution.

**Fix.** A generated stub does not shadow a member of the enclosing module. The
check is narrow — it applies only when the resolved symbol is a *global*, the
name is one this compiler generated a stub for, and the enclosing module has a
member of that name — so locals still shadow both, as before.

Regression coverage: `examples/type_classes/module_member_shadows_stub.flx`.

Related: [KI-063](#ki-063) is the same collision at declaration time rather
than at a call site.

### KI-066 — A constrained function's operator resolved to nothing when its unit declared no instances — **fixed 2026-09-02**

**Severity:** High · **Area:** type classes · **Verified:** 2026-09-02

Compiling `lib/Flow/Array.flx` on its own failed:

```
error[E004]: Undefined Variable
I can't find a value named `eq`.
  lib/Flow/Array.flx:176:23
```

Line 176 is `any(arr, \v -> v == x)` inside
`public fn contains<a: Eq>(arr: Array<a>, x: a) -> Bool`. Inside an explicit
class-constraint context `==` desugars to `eq(x, y)`, and that call resolved to
nothing.

**Cause.** The polymorphic dispatch stub a desugared call lands on is generated
from `dispatch_table`, which was filled from two places: instances declared in
*this* unit's statements, and the phantom instances the Rust registration
created. Proposal 0179 Stage 8 deleted the second — the standard classes'
instances are now Flux source in `Flow.Eq` and its siblings. A unit that merely
*uses* a class therefore contributed nothing to the table and generated no
stub.

The normal pipeline hid this: compiling `Flow.Eq` fills the table from its own
instance statements and leaves a global `eq` stub behind in the shared
`Compiler`, which later modules found. Only a unit compiled *without* that —
a `Compiler` built directly, as the surface-wrapper tests do — was left with
nothing to resolve against.

**Fix.** `seed_dispatch_table_from_class_env` records every method of every
class the class environment holds an instance for, so a stub is generated in
any unit that can see the class rather than only where its instances are
written. A class with no instance is skipped: there would be nothing for the
stub to stand in for, and its name should stay free.

Regression coverage: `flow_array_surface_wrappers_compile_without_legacy_warning`
and its siblings in `tests/integration/compiler_rules_tests.rs`, which compile
each `Flow.*` surface module through a bare `Compiler`.

### KI-067 — A locally declared class of a prelude name could not be named in a bound — **fixed 2026-09-02**

**Severity:** Medium · **Area:** type classes · **Verified:** 2026-09-02

Declaring your own `class Eq` and then constraining on it was rejected:

```
error[E456]: Ambiguous Class Constraint
Class constraint `Eq` is ambiguous: matches classes in <prelude>, Flow.Eq.
```

`report_ambiguous_class_constraint` reported whenever two classes shared a
short name, with no precedence. Since Proposal 0179 Stage 8 the prelude
contributes `Eq`, `Ord`, `Num`, `Show` and `Semigroup` to every module, so any
program declaring a class of one of those names was ambiguous by construction
and could not name its own class in a bound.

**Fix.** A class declared in the module being compiled wins over one merely in
scope — the precedence `ClassEnv::resolve_class_id` already applies, and which
the constraint then resolves through. Two classes of the same name that are
*both* foreign are still ambiguous.

### KI-068 — A bare `Compiler` cannot supply the standard classes' instance bodies

**Severity:** Low · **Area:** type classes, embedding · **Verified:** 2026-09-02

Proposal 0179 Stage 8 moved `Eq`, `Ord`, `Num`, `Show` and `Semigroup` out of
Rust and into `lib/Flow/*.flx`, deleting the `builtin_method_body` generation
that used to synthesize `__tc_Ord_Int_lt` and friends into *every* compilation
unit. Their implementations now exist only where those modules are compiled.

A `Compiler` constructed directly — no module graph, no driver — therefore
cannot build a working program that uses them. `ClassEnv::register_prelude_classes`
puts the classes and instances in the environment, so inference and dispatch
succeed, but no implementation is generated. For a *contextual* instance the
failure is visible at compile time: `Ord<Int>` is `Eq<Int> => Ord<Int>`, its
dictionary is a constructor, and `build_contextual_dictionary_expr` bails when
the mangled method names are not interned, so the definition is skipped and the
reference reports `E004 I can't find a value named '__dict_..._Ord_Int'`. A
non-contextual instance such as `Num<Int>` is emitted as a plain tuple of
external references and compiles, but would not link or run.

Every real path is unaffected — the driver, `flux --test`, and the REPL all
compile the `Flow` modules, and `fn max_of<A: Ord>(x: A, y: A)` using `>` works
on both backends and under `--no-cache`. Only a directly constructed
`Compiler` is affected, which is a test and embedding concern.
Two tests drive the real binary for this reason:
`generic_ord_operator_compiles_without_strict_types` in
`tests/type_inference/constrained_type_params_integration.rs`, whose doc comment
records why its `Eq`/`Num` siblings can still use a bare `Compiler`, and
`polymorphic_operator_dump_core_uses_named_class_methods` in
`tests/core_ir/ir_pipeline_tests.rs`, which needs a Core dump and so goes
through `--dump-core --no-cache`.

Restoring the capability means making the prelude modules' instance statements
available to a unit that has no imported implementation — not reinstating the
Rust bodies, which Stage 8 removed deliberately so there is one source of
truth.

---

### KI-058 — A `let` annotation naming a rigid type parameter is rejected — FIXED 2026-09-02

**Severity:** High · **Area:** Type inference, annotations · **Verified:** 2026-09-02 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

Inside a generic function, annotating a `let` with the function's own type
parameter was rejected even when trivially correct:

```flux
fn annotated<a: Root>(x: a) -> Int {
    let y: a = x                // error[E300]: Annotation Type Mismatch
    root(y)                     // error[E444]: No Type Class Instance (cascade)
}
```

```
error[E300]: Annotation Type Mismatch
14 |     let y: a = x
   |                - this value has type `_`
   |            - but `y` was annotated as `a`
```

**Correction to the earlier diagnosis.** This entry said the conversion failed.
It did not. `infer_let_binding` converted the annotation with an empty
type-parameter map, and `convert_type_expr_rec` falls through an unknown
nullary name to `TypeConstructor::Adt(sym"a")`
([type_env.rs](../src/types/type_env.rs)) — so `a` became a *nominal type named
`a`*, which then failed to unify with the signature's rigid variable of the same
name. That is why both sides of the diagnostic rendered as `a`, and why the
value's type showed as `_`.

**Fix.** `InferCtx` now carries `signature_type_params`, a stack of the declared
type parameters of each function whose body is being inferred, pushed beside
`mark_signature_skolems` and popped beside `unmark_skolems`
([function.rs](../src/ast/type_infer/function.rs)). `infer_let_binding` reads
its top and routes through `infer_type_from_annotation`, which also applies
`normalize_associated_types` — a second thing the `let` path had been skipping.

A stack rather than the inverted `skolem_names` the earlier note proposed:
`skolem_names` is keyed by `TypeVarId` and is flat across the whole `InferCtx`,
so inverting it collides whenever two functions use the same parameter name,
and nested functions get no shadowing. `let_annotation_rigid_param.flx` covers
the accepted case; `type_inference_tests.rs` covers the genuine-mismatch and
sibling-function-shadowing cases.

**The soundness hole this unblocked is now resolved.** With annotations usable,
a body can hold more than one result-directed class-method call — the shape
dictionary selection could not handle:

```flux
class Make<a> { fn make(tag: Int) -> a }
instance Make<Int>    { fn make(tag) { 7 } }
instance Make<String> { fn make(tag) { "s" } }

fn two<a: Make, b: Make>(pa: a, pb: b) -> a {
    let y: b = make(0)
    make(0)
}

print(two(1, "z"))   // expected 7 — both backends printed "s"
```

A `String` returned from a function whose return type is `Int`, on the VM *and*
natively (an earlier reading of this expected the two backends to disagree; they
do not — both were wrong the same way).

**Both halves are now fixed.** Selection reads the type a call's result is
required to have, taken from the enclosing return type or `let` binder:

- `ClassEnv::result_positions` says which class parameters a method names as
  exactly its return type — the counterpart of `dispatch_positions`, which
  searches value parameters only.
- Core threads an expected type through `rewrite_expr`: a `Lam`'s declared
  result at tail positions, a `let` binder's own type at its right-hand side.
  `choose_candidate` reads those positions instead of falling back to
  `candidates.last()`.
- A `let`'s type cannot be recovered from Core — there is no type annotation
  node and `CoreBinder` keeps only a `FluxRep`, which collapses every boxed
  type to `TaggedRep` and so cannot tell one rigid parameter from another. It
  is recorded at lowering and carried on `CoreDef::binder_types`.
- The AST path derives the same predicate (`context_predicate_args`) and picks
  the matching dictionary through `context_dictionary_symbol`, which already
  knew how to tell `__dict_C` from `__dict_C_1`.

`examples/type_classes/result_directed_two_dictionaries.flx` locks both
directions, including the one that would pass by accident under the old
last-candidate fallback.

**Still reported, correctly:** a method that mentions its class parameter
*nowhere* — `fn mk(tag: Int) -> Int` on `class Mk<a>` — can be named by no call
site at all, and remains `E485`
(`examples/compiler_errors/ambiguous_dictionary_e485.flx`).

**Guarding the trap the notes recorded.** Scheme constraints are not dictionary
parameters: `Flow.Json`'s `Decode<a> => Decode<List<a>>` method carries two
`Decode` constraints but resolves through the one dictionary its context gave
it. `report_ambiguous_dictionary_calls` therefore skips generated instance
methods, testing the last segment of the name because a module qualifies them
(`Flow.Json.__tc_…`) — the same treatment `emit_instance_method_aliases`
applies.

**On the abandoned attempt recorded here earlier:** it looked for a way to hand
per-call inference data to lowering, and stalled on `CoreExpr` having no
expression id (`CoreExpr::App` alone has ~155 construction sites). That was the
wrong shape. What the selection sites need is not the call's identity but the
type its result must have, and that is a property of the *enclosing* binder or
signature — which Core already carries, or which lowering can record once per
binder.

---

### KI-071 — An instance method captures unqualified calls to a module-level function of the same name

**Severity:** High · **Area:** Type classes / name resolution · **Verified:** 2026-09-03 · **From:** Flume typeclass conversion

Declaring an instance inside a module silently rebinds every *unqualified* call
to a module-level function whose name matches one of the instance's methods.

```flux
module M {
    public data Ordering { Lt, Eq, Gt }

    public fn compare(a: Ver, b: Ver) -> Ordering { ... }

    public fn equals(a: Ver, b: Ver) -> Bool {
        match compare(a, b) { Eq -> true, _ -> false }   // resolves to Ord.compare
    }

    public instance Eq<Ver> => Ord<Ver> {
        fn compare(x, y) { ... }                          // returns Int
    }
}
```

`equals(a, a)` returns `false`. The bare `compare(a, b)` reaches the `Ord`
dispatch stub, which returns `Int`, and the `Ordering` constructor patterns then
never match. Deleting the `Ord` instance restores the correct answer, which is
how the capture was isolated.

The two backends do not agree: the VM produces the wrong answer, and **the
native backend terminates with SIGSEGV**. So this is a parity break and a
memory-safety failure, not only a scoping defect.

**The capture happens after type checking**, which is why nothing reports it.
Inference resolves the bare `compare` to the module's own function and types the
scrutinee as `Ordering`, so the arms check out; the rebinding to the class
dispatch stub is introduced during lowering, when the `Int` it actually returns
is no longer checked against anything. Re-verified 2026-09-03 against the fix for
[KI-072](#ki-072): that check now reports a constructor pattern from the wrong
type, including inside a `module` block, and this program is still silent —
confirming the defect is in lowering, not inference.

**Workaround:** qualify the call (`M.compare(a, b)`), or route internal callers
to a differently named private helper. Qualified calls are unaffected.

Found by adding `Eq`/`Ord` instances to `Flume.Resolve.Version`, which turned 21
of its 60 tests red.

### KI-073 — Result-directed selection is lost on native when routed through a constrained function

**Severity:** High · **Area:** Type classes / native backend · **Verified:** 2026-09-03 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

```flux
class Convert<a, b> { fn convert(x: a) -> b }
instance Convert<Int, String> { fn convert(x) { to_string(x) } }

fn via<a, b>(x: a) -> b where Convert<a, b> { convert(x) }

fn main() with IO {
    let direct: String = convert(42)   // both backends print "42"
    print(direct)
    let routed: String = via(42)       // VM prints "42", native prints <value>
    print(routed)
}
```

A direct class-method call resolves from the result type on both backends —
that is Stage 4 working as specified. Routing the same call through a
`where`-constrained function loses it on native only.

Since `where C<a, b>` is the *only* spelling that reaches a multi-parameter
class (see [type_class_syntax.md](internals/type_class_syntax.md#3-constraining-a-function)),
this makes multi-parameter classes unusable on the native backend in the general
case.

Reproduced by [`examples/type_classes/syntax_tour.flx`](../examples/type_classes/syntax_tour.flx),
which is deliberately **not** mirrored into `tests/parity/` until this is fixed.

### KI-074 — A lowercase class name is declarable but unusable in a `where` clause

**Severity:** Low · **Area:** Parser · **Verified:** 2026-09-03 · **From:** syntax reference audit

```flux
class sz<a> { fn size_of(x: a) -> Int }              // accepted
fn twice<a>(x: a) -> Int where sz<a> { ... }         // error[E034]
```

`peek_starts_class_constraint` ([statement.rs](../src/syntax/parser/statement.rs))
distinguishes a signature-position `where` constraint from a `where x = expr`
local binding by testing whether the next identifier begins with an uppercase
letter. A lowercase class therefore cannot be written in the `where` spelling,
though `<a: sz>` still works.

The diagnostic compounds it: the error blames a missing function body ("This
looks like the function body… Try adding `{` after the function signature")
rather than naming the real rule.

**Either** reject a lowercase class name at its declaration, **or** stop
inferring class-ness from capitalisation. Accepting the declaration and then
refusing the use is the one combination that should not stand.

### KI-075 — `<a: C>` on a multi-parameter class suggests an instance that cannot be written

**Severity:** Low · **Area:** Diagnostics / type classes · **Verified:** 2026-09-03 · **From:** syntax reference audit

```flux
class Convert<a, b> { fn convert(x: a) -> b }
fn via<a: Convert>(x: a) -> String { convert(x) }
```

```
error[E444]: No instance for `Convert<Int>`
Hint: Add an instance: `instance Convert<Int> { ... }`
```

The hint proposes a one-argument instance of a two-parameter class, which is
itself a parse error. The real problem is that the `<a: C>` sugar always means
`C<a>` and cannot supply a second argument, so the constraint is unwritable in
this spelling; `where Convert<a, b>` is what the user needs.

The diagnostic should say so — that the class takes two parameters and the bound
sugar reaches only one — rather than suggesting an impossible declaration.

### KI-076 — An operator on a class-constrained type parameter does not dispatch inside a `module` block

**Severity:** High · **Area:** Type classes / dictionary passing · **Verified:** 2026-09-03 · **From:** Flume typeclass conversion

At top level, a constrained function's operator dispatches through the
dictionary and works on a user type:

```flux
fn bigger<a: Ord>(x: a, y: a) -> a { if x <= y { y } else { x } }   // works
```

The identical function inside a `module` block traps at runtime:

```
cannot compare Adt with OpLessThanOrEqual
```

**The visible consequence is that `List.sort` and `List.sort_by` cannot sort a
user type that has an `Ord` instance.** `sort_by<a, b: Ord>` delegates to
`merge_by_key` ([List.flx](../lib/Flow/List.flx)), whose `key_fn(a) <= key_fn(b)`
is inside `module Flow.List`; the public signature promises `b: Ord` and the
implementation cannot honour it. Annotating the private helpers with `<a, b: Ord>`
does *not* fix it — that was checked, and the constraint reaches them correctly;
the operator is what fails to dispatch.

This is the shape of [KI-061](#ki-061) but for *operators* rather than calls by
name, so that fix did not reach it. The claim in
[Flow/Eq.flx](../lib/Flow/Eq.flx) that "operators desugar through a path that
does carry the dictionary" holds only outside module blocks.

Found by trying to replace the hand-rolled insertion sort in
`Flume.Resolve.Solver` — whose comment explains that the sort is hand-rolled
because `Version` cannot be sorted generically — with `List.sort_by`.

### KI-078 — An instance method calling a sibling method on its own head type gains a dictionary parameter — FIXED 2026-09-03

**Severity:** High · **Area:** Type classes / constraint solving · **Verified:** 2026-09-03 · **From:** Phase 1 of the type-class audit

A constraint on the instance's *own* head, raised inside one of its own
methods, is generalized into a dictionary parameter instead of being
discharged by the instance being defined:

```flux
class MyEq<a> {
    fn meq(x: a, y: a) -> Bool
    fn mneq(x: a, y: a) -> Bool
}

instance MyEq<Int> { fn meq(x, y) { x == y }  fn mneq(x, y) { x != y } }

instance MyEq<a> => MyEq<List<a>> {
    fn meq(xs, ys) { ... }
    fn mneq(xs, ys) { !meq(xs, ys) }        // ← the sibling call
}

fn main() with IO { print(mneq([1, 2], [1, 3])) }
// error[E004]: I can't find a value named `__dict_MyEq_List<a>`
```

`mneq`'s body calls `meq` at `List<a>`, the very head this instance defines.
The solver classifies `MyEq<List<a>>` as `Generalized` — its argument is not
ground, and only ground predicates are matched against instances — so it
becomes a second dictionary parameter:

```
PROBE __tc_MyEq_List<a>_meq:  1 constraints: ["MyEq<?10761>"]
PROBE __tc_MyEq_List<a>_mneq: 2 constraints: ["MyEq<?10775>", "MyEq<List<?10775>>"]

letrec __tc_MyEq_List<a>_mneq = λ__dict_MyEq, __dict_MyEq_1, __dict_MyEq, xs, ys. ...
```

Callers pass one dictionary, the method wants more, and the program fails on
arity — or, when the extra parameter is filled from the enclosing scope,
reaches for a `__dict_*` global that only exists as a Core definition.

A method calling *itself* is unaffected: that lowers to a direct call to the
mangled name and raises no constraint.

The fix is the standard one: match a non-ground predicate against declared
instance heads (THIH's `byInst`, which does not require ground arguments), and
discharge it with that instance's evidence applied to the context the enclosing
function already holds. `resolve_instance_with_subst_by_id`
([class_env.rs](../src/types/class_env.rs)) already does the matching half.

This is the same root cause as the duplicated dictionary parameters in
[KI-077](#ki-077), and it is what kept the helper functions in
[Flow/Eq.flx](../lib/Flow/Eq.flx): until it was fixed the container instances
could not define `eq` by recursion and `neq` as its negation.

**Fixed 2026-09-03 by context reduction.** This entry and [KI-077](#ki-077)
had one cause: the solver retained a scheme constraint whose argument was a
*constructed* type, asking the caller for a dictionary the instance itself
defines. `collect_scheme_constraints`
([class_defaulting.rs](../src/types/class_defaulting.rs)) now reduces every
retained predicate to head-normal form — THIH's `toHnfs` — replacing
`Eq<List<a>>` with the `Eq<a>` its instance requires and dropping the duplicate
that exposes. The extra dictionary parameter disappears, and with it the wrong
evidence in the superclass slot. `lib/Flow/Eq.flx` lost its `list_eq` /
`option_eq` workarounds in the same change.

### KI-079 — A stale bytecode cache runs a program the current compiler rejects

**Severity:** Medium · **Area:** Build caching · **Verified:** 2026-09-03 · **From:** Phase 1 of the type-class audit

The bytecode cache key covers the module's source hash and
`CARGO_PKG_VERSION` ([artifact_store.rs](../src/driver/artifact_store.rs),
[module_cache.rs](../src/bytecode/bytecode_cache/module_cache.rs)) but not the
compiler binary. Two builds of the same version therefore share cache entries,
so a module compiled by an earlier build is reused verbatim — diagnostics
included, which means diagnostics *not* re-reported:

```
$ ./target/debug/flux examples/compiler_errors/instance_missing_method.flx ; echo $?
0
$ ./target/debug/flux examples/compiler_errors/instance_missing_method.flx --no-cache ; echo $?
error[E442]: Missing Instance Method
1
```

For a released compiler the version bump invalidates everything, so this is a
development-time hazard rather than a user-facing one. It is recorded because
it silently invalidates measurement: several claims in the type-class audit,
including "these fixtures exit 0 with no output", were measured against cached
artifacts and are wrong. **Any behavioural comparison across a compiler change
must pass `--no-cache` or clear the store first** (`flux clean --store`).

### KI-082 — Generalizing an unannotated definition breaks two call sites

**Severity:** Medium · **Area:** Type inference / dictionary elaboration · **Verified:** 2026-09-04 · **From:** Proposal 0183, R6

`finalize_and_bind_function_scheme` binds a function that declared no type
parameters with `Scheme::mono`, so its inferred class obligations are never
generalized and never consumed. Turning that off — generalizing every
definition, quantifying the variables a class constraint mentions — is Proposal
0183's R6, and it works: the standard library's terminal stuck predicates fall
from 9 to **0**.

Two programs of 1,305 stop working, and each is a real gap it exposes rather
than a problem with generalizing:

**1. A top-level `let` cannot call a constrained function.** Filed separately as
[KI-083](#ki-083) — it is **not** caused by generalizing, only exposed by it,
and reproduces on the current compiler with an explicit bound.

**2. An arity error is masked by a worse diagnostic.**

```flux
fn add(a, b) { a + b }
let result3 = add(1, 2, 3);   // E056 "wrong number of arguments" → E430
```

(`examples/diagnostics/hint_demos/function_arg_mismatch.flx`.) The fixture
exists to demonstrate `E056`; with `add` generalized the call's result type
stays unresolved and `E430` is reported instead. Diagnostic quality, not
correctness.

A third failure — `[DuplicateBinder] in `multiply`` — was a separate latent bug
in the CFG path's binder-id seeding and is fixed (see the commit that added this
entry). The generalization patch itself is kept at
`scratchpad/r6-generalize-unannotated.patch`.

### KI-085 — A call to a program's own function was dispatched as a class method — FIXED 2026-09-04

**Severity:** High · **Area:** type classes, Core lowering, VM codegen · **Verified:** 2026-09-04 · **From:** Proposal 0183

A bare call to a function the program itself defines was rewritten to a class
method of the same name whenever an instance applied — a **silent wrong
answer**, with no diagnostic:

```flux
fn add(a: String, b: String) -> String {
    "[" + a + "|" + b + "]"
}

fn main() with IO { print(add("a", "b")) }
```

```
"ab"        // Flow.Add's add — string concatenation
```

The user's `add` was compiled correctly and simply never called. `main` held
`__tc_m8_466C6F772E416464_Add_String_add("a", "b")`.

**Cause.** Three places decide whether a bare call is a class method, and only
one of them checked whether the name is already bound:

| | decides by | checked the binding |
|---|---|---|
| `class_method_call_info` (inference) | `env.lookup_span(name) != Span::default()` | yes |
| `LowerCtx::try_resolve_class_call` (Core) | `resolve_method_class_id(name)` | **no** |
| `Compiler::try_resolve_class_method_call` (VM) | `resolve_method_class_id(name)` | **no** |

Inference declined, correctly, and then both lowering paths re-derived dispatch
from the bare name alone and disagreed with it. The two carry comments saying
they "must stay in lockstep"; they were, with each other, and neither with
inference.

**Not introduced by 0183, but widened by it.** On `main` the same program is
wrong whenever an instance exists for the argument type — `fn add(a: Int, b: Int)`
returns `a + b`, and `eq`, `compare` and `show` are hijacked at every type with
an instance. 0183 added `Add<String>`, which `Num` never had, so `add` at
`String` — much the commonest shape — joined them. That is what broke
`tests/flux/flume_edit.flx`: `Flume.Schema.Edit`'s wrapper
`add(name: String, value: String)` became string concatenation, failing 6 of its
18 tests.

Verified against `main` (`b4e35838`): `add(String, String)` and the same call
inside a `module` block both answer correctly there and wrongly on the 0183
branch, while `add(Int, Int)`, `eq`, `compare` and `show` are already wrong on
`main`.

**Fix.** Both lowering sites decline a *bare* name the unit binds to a
user-written function, using the same test inference uses — a `Statement::Function`
carrying a real span, since dispatch generation synthesises its instance bodies
and stubs with `Span::default()`. A **qualified** call still dispatches: it
names the class outright. The fix also closes the pre-existing `eq` / `compare` /
`show` / `add(Int, Int)` cases.

Regression coverage: `examples/type_classes/user_function_shadows_class_method.flx`,
where the user's function and the class method disagree on every line.

Related: [KI-065](#ki-065) is the same collision at symbol resolution, and its
fix covered only a sibling member of an enclosing module.

### KI-084 — A bare `Compiler` cannot build a dictionary for a contextual prelude instance

**Severity:** Low · **Area:** Compiler harness / LSP · **Verified:** 2026-09-04 · **From:** Proposal 0183

A `Compiler` built directly — `Compiler::new_with_interner` plus
`compile_with_opts`, with no module graph — rejects any program that needs a
dictionary for a *contextual* instance of a prelude class:

```flux
fn big<A: Ord>(x: A, y: A) -> Bool { x < y }

fn main() { big(8, 2) }
```

```
error[E004]: I can't find a value named `__dict_m8_466C6F772E4F7264_Ord_Int`.
```

The same program compiles and runs through the driver, which is the path the
CLI, the test runner and every real compilation take. Only the bare-`Compiler`
harness is affected: unit tests that build one directly, and the LSP's
"view Core IR" / "view bytecode" commands (`crates/flux-lsp/src/handlers/view.rs`),
which show the error text in place of the dump. Ordinary LSP diagnostics,
hovers and completion do not use it.

**Cause.** `ClassEnv::register_prelude_classes` parses `lib/Flow/Eq.flx` and its
siblings for their *declarations*, so the class environment knows every prelude
class and instance. It never compiles their *bodies*, so no
`__tc_<class>_<type>_<method>` symbol is interned. `emit_dictionary_defs`
(`src/core/passes/dict_elaborate.rs`) needs all of a dictionary's slots or none
— a short tuple would read every later slot at the wrong index — so it emits no
def for any prelude instance. For a plain instance that is harmless: the
dictionary is a tuple of external references and the reference resolves against
the predeclared global. For a contextual instance the dictionary is a
*constructor*, the reference is a call, and nothing defines it.

`Compiler::predeclare_instance_dictionary_globals` does not cover the gap: it
demands only classes some *visible* binding constrains, and in this harness
`type_env.visible_bindings()` is empty at Phase 2. Declaring a global for every
known instance closes the error, but the global is then never stored, which
turns a compile error into a run-time nil — worse, not better.

**Not a regression.** `Ord` has had this since Proposal 0179 Stage 8 made it
`Eq<a> => Ord<a>`; verified failing at `f8d8f585^`. Proposal 0183 moved `Num`
into the same position by giving it `Add` as a superclass, which is why
`tests/type_inference/constrained_type_params_integration.rs` now runs its two
`Num` cases through the driver, as the `Ord` case already did.

**Fix would be** to compile the prelude bodies in this harness, or to fall back
to direct dispatch when a unit cannot define the dictionary it references.

### KI-083 — A top-level `let` cannot call a constrained function

**Severity:** Medium · **Area:** Core lowering / VM · **Verified:** 2026-09-04 · **From:** Proposal 0183, R6

A top-level `let` whose initializer calls a function with a class bound fails at
run time. No generalization is involved — this reproduces on the current
compiler with the bound written out:

```flux
fn square<a: Num>(x: a) -> a { x * x }

let d = square(3)

fn main() with IO {
    print(d)
}
```

```
error[E1001]: Not A Function
Cannot call non-function value (got None).
  tl.flx:3:9
3 | let d = square(3)
  |         ^^^^^^
```

**Dictionary elaboration is not at fault.** The Core it produces is correct, and
passes the dictionary:

```
letrec square =
λ__dict_m8_466C6F772E4E756D_Num, x.
    let %t526 = __dict_m8_466C6F772E4E756D_Num.2
    %t526(x, x)

def d =
let %t527 = __dict_m8_466C6F772E4E756D_Num_Int(__dict_m8_466C6F772E416464_Add_Int)
  square(%t527, 3)
```

The failure is at run time: `square` is `None` when `d`'s initializer runs, so
the binding order between top-level value defs and rewritten `letrec` functions
is wrong. Isolating it:

| top-level `let` calls | dictionaries present | result |
|---|---|---|
| an unconstrained function (`x * x`) | no | works |
| an unconstrained, non-foldable function (`match` over a `List`) | no | works |
| an unconstrained function | yes | works |
| *(the constrained call moved inside `main`)* | yes | works |
| a constrained function, result annotated `Int` | yes | **fails** |
| a constrained function using `+` rather than `*` | yes | **fails** |
| **a constrained function** | yes | **fails** |

Row 3 rules out the prepended dictionary defs on their own; row 4 rules out the
constrained function on its own; row 2 rules out constant folding as the reason
the unconstrained cases pass. Neither annotating the result nor changing which
class is involved makes any difference.

So the failing combination is precisely *a top-level value def calling a
function that dictionary elaboration synthesized*.

**The callee is the dictionary constructor, not the user's function.** Tracing
the VM's `execute_call` at the point it rejects the callee:

```
[call] callee not a function: None num_args=1
```

One argument — so it is `__dict_..._Num_Int(__dict_..._Add_Int)`, not
`square(%t527, 3)`, which takes two. The reported span belongs to the enclosing
call, which is what made this look like a problem with `square`.

Established while investigating, to save the next attempt the detours:

- The VM path lowers through `lower_aether_program`, which carries its **own**
  copy of the seeding loop over `aether.defs()`. Instrumenting `lower_program`
  in the same file traces nothing.
- Lowering resolves the callee correctly: `bound_var` panics on a missing env
  entry, and it does not fire. The `None` is a *runtime* value, so a global slot
  was read before anything assigned it.
- `bind_function_id_in_items` returns `true` for the dictionary constructor, so
  a top-level item for it already exists. It is not the "synthesized function
  with no item" case.
- Binding the function's name to `IrExpr::MakeClosure(fn_id, [])` in the entry
  function, plus an `IrProgram.global_bindings` entry, **does not fix it** —
  and `IrProgram.global_bindings` appears never to be read by the VM backend.
  The compiler reads `symbol_table.global_bindings()`, which is a different
  structure. Whatever assigns a declared function's global slot is elsewhere,
  and that is the thing a synthesized function is missing.

The remaining question is narrow: *what assigns a top-level function's global
slot in the VM path, and why does a dictionary constructor miss it?*
`ir_lowering.rs` already special-cases `__dict_*` names to **define** their
symbols ("weren't predeclared during Phase 2, which only sees AST function
names") without giving them values, which is the strongest hint about where to
look.

`tests/parity/toplevel_pure_expression.flx` carries a comment describing the
same symptom for the native backend, so this is likely one bug seen from two
sides. It blocks Proposal 0183's R6, because generalizing unannotated
definitions turns almost every top-level helper into a constrained one.

### KI-081 — A class-method call emits its instance's context at variables nothing binds — FIXED 2026-09-04

**Severity:** Low · **Area:** Type classes / inference · **Verified:** 2026-09-04 · **From:** Proposal 0183, R6

Resolving a direct class-method call looks up the generated mangled `__tc_*`
function and instantiates its scheme, in order to constrain the caller's ambient
effect row against that function's row
([calls.rs](../src/ast/type_infer/expression/calls.rs),
`propagate_resolved_class_call_effects`). The instantiation also emitted the
scheme's *constraints* — the selected instance's context, `Eq<a>` for
`instance Eq<a> => Eq<List<a>>`.

Those constraints were unsolvable by construction. The instantiated signature is
used only for its effect row; its parameters are never unified with the call's
arguments, so the context landed on fresh variables nothing ever binds. Every
direct class-method call therefore left one predicate over `_` behind:

```flux
instance MyEq<a> => MyEq<List<a>> {
    fn meq(xs, ys) { true }
    fn mneq(xs, ys) { !meq(xs, ys) }     // one stuck MyEq<_> here
}

fn main() with IO {
    print(mneq([1], [1]))                // and one here
}
```

**Fixed 2026-09-04** by not emitting them. The obligation is not lost: the
call's predicate is emitted from the *argument* types by
`emit_class_method_predicate`, it resolves against that same instance, and
`solve_instance_evidence` checks the instance context as part of the evidence it
builds — so `mneq(["s"], ["s"])` with no `MyEq<String>` in scope is still
`E444`. Regression test
`a_class_method_call_enforces_its_instances_context` pins both directions.

The stdlib's stuck predicates fall from 11 to 9, with no diagnostic change
across all 1,305 programs in the repository.

### KI-080 — A match arm binds pattern variables against a fresh type, losing the scrutinee's type — FIXED 2026-09-04

**Severity:** Medium · **Area:** Type classes / inference · **Verified:** 2026-09-04 · **From:** Proposal 0183, R6

A contextual instance's method body raises obligations over a type variable
that is not the instance's own, so the context cannot discharge them. The
program still runs — dispatch resolves separately — but the predicates never
reach a terminal state, and they are the bulk of what
[Proposal 0183](proposals/0183_constraint_solver_terminal_states.md) is trying
to escalate.

```flux
class MyEq<a> {
    fn meq(x: a, y: a) -> Bool
}

instance MyEq<Int> {
    fn meq(x, y) { x == y }
}

instance MyEq<a> => MyEq<List<a>> {
    fn meq(xs, ys) {
        match xs {
            [h1 | t1] -> match ys {
                [h2 | t2] -> meq(h1, h2) && meq(t1, t2),
                _ -> false
            },
            _ -> true
        }
    }
}

fn main() with IO {
    print(meq([1, 2], [1, 2]))
}
```

Prints `true`, and leaves three predicates undischarged. Traced with
`FLUX_STUCK_TRACE=full`, and with the enclosing scope printed at the point
`classify_constraint` gives up:

```
scope=WholeProgram want=[Var(10827)]                 givens=[("MyEq", [Var(10821)])] quant=[10821, 10822]
scope=WholeProgram want=[App(List, [Var(10827)])]    givens=[("MyEq", [Var(10821)])] quant=[10821, 10822]
```

The implication is built correctly: the instance context *is* in scope as a
given, and the method's variables are quantified. But the body's `h1` has type
`Var(10827)` while the instance declared `Var(10821)`, and `10827` is not in the
quantified set at all. `entailed_by_givens` compares type arguments
syntactically, so `MyEq<10827>` cannot match `MyEq<10821>` and the predicate is
recorded stuck.

**Root cause — match-arm scrutinee isolation.** The instance machinery is not
at fault: the synthesized `__tc_*` function binds its parameters correctly
(`params=[Var(10822), App(List, [Var(10821)]), App(List, [Var(10821)])]`). The
connection is lost inside the `match`.

`arm_pattern_scrutinee_ty`
([control_flow.rs](../src/ast/type_infer/expression/control_flow.rs)) returns a
fresh fallback variable instead of the scrutinee's type whenever
`should_isolate_match_arm_scrutinees` says the arms disagree on pattern family:

```rust
if isolate_arm_scrutinees {
    self.alloc_fallback_var()
} else {
    scrutinee_ty.clone()
}
```

`match xs { [h | t] -> ..., _ -> ... }` mixes a `Cons` arm with a
non-constraining one, so each arm binds against a fresh variable that "unifies
with anything" — the comment on `should_isolate_match_arm_scrutinees` says so
outright. `h` therefore gets a variable unrelated to the element type of `xs`,
and every predicate raised on `h` is over a variable no context can discharge.

Reproduced with no instance machinery at all:

```flux
fn direct<a: MyEq>(x: a) -> Bool { meq(x, x) }

fn viapat<a: MyEq>(xs: List<a>) -> Bool {
    match xs {
        [h | t] -> meq(h, h),
        _ -> true
    }
}
```

```
[emit] MyEq [Var(10824)] origin=ExplicitBound   at 9:13     [emit] MyEq [Var(10824)] origin=MethodCall at 9:35
[emit] MyEq [Var(10827)] origin=ExplicitBound   at 11:13    [emit] MyEq [Var(10830)] origin=MethodCall at 13:19
```

`direct` raises its obligation over the *same* variable as its bound; `viapat`
raises it over a fresh one. The parameter is bound correctly
(`params=[App(List, [Var(10827)])]`) and the pattern then sees
`scrut=Var(10829)`.

**Why no program misbehaves today.** The declared bound is still emitted at each
call site, so a missing instance is caught there — `viapat(["s"])` reports
`E444` correctly. The stale predicate is redundant rather than unsound, which is
why this has gone unnoticed. It matters because it is most of what Proposal 0183
would escalate, and escalating it would produce errors on correct programs.

**Fixed 2026-09-04.** Only the head constructor and its arity decide a pattern
family — `List<a>` is as much a list as `List<Int>` — so the check that decides
whether the arms already agree no longer requires the scrutinee to be fully
concrete. `concrete_scrutinee_matches_family` became
`scrutinee_head_matches_family`: it resolves the scrutinee through the current
substitution and matches its head, and a scrutinee whose head is still unknown
(a bare variable) matches nothing and so continues to isolate, which is what
kept `Some` and `Left` arms from constraining one another in the first place.

The stdlib's stuck predicates fall from 15 to 11, and the reproduction above
loses both of its instance-body predicates, with **no diagnostic change across
all 1,305 programs** in the repository. Regression tests
`a_pattern_variable_keeps_the_scrutinees_element_type` and
`a_pattern_variable_from_a_concrete_scrutinee_is_still_checked` in
[typeclass_baseline_tests.rs](../tests/type_inference/typeclass_baseline_tests.rs)
cover both directions.

### KI-077 — Superclass evidence for a contextual superclass instance is built from the wrong dictionary — FIXED 2026-09-03

**Severity:** High · **Area:** Type classes / dictionary passing · **Verified:** 2026-09-03 · **From:** Phase 1 of the type-class audit

An instance whose superclass obligation is discharged by a *contextual*
instance builds its superclass slot from whichever context dictionary names the
same class, without checking that the type arguments agree:

```flux
class Base<a> { fn base(x: a) -> Int }
class Base<a> => Mid<a> { fn mid(x: a) -> Int }

instance Base<Int> { fn base(x) { x } }
instance Base<a> => Base<List<a>> { fn base(xs) { 9 } }
instance Base<a> => Mid<List<a>> { fn mid(xs) { base(xs) } }

fn call_mid<a: Mid>(x: a) -> Int { mid(x) }
fn main() with IO { print(call_mid([1, 2])) }   // E1000: want=4, got=2
```

`Mid<List<a>>` owes evidence for `Base<List<a>>`. Its context supplies
`Base<a>`, which is a different predicate, but `superclass_evidence_expr`
([dict_elaborate.rs](../src/core/passes/dict_elaborate.rs)) matches a context
entry on the class alone — `position(|&class_id| class_id == superclass)` — so
the `Base<a>` dictionary lands in the slot. The correct evidence is the
contextual instance applied to it: `__dict_Base_List<a>(__dict_Base)`.

Two things go wrong together, and the Core dump shows both:

```
def __dict_Mid_List<a> = λ__dict_Base. MakeTuple(__dict_Base, ...)
letrec __tc_Mid_List<a>_mid = λ__dict_Base, __dict_Base_1, __dict_Base, xs. ...
```

The tuple's leading slot holds the wrong dictionary, and the instance method was
given three dictionary parameters for one context predicate. The call therefore
fails on arity before the wrong evidence can be observed — which is the only
reason this is loud rather than silent.

Validation is not at fault: `validate_superclass_obligations_for`
([class_env.rs](../src/types/class_env.rs)) accepts the program correctly,
because `Base<List<a>>` really does have an instance. The defect is entirely in
evidence *construction*, and it has an AST-side twin: `dictionary_slot_names`
names `__dict_Base_List<a>` without applying its context.

The context-free spelling works — an instance that omits the `=>` context
altogether has its superclass slot filled from the plain dictionary of the
superclass instance, which `examples/type_classes/superclass_evidence_without_context.flx`
locks. Only the contextual case is broken.

### KI-015 — A class whose variable appears only in the return position cannot dispatch — FIXED

**Severity:** Medium · **Area:** Type classes / dispatch · **Verified fixed:** 2026-09-02 · **From:** [0179](proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)

Dispatch used to select an instance from the *first argument's* type, so a class
whose type variable appeared only in the return position resolved against the
parameter type and failed:

```flux
class Parse<a> { fn from_text(text: String) -> Result<a, String> }
instance Parse<Int> { fn from_text(text) { Ok(len(text)) } }

fn read_int(t: String) -> Result<Int, String> { from_text(t) }
// error[E444]: No instance for `Parse<String>`   ← the parameter type, not `Int`
```

**Fixed by Proposal 0179 Stage 4**, which was never recorded here. A class-method
call now derives its predicate from the positions the *class declaration* puts
its parameters in, reading each from whichever argument — or from the result —
actually carries it (`class_param_bindings`,
[class_predicate.rs](../src/types/class_predicate.rs); `try_resolve_class_call`,
[lower_ast/mod.rs](../src/core/lower_ast/mod.rs)). That subsumed and removed the
hardcoded `Decode.decode` special case this entry described, so the general rule
it asked for is the rule in force.

Verified 2026-09-02 on both backends: the repro above prints `4`; two instances
distinguished only by the expected result select correctly through a return type
*and* through a `let` annotation; and a polymorphic forwarder
`fn read_any<a: Parse>(t: String) -> Result<a, String>` resolves at its call
site. `examples/type_classes/result_directed_resolution.flx` locks the shape.

[KI-058](#ki-058) later extended the annotation channel to an annotation naming
the enclosing signature's own rigid parameter, which had converted to a nominal
type of the same name.

**One shape remains unresolvable, and is reported rather than guessed.** A
function constrained twice on one class, whose method is distinguished only by
its result, cannot say which of its two dictionaries it means — selection reads
argument types, and neither backend has the call's expected type. That is
`E485`, covered by
`examples/compiler_errors/result_directed_ambiguity_e485.flx`. The workaround
this entry recorded still applies there: reify the choice as a value, as
`Flume.Manifest` does with a `Reader<a>` record, which also composes further
than an instance head can (`array_of(element: Reader<a>) -> Reader<List<a>>`
needs no higher-kinded types).
