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

_(none)_

---

## Resolved

_(Move entries here with the fixing commit/PR when closed; prune at release.)_

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
