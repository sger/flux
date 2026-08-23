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

### KI-001 — A `let` read inside a `match` arm is `Uninit` after the match

**Severity:** High · **Area:** VM backend, CFG lowering · **Verified:** 2026-08-22 on `038f51a8`

On the VM backend, a `let` binding **read inside a statement-position `match`
arm** is `Uninit` for the rest of the function. Correct inside the arm, corrupt
after it. The LLVM backend is correct, so this is also a parity divergence.

```flux
fn main() with IO {
    let n = 42
    match Some(1) {
        Some(x) -> println(to_string(n)),   // "42"       — correct
        None -> println("none"),
    }
    println(to_string(n))                   // "<uninit>"  — no error
}
```

The type decides how it fails. `String` aborts with `E1009: Cannot add String
and Uninit values`; **`Int` prints `<uninit>` with no diagnostic at all.** The
silent case is why this is High: a program produces wrong output and nothing
says so.

The trigger is narrow — it needs *both* a read in the arm *and* statement
position:

| Shape | Result |
|---|---|
| arm does not read the binding | fine |
| `let r = match ...` (bound to a let) | fine |
| same shape with `if`/`else` | fine |
| `do { }` block vs single-expression arm | no difference — both break |

Suspected liveness / register-allocation defect in VM-only CFG lowering
(`src/cfg/`), in how statement-position match arms merge.

**Workaround:** extract the arm body into a helper taking the value as a
parameter.

**Note:** the shape `let dir = ...; match Fs.list_dir(dir) { ... };
Fs.remove_dir_all(dir)` is common in filesystem code, so this costs real
debugging time — it looks like a bug in the code under test.

### KI-002 — `println` prints nothing for cons lists and arrays

**Severity:** Medium · **Area:** VM runtime · **Verified:** 2026-08-22

`println(xs)` on a list or array produces no output. `to_string(xs)` and
`print(xs)` both work, so the value and its formatter are fine — the defect is
in `println`'s dispatch for collection values.

**Workaround:** `println(to_string(xs))`.

### KI-003 — The bare `contains` builtin returns false on primop-returned arrays

**Severity:** Medium · **Area:** VM runtime / Base builtins · **Verified:** 2026-08-22

The bare `contains` builtin returns `false` for arrays produced by a primop
(such as `Fs.list_dir`), even when the element is present. `Array.contains` from
`Flow.Array` is correct on the same value.

Suggests the builtin is matching on a narrower representation than the one
primops return.

**Workaround:** use `Flow.Array.contains`.

### KI-004 — Native subprocess execution is POSIX-only

**Severity:** Medium · **Area:** Native backend, `Flow.Process` · **Verified:** 2026-08-22 · **From:** [0178](proposals/implemented/0178_os_capabilities_for_tooling.md) Q8

`Flow.Process.run` spawns via `posix_spawnp` in the C runtime. The Windows
branch returns an `IoError` (`ENOSYS`) instead of spawning. The **VM** backend
works on Windows, since it goes through Rust's `std::process::Command`.

This is the only deliberate behavioural difference between the two backends in
the OS-capability surface, and it must close before Windows is a supported
target for Flux tooling.

### KI-005 — `Flow.Fs` is not async-aware

**Severity:** Medium · **Area:** `Flow.Fs`, `Flow.Async` · **From:** [0178](proposals/implemented/0178_os_capabilities_for_tooling.md) Q5

`Flow.Fs` operations are blocking. `Flow.Async` already maintains a filesystem
thread pool (`fs_pool`), and [0174](proposals/0174_async_effect_concurrency.md)
documents blocking calls inside a fiber as a known hazard — so calling `Flow.Fs`
from a fiber stalls a scheduler worker.

Unresolved: whether `Flow.Fs` should route through `fs_pool` automatically,
expose async variants, or keep blocking semantics and document the hazard.

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

### KI-008 — The stdlib is found via a CWD-relative `lib/Flow`

**Severity:** High · **Area:** Driver, tooling · **Tracked by:** [0177](proposals/0177_package_manager.md)

`inject_flow_prelude` (`src/driver/frontend.rs`) resolves the stdlib relative to
the current directory, and `find_project_root` keys on `Cargo.toml`. **Flux
therefore only runs from inside this checkout**, which blocks installing it as a
tool or shipping a binary. Proposal 0177 tracks the fix.

### KI-009 — TCP operations block the fiber scheduler

**Severity:** Medium · **Area:** Runtime, `Flow.Tcp` · **Tracked by:** [0174](proposals/0174_async_effect_concurrency.md)

TCP operations use blocking stdlib calls with no fiber-scheduler integration, so
concurrent TCP tests are not yet possible. Needs the mio reactor wiring.

### KI-010 — The test suite is flaky under load: tests share one on-disk cache

**Severity:** High · **Area:** Test harness · **Verified:** 2026-08-22

`cargo test --all --all-features` intermittently fails targets that pass in
isolation. The failures move between runs and look unrelated to each other,
which makes them read as separate bugs. They are one bug.

**Cause.** 34 test files drive the `flux` binary against a shared
`target/test-scratch/` and the shared compilation cache under `target/`. Many do
**not** pass `--no-cache`, so concurrent targets read and write each other's
`.flxi` interfaces and bytecode. Anything else touching `target/` at the same
time — another `cargo` invocation, an editor build — widens the window.

Two distinct symptoms, both from this cause:

1. **A compiler assertion escapes as a test failure.**
   ```
   parallel VM compilation failed: missing global mapping for local index 26
   ```
   Raised by `module_linker.rs:141` when a module's instructions reference a
   global slot no cache binding covers. `synthetic_top_level_temp_bindings`
   (`compiler/mod.rs`) exists to prevent exactly this, so reaching it means the
   cache was read in a state that function did not anticipate.

2. **A native fixture produces no output at all**, so the harness cannot parse a
   summary:
   ```
   no native summary for stdlib_either.flx:
   ```
   The `*_native_tests` targets are the slowest in the suite (25s–165s each) and
   rebuild native artifacts, so they lose these races most often.

**How to tell it apart from a real failure.** A contention failure finishes in
~0.1s (nothing ran) and passes when the target is run alone. A real failure
takes normal time and reproduces in isolation. **Always re-run a suspect target
by itself before believing it.**

**Partially fixed (2026-08-22).** `tests/support/scratch.rs` provides a
`Scratch` guard giving each test a unique directory (pid + counter) and, via
`cache_args()`, its own `--cache-dir`. Converted so far: the shared
`stdlib_fixture` runner — which covers every `stdlib_*` fixture on both
backends — plus `cross_module_named_fields`, `module_local_shadowing`,
`qualified_class_method_dispatch`, and the `stdlib_{io,path,result,env,process}`
runners.

Two things that fix which `--no-cache` alone did not:

- **`--no-cache` does not isolate native builds.** The native backend writes
  shared artifacts under the cache root regardless, which is why
  `*_native_tests` targets lost these races most often despite passing
  `--no-cache`.
- **A test that *exercises* caching cannot use `--no-cache`.**
  `field_order_survives_the_warm_module_cache` needs a cold run followed by a
  warm one; it now gets a private cache instead of sharing the repo-wide one.

`run_fixture` also folds stderr into its returned text on failure, so a native
compile or link error is visible instead of surfacing as an unexplained
`no native summary for <fixture>`.

**Still outstanding:** `stdlib_fs_tests`, `stdlib_crypto_tests`,
`native_constructor_tag_tests`, and the `tests/flux/*.flx` fixtures hardcode
paths like `target/test-scratch/flux_fs_rename` inside **Flux source**, so they
cannot be redirected from the Rust side alone. Each such path is currently
unique to one test, so they do not collide with each other — but a future test
reusing a name would reintroduce the problem silently.

**Meanwhile:** re-run any suspect target on its own before believing it.

### KI-011 — Re-wrapping `Err(e)` into a `Result` with a different success type fails inference

**Severity:** Medium · **Area:** HM inference · **Verified:** 2026-08-23

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

---

### KI-013 — A recursive parser grammar over a recursive ADT crashes the native backend

**Severity:** High · **Area:** LLVM backend · **Verified:** 2026-08-23

`Flume.Toml` parses correctly on the VM and crashes on the native backend with
`signal 5` (SIGTRAP) as soon as the input contains an integer, an array, an
inline table, or a dotted key. Boolean and string values parse fine on both.

```sh
cargo run --features llvm -- --test --native tests/flux/flume_toml.flx
# 58 tests: 21 passed, 37 failed
#   native program terminated by signal 5 (signal): native program crashed
```

The crash is **not** a stack limit: `depth(10000)` runs natively without
trouble. It is also not reproduced by any reduction attempted so far —
`separated_by` alone, mutual recursion between two parser constructors via
`lazy`, and a recursive ADT built by a recursive grammar all work natively in
isolation. The trigger is some combination of these that only the full
`Flume.Toml` grammar exhibits, so the minimal reproduction is currently the
module itself.

**Impact:** `tests/flume/flume_toml_tests.rs` and `flume_manifest_tests.rs` run
their fixtures on the VM only, rather than through `assert_backends_agree`.
`flume_resolve` and `flume_version` pass on both backends and still assert
parity. Restore the parity assertion in both harnesses when this is fixed.

**Found alongside a bug this work did fix:** ordering comparisons on strings
compared heap *addresses* on the native backend, because `flux_rt_lt/le/gt/ge`
in `runtime/c/flux_rt.c` handled only floats and ints and fell through to
`flux_untag_int` on a string pointer. `"x" >= "a"` was `false` natively and
`true` on the VM. Fixed by adding a `flux_string_cmp` lexicographic path to all
four, matching the VM's byte ordering.

---

### KI-014 — A constructor imported from another module infers as a type variable

**Severity:** High · **Area:** HM inference / module interfaces · **Verified:** 2026-08-23

A constructor applied in a module other than the one declaring its ADT gets an
unresolved type variable rather than the ADT type, so anything keyed on that
type fails. Class dispatch is the visible casualty:

```flux
import Flume.Value as Value
import Flume.Value exposing (Toml, TString)

class Render<a> { fn render(value: a) -> String }
instance Render<Int>  { fn render(value) { to_string(value) } }
instance Render<Toml> { fn render(value) { Value.render_toml(value) } }

render(42)             // "42"
render(TString("hi"))  // error[E1009] panic: No instance of Render.render ...
```

`hm_expr_types` holds an entry for `TString("hi")`, but its value is
`Var(_)` — inference never concretised it. Dispatch then has a type variable
where it needs a constructor, and `resolve_method_call_instance_from_first_arg`
correctly declines to guess.

The same class *does* dispatch when the value arrives from a function call
rather than a directly-applied imported constructor
(`describe(Resolve.from_root())` works), which locates the gap in constructor
scheme import rather than in dispatch itself. Related to the
`preloaded_adt_constructor_types` plumbing that fixed cross-module *named-field*
constructors; the positional-application path appears not to be covered.

**Workaround:** wrap the construction in a local function whose return type is
annotated, or reify the dispatch as an explicit record of functions — which is
what `Flume.Manifest`'s `Reader<a>` does.

---

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

## Resolved

Entries move here with the resolving commit rather than being deleted, so
existing `#KI-nnn` references still explain themselves.

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
