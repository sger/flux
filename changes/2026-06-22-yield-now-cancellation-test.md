### Tests
- **`yield_now` cancellation-checkpoint confirmation** (proposal 0177 T2.3).
  New `tests/integration/vm_yield_now_cancel.rs` pins, under the seedable
  single-worker deterministic scheduler (T1.1, zero `sleep`), that a fiber doing
  a cooperative `yield_now` loop observes its enclosing scope's cancellation and
  stops at the yield point instead of running to completion. A `race` loser runs
  a finite tick-recording loop (bound 200) and is cancelled when the winner
  resolves; the test asserts it was curtailed to ~1 tick. The test genuinely
  isolates the checkpoint: with the `is_current_cancelled()` guard in
  `FiberYieldNow` removed, the same seeds (0, 7, 123) run the loser to the full
  200 ticks instead of ~1, so a regression that drops the checkpoint flips the
  assertion (and the finite bound means a regression fails loudly rather than
  hanging). Complements `vm_fiber_check_cancelled.rs`, which already covers the
  `check_cancelled` checkpoint.
