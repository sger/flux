### Added
- Added `Flow.Event` first-class events and `select { recv/send/after -> ... }` syntax for channel and timer selection on VM and native backends.

### Changed
- `select` is now a reserved keyword.

### Fixed
- `Flow.Event.sync` now yields between readiness polls instead of blocking the scheduler thread.
