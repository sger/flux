# Known Issues

A living log of confirmed, currently-unfixed bugs and limitations that are not
tied to a single in-flight proposal. Unlike `changes/` fragments (which are
consumed into `CHANGELOG.md` at release), entries here persist until the issue is
fixed — at which point the entry is **moved to the "Resolved" section** with the
fixing commit/PR, then pruned at the next release.

Each entry: ID, status, area, reachability (how likely real code hits it),
a minimal repro, root cause (if known), and the proposed fix direction.

---

## Open

_All surfaced while building the Actor MVP (proposal 0177 M4), which stresses
forked fibers that suspend on channels through library-provided closures. None
is caused by the actor layer; each is a pre-existing native-backend or
effect-system limitation the pattern is the first to exercise together._

_(No open backend issues. KI-4/KI-5/KI-6 moved to Resolved below.)_

### Effect-system / surface-syntax limitations (not backend bugs)

These are language limitations, confirmed empirically, that shaped the Actor MVP
design. Tracked here so they are not rediscovered.

- **L1 — A `with` clause accepts only `Label | rowvar` or a single alias, never
  two concrete labels.** `fn f() with Actor | Async` and `with Console | Clock`
  fail to parse (`E034`); the whole stdlib expresses multi-label rows through
  **aliases** (`IO`, `Async`). Consequence: a new capability that must co-occur
  with `Async` has to ship as an alias, not a bare label.
- **L2 — An effect **alias** is carried as an opaque atom through a higher-order
  boundary and never decomposes/discharges there.** `handle ActorCap {}` strips a
  concrete label from a multi-label row at a *direct* call site, but when a
  `with Actor`-typed function is passed as a value and invoked via `fork`, the
  `Actor` atom rides through and cannot be discharged — forcing it up to `main`.
  This is why proposal 0177 T4.1's distinct `Actor` capability label is not yet
  achievable; the Actor MVP uses `with Async` + a value-carried capability
  (`Mailbox<msg>`) instead.
- **L3 — A function *parameter* cannot be forwarded through a Flux-level
  effect-polymorphic wrapper.** `fn spawn(body) { fork(s, fn(){ body() }) }` fails
  `E422` (the `body` obligation leaks past the enclosing `with Async`), whereas
  passing the parameter **straight to a primop** (as `both`/`first_of` do)
  type-checks. Consequence: `Flow.Actor.spawn` calls the fork primop directly and
  cannot delegate to `spawn_sized`.
- **L4 — A module-qualified **generic** type does not parse in data-constructor or
  parameter type position.** `data M { M(Channel.Channel<Int>) }` and
  `fn f(c: Channel.Channel<Int>)` fail (`E034`); the type must be imported
  unqualified (`exposing (Channel)`), which is why `Flow.Actor` double-imports
  `Flow.Channel` (aliased for calls per KI-4, exposed for the type).

---

## Resolved

_(Move entries here with the fixing commit/PR when closed; prune at release.)_

- **KI-6 — Cancelling a fiber blocked on channel `recv`, then exiting
  `run_async` immediately, corrupted the boundary** (originally reported as a
  `channel N not found` error; the same defect also surfaced as `run_async`
  returning a stale `None` instead of the root's value, and as the cancelled
  fiber's `Canceled` error escaping to `main` as an opaque `E1009
  __fiber_error__` — `Flow.Actor.stop` on a `receive`-looping actor right
  before program exit was the user-facing shape. Native: same shape SIGSEGV'd.)
  - **Root cause (VM):** the dispatch loop's inner ready-queue drain kept
    ticking fibers *after the root fiber had completed*. `cancel(scope)`
    re-queues a recv-parked fiber as ready/`Cancelled`; when the root then
    finished in the same drain pass (cancel **immediately** before returning),
    the cancelled fiber's continuation was resumed on the shared worker-0 VM
    whose stack the root's final tick had already unwound to the `run_async`
    boundary. The post-root resume trashed the boundary state — which stale
    artifact surfaced (clobbered result, phantom fiber error, freed-channel
    lookup) depended on the shape. This is exactly why "cancel then `sleep`
    then exit" always worked: with the root parked, the cancelled fiber's
    cleanup resume ran against a boundary-consistent VM.
  - **Fix (2026-07-09):** once the root has produced its result, the dispatch
    loop ticks no further fibers — `dispatch_loop`'s ready drain breaks on
    `root_result`, and the multi-worker `worker_0_dispatch_loop` drain breaks
    on `shared.is_finished()` ([src/vm/core_dispatch.rs](../../src/vm/core_dispatch.rs)).
    Remaining fibers are reaped by `exit_run_async`, exactly as they already
    were when the root exited while children were still parked. (Semantics
    note: a fiber cancelled in the boundary's final drain pass no longer runs
    its cleanup arms — consistent with the existing behavior for children
    still parked at root exit.)
  - **Native:** the SIGSEGV on this shape no longer reproduces after the
    KI-4/KI-5 yield-check fix (the cancellation path through `receive` now
    suspends properly); pinned by the native regression test below.
  - **Regression tests:**
    [tests/integration/vm_cancel_teardown.rs](../../tests/integration/vm_cancel_teardown.rs)
    (fork-cancel-return, actor-stop-return, looped + multi-worker) and
    `actor_stop_then_immediate_return_native` in
    [tests/native_llvm/native_actor_mvp_tests.rs](../../tests/native_llvm/native_actor_mvp_tests.rs).

- **KI-5 / KI-4 — Native: a bare `exposing`-imported cross-module async call
  missed its yield check** (one bug, two reported shapes; resolved together).
  - **KI-5 as reported:** a fiber body invoked through a function value
    (`spawn(body)` → `fork(s, fn(){ body(mb) })`) that suspends on
    `Flow.Actor.receive` computed on the **yield sentinel** instead of parking:
    the single-`receive` actor replying `x + 100` printed `105` (sentinel
    `10 >> 1 = 5`, plus 100) instead of `141`; a forced suspend rendered a raw
    pointer as `"<value>"`. **KI-4 as reported:** `import Flow.Channel exposing
    (..)` then a bare `recv(ch)` / `send(ch, v)` SIGSEGV'd, while qualified
    `Channel.recv(ch)` worked. VM unaffected in both.
  - **Root cause:** `LirProgram::async_extern_symbols` — the KI-1 data-driven
    cross-module async classification — was populated only in
    `lower_program_with_interner_and_externs`, the **Core**-lowering entry point,
    which the native pipeline never invokes with extern symbols (dead path). The
    per-module native pipeline (`Compiler::lower_to_lir_llvm_module_per_module` →
    `lower_aether_program_with_interner_and_externs`, the **Aether** twin) never
    populated the set, so `DirectExtern` async classification silently degraded
    to the hardcoded `Flow.*` allowlist (`is_direct_async_extern_symbol`). Any
    suspending import off the allowlist — bare exposed `recv`/`send`,
    `Flow.Actor.receive` — got **no `flux_is_yielding` check** at its call site
    (confirmed by disassembly: `flux_once_once` called
    `flux_Flow_Actor_receive` and consumed the return with no check), so when
    the callee suspended, the caller ran on with the sentinel.
  - **Why the shapes misled:** only **bare** names of interface-loaded library
    modules lower to `DirectExtern` (no binder → extern-map hit). *Qualified*
    member calls (`Channel.recv(...)`) lower as closure loads + **indirect**
    calls, which get yield checks via the caller's own async effect row
    (`async_effect_binders` → `CallKind::Indirect { async_capable }`) — hence
    "qualified works". Sibling *user*-module imports carry HM binders and also
    take the indirect path — hence the KI-1 regression tests kept passing while
    the per-module gap was live, and KI-5 looked "indirect-call-specific"
    (the actor was simply the first *bare* exposed suspending import in a
    position whose resume value was consumed).
  - **Fix (2026-07-08):** populate `async_extern_symbols` on the aether lowering
    entry point too — shared helper `record_async_extern_symbols`
    ([src/lir/lower.rs](../../src/lir/lower.rs)), called by both
    `lower_program_with_interner_and_externs` and
    `lower_aether_program_with_interner_and_externs`.
  - **Regression tests** (fail pre-fix, pass post-fix):
    `native_exposing_imported_channel_intrinsic_gets_yield_check` (KI-4 shape)
    and `native_actor_receive_gets_yield_check` (KI-5 shape) in
    `tests/native_llvm/native_async_cross_module_tests.rs`.
  - **Note:** qualified channel calls in `lib/Flow/Actor.flx` were a KI-4
    workaround and are now optional; the `exposing (Channel)` double-import is
    still required by L4 (qualified generic types don't parse in type position).

- **KI-2 — Effectful call duplicated in a `drop x in (let r = f(...); r)` tail-return.**
  On the native (LLVM) backend, an effectful call bound and returned bare in tail
  position, immediately after a dropped binding, was lowered to **two** calls (the
  first result discarded), double-executing `f`. `--dump-core` showed one call but
  `--dump-aether`/`--dump-lir` showed two (`let r = RHS in RHS`). The VM was
  unaffected.
  - **Trigger:** an **unused binding** (→ a `Drop`) immediately before a
    `let r = f(...); r` whose body returns the bound var **bare in tail
    position**. The generic/cross-module framing in the original report was
    incidental — the essential trigger is the `Drop` wrapping the let, which is the
    only entry to the reuse-token threader. (Observably masked for the `print`
    effect, which executed once on both backends; a non-idempotent effect in this
    position was the latent risk.)
  - **Root cause:** the reuse pass, **not** the Core inliners. In
    `rewrite_drop_body_with_env`
    ([src/aether/reuse_analysis.rs](../../src/aether/reuse_analysis.rs)), the `Let`
    case recorded the binding as an alias (`r → rhs`, gated by
    `is_safe_precompute_rhs`, which treats effectful `App`/`AetherCall` as safe),
    and the `Var` case then followed that alias and returned the rewritten rhs **as
    the body expression** — while the enclosing `let r = rhs` binding was retained,
    producing `let r = rhs in rhs`. (The original entry's "an aether pass
    copy-propagates the tail var without eliminating the binding" was morally
    correct; it just misattributed the pass to the inliners.)
  - **Fix (2026-07-03):** the `Var` case now returns the substituted alias **only
    when following it actually yields a reuse**; otherwise it keeps the original
    `Var(r)`, so the call is emitted once. Pinned via per-pass call-count
    instrumentation (count 3→5 exactly at `reuse::insert_reuse_aether`). The
    legitimate precompute-let-to-`Con` reuse path
    (`aether_call_precompute_let_can_still_reuse`) is unaffected. Regression:
    `reuse_analysis::tests::effectful_let_returned_bare_is_not_duplicated` (fails
    without the fix). Runnable demo:
    `examples/aether/regression_ki2_reuse_drop_let/` (inspect with `--dump-lir`).
    See `changes/2026-07-03-reuse-drop-let-var-duplication.md`.
  - **Refs:** `changes/2026-06-23-native-async-resume-followups.md`.

- **KI-1 — User-defined cross-module `async` functions miss native yield checks.**
  On the native (LLVM) backend, a call into a user-defined `async` function
  defined in another module emitted **no `flux_is_yielding` check**, so when the
  callee suspended the caller dereferenced the yield sentinel → SIGSEGV. The VM
  was unaffected.
  - **Root cause:** cross-module/extern async-ness was decided by a hardcoded
    allowlist, `is_direct_async_extern_symbol`
    ([src/lir/mod.rs](../../src/lir/mod.rs)), which only listed the `Flow.*`
    library combinators. A user-defined async function in another module was
    never on the list, so its call site was treated as non-suspending. (Within a
    single module, `direct_async_func_ids` already classified correctly via the
    local call graph; the gap was purely cross-module.)
  - **Fix (2026-07-03):** async-ness is now **data-driven** from the callee's
    known effect row instead of the allowlist. `ImportedNativeSymbol` gains an
    `is_async` flag ([src/lir/lower.rs](../../src/lir/lower.rs)), populated in
    `Compiler::build_native_extern_symbols` from each imported member's cached
    type scheme (`scheme_effect_row_is_async`, matching the `Async` alias and its
    `Suspend`/`Fork`/`GetContext`/`AsyncFail` expansion). At LIR-lowering the
    suspend-capable mangled symbols are collected into
    `LirProgram::async_extern_symbols`, which `call_kind_is_direct_async` now
    consults **alongside** the allowlist — so `direct_async_func_ids`,
    `promote_tail_calls`, and `cont_split` all see cross-module async call sites.
    The allowlist is retained as a fallback for prim/value-getter symbols that
    carry no scheme. Regression:
    `tests/native_llvm/native_async_cross_module_tests.rs` (the exact repro plus
    a direct `sleep`-suspending variant, both asserted VM==native==`7`/`6`).
    Runnable demo: `examples/async/regression_ki1_cross_module_async/`. See
    `changes/2026-07-03-cross-module-async-yield-check.md`.
  - **Refs:** surfaced landing proposal 0177 T2.5; see
    `changes/2026-06-23-native-async-resume-followups.md`.

- **KI-3 — Native heap-use-after-free: composed-continuation double-drop.**
  On native, an async op that re-yields on reactor I/O from within a **multi-cont
  (deep) composed continuation** running inside a `both`/`fork` child fiber
  crashed with `SIGSEGV`. First seen via `Flow.Http.get` (all `examples/http/*`
  and any `Http.get` inside `both`), but bisected — via an AddressSanitizer build
  (no debugger was available) — to a bug with no HTTP involved.
  - **Root cause:** in the `flux_compose` trampoline
    ([runtime/c/effects.c](../../runtime/c/effects.c) `flux_compose_trampoline_closure_entry`),
    when a composed continuation re-yields mid-resume it carries its *remaining*
    conts into the next composed continuation via `flux_yield_extend`. Those
    conts were read as **borrows** (`flux_array_get` does not dup) and stored
    into the next continuation's array (`flux_array_new` memcpies, also no dup),
    so the same cont closures were owned by **two** continuation arrays at a
    single refcount. When the just-executed continuation was released after the
    suspend (`native_abi::release_executed_work`), it dropped those conts to
    zero and freed them — leaving the *next* continuation dangling → UAF on
    resume/finish (ASan: `heap-use-after-free` in `rc_load_relaxed`, freed at
    `release_executed_work`, re-freed at `release_finished_fiber`). Only
    reachable for **multi-cont** continuations: the single-cont fast path in
    `flux_compose_conts` returns the cont directly and never builds the shared
    array, which is why *depth* was the trigger and shallow I/O was fine.
    `FLUX_WORKERS=1` masked it (timing) and every native HTTP test pinned
    `with_worker_count(1)`.
  - **Fix (2026-07-02):** the trampoline now `flux_dup`s each carried `outer`
    cont before `flux_yield_extend`, so ownership is balanced across the two
    continuation arrays. One line + comment in `runtime/c/effects.c`; no Rust /
    scheduler change. Verified UAF-free under ASan with no new leaks (leak
    profile matches a baseline-passing program). Regression:
    `tests/native_llvm/native_http_client_tests.rs::native_http_client_get_under_both_multiworker_no_uaf`
    (HTTP `get` inside `both` under `with_worker_count(4)`), plus the now-gated
    `tests/parity/http_get_roundtrip.flx`. See
    `changes/2026-07-02-async-parity-gate-fixtures.md`. Related invariant: the
    work-stealing counterpart in commit 2b2965bc.

- **Native `Flow.Event.with_nack` crash (T2.5).** Cross-module async call missed
  its yield check: expanded-row async functions were misclassified
  (`effect_expr_contains_async`) and `with_nack` was absent from the async
  allowlist. Fixed 2026-06-23 (src/lir/lower.rs, src/lir/mod.rs); native nack now
  matches the VM. See `changes/2026-06-23-native-async-resume-followups.md`.
