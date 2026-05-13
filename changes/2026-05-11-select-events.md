### Added
- Added `Flow.Event` first-class events and `select { recv/send/after -> ... }` syntax for channel and timer selection on VM and native backends.

### Changed
- `select` is now a reserved keyword.

### Fixed
- `Flow.Event.sync` now suspends on readiness notifications instead of blocking the scheduler thread or waking every millisecond.
- Native event slots are reused after a committed event tree is freed, so long-running select loops no longer grow the event table monotonically.

### Notes
- `Event.sync` consumes the committed event tree; build a fresh event before syncing again, and do not share a sub-event across choices after one choice commits.
- `select` arm bodies may be effectful. Internally, event wrappers pick a thunk and the thunk runs after the event commits.
- `Event.guard` is a placeholder and currently evaluates its function at event construction time, not sync-time.
- `with_nack` remains a placeholder; the nack event does not fire yet.
