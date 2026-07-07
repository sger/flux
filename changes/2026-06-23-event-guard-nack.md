### Added
- **CML-style `Event.guard` / `Event.with_nack` real semantics** (proposal 0177
  T2.5), replacing the v0.0.6 placeholders in
  [lib/Flow/Event.flx](../lib/Flow/Event.flx).
  - `guard(f)` now defers building its event until **sync-time**: `f` runs once,
    the first time `sync` polls the node, and its result is memoized across
    re-polls (so side effects happen exactly once per sync). Backed by a new
    `EventGuard` primop with VM + native runtime support.
  - `with_nack(f)` now passes `f` a real **negative-acknowledgement** event that
    fires (yielding `()`) when the branch `f` builds *loses* the enclosing
    `choose` at sync commit — letting a loser clean up a partially-started
    operation. If that branch wins, the nack never fires (a fiber blocked on it
    parks until `run_async` teardown, as in Concurrent ML). Backed by a new
    `EventWithNack` primop and a 1-capacity channel the runtime signals on loss.

### Tests
- VM+native parity fixtures under `tests/parity/`:
  - `async_event_guard_defers.flx` — `guard`'s side effect lands at sync-time,
    after the build-time write (deferred, not eager).
  - `async_event_nack_fires_on_loss.flx` — nack **fires on loss**: a cleanup
    fiber records the sentinel.
  - `async_event_nack_silent_on_win.flx` — nack **stays silent on win**: the
    cleanup fiber parks and is torn down.
- `examples/async/24_event_composition.flx` — `guard` + `with_nack` + `choose`
  composition demo (both backends).
