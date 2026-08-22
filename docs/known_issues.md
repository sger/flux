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

---

## Resolved

_None yet. Move entries here with the resolving commit rather than deleting
them, so existing `#KI-nnn` references still explain themselves._
