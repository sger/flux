### Changed
- **`race` / `first_of` tie-break contract pinned** (proposal 0177 T2.4).
  Decision: `race` stays **2-way** and delegates the n-way case to `first_of` /
  `first` (`race(f, g)` ≡ `first([f, g])`); both share one deterministic
  **source-order tie-break**. When several branches are simultaneously
  *runnable* (ready in the same cooperative round, including across `yield_now`),
  the earliest in source order wins; a later branch wins only once every earlier
  branch is *suspended on a real async wait* (`sleep` / I/O). Documented on
  `race` and `first_of` in [lib/Flow/Async.flx](../../lib/Flow/Async.flx). No
  behavioral change — this records and locks the existing
  `AwaitCoordinator::resolve_race` / `resolve_first_of` semantics (the
  `blocked_by_earlier_ready` rule).

### Tests
- New `tests/integration/vm_race_tiebreak.rs` locks the contract under the
  seedable single-worker deterministic scheduler (T1.1) by sweeping 11 seeds and
  asserting the tie-break holds for *every* one — proving it is decided by source
  order, not scheduling luck: (a) a 2-way `race` immediate tie always returns the
  first branch; (b) an n-way `first_of` immediate tie always reports index 0; and
  (c) an earlier-source branch that `yield_now`s once still beats an
  immediately-ready later branch (a regression would flip this to the later
  branch). Complements `vm_fiber_first_of.rs`, which covers the sleep-driven
  fastest-wins path the deterministic scheduler does not virtualize.
