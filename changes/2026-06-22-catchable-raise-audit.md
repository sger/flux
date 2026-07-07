### Changed
- **Catchable-raise audit** (proposal 0177 T2.2). Confirmed `Async.fail` is a
  genuine catchable raise end-to-end on both backends — `try` recovers it as
  `Err(err)` with the `AsyncError` payload intact, and an unwrapped raise
  propagates to the enclosing `scope`/await, including across `both`/`race` and
  forked children. `bail_if_cancelled` is now `if check_cancelled() { fail(Canceled) }`
  built on that real raise; its shim semantics (a stand-in from before catchable
  raise existed in 0174 slice 2-vi) are retired. Updated the stale comments/docs
  that still framed the `check_cancelled` + `fail` composition as "becomes
  catchable in a later slice" ([core_dispatch.rs](../../src/vm/core_dispatch.rs)
  `FiberCheckCancelled`, [lib/Flow/Async.flx](../../lib/Flow/Async.flx) `fail` /
  `bail_if_cancelled` docs). No behavioral change — the raise machinery was
  already real; this closes out the audit and removes the misleading framing.

### Tests
- New parity fixture `tests/parity/async_fail_catchable.flx` (vm + llvm): a
  direct `try(fail)`, a raise carrying a multi-field payload
  (`ProtocolError(503, "upstream busy")`), and a forked-child `fail` (via `both`)
  caught at the scope — each recovered as `Err(...)` byte-identically on both
  backends. Complements the pre-existing `async_try_panic.flx` (which already
  covered `fail(Canceled)` → `Err(Canceled)`).
