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

### KI-009 — TCP operations block the fiber scheduler

**Severity:** Medium · **Area:** Runtime, `Flow.Tcp` · **Tracked by:** [0174](proposals/0174_async_effect_concurrency.md)

TCP operations use blocking stdlib calls with no fiber-scheduler integration, so
concurrent TCP tests are not yet possible. Needs the mio reactor wiring.

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

**Severity:** High · **Area:** Driver, tooling · **Verified:** 2026-08-23 · **From:** [0177](proposals/0177_package_manager.md)

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
