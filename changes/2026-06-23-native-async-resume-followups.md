### Fixed
- **Native `with_nack` (and cross-module async classification) — missing yield
  checks at cross-module async call sites** (proposal 0177 T2.5). On the native
  (LLVM) backend a call into an async function could skip its `flux_is_yielding`
  yield check, so when the callee suspended the caller dereferenced the yield
  sentinel (SIGSEGV). Two gaps caused it, both fixed:
  - **Expanded-row async functions misclassified** ([src/lir/lower.rs](../src/lir/lower.rs),
    `effect_expr_contains_async`): a function performing `Async` was only treated
    as async when its effect row literally contained the `Async` *alias*.
    `Flow.Event.with_nack` is written with the **expanded** seam labels
    (`Suspend, Fork, GetContext, AsyncFail` — required because the alias does not
    unify in a cross-module higher-order position), so its *indirect* call to the
    callback got no yield check. Now the four seam labels are recognized too.
  - **`with_nack` absent from the cross-module async allowlist**
    ([src/lir/mod.rs](../src/lir/mod.rs), `is_direct_async_extern_symbol`): a
    *caller in another module* (e.g. user code calling `Event.with_nack`) needs to
    know the callee can suspend. Added `Flow_Event_with_nack` alongside the
    existing `Flow_Event_sync` / `run_selected` / `event_wait_id` entries.
    (`Flow_Event_guard` is deliberately absent — its thunk is pure, so it never
    suspends.)

  Verified: native `with_nack` now matches the VM (the three event parity
  fixtures `async_event_guard_defers`, `async_event_nack_fires_on_loss`,
  `async_event_nack_silent_on_win` pass on vm + llvm), with no regressions across
  the native async / fiber / effect-parity suites.

### Docs
- **Known limitation — user-defined cross-module async functions.** Native
  cross-module async classification still relies on the hardcoded
  `is_direct_async_extern_symbol` allowlist (covering the `Flow.*` library
  combinators). An arbitrary *user-defined* `async` function called from a
  different module is not on that list, so its call site gets no yield check and
  crashes if it actually suspends. This is pre-existing and architectural — the
  proper fix is data-driven async-ness carried in module export metadata (the
  code comment on `direct_async_func_ids` notes "higher-order closure-call
  metadata is a later phase"). A related narrow codegen quirk (an effectful call
  in a generic cross-module function can be duplicated when its result is a
  let-bound value returned directly in tail position after an unused binding) is
  masked by the same gap and does not affect the `Flow.*` combinators. Tracked as
  a separate follow-up; it does not affect T2.5.
