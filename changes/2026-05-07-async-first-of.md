## Added

- Added `Flow.Async.first_of` and `Flow.Async.first` over non-empty lists of
  async thunks.
- Added the `FiberFirstOf` core primitive with VM and LLVM/native scheduler
  implementations that return `(winning_index, value)`, cancel losers, and
  preserve source-order FIFO ties for immediate children.
- Added VM, native LLVM, and parity coverage for fastest-child selection,
  source-order immediate ties, and loser cancellation.

## Changed

- Marked proposal 0174 Phase 2 slice 2-ii as landed and corrected the stale
  Phase 1a `Sendable` progress row to point at the closed 2-x ADT derivation
  audit.
