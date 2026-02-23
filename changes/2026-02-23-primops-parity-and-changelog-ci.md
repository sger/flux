### Added
- Expanded primop coverage with additional Flux examples and VM/JIT parity test scenarios.

### Changed
- Primop execution and routing paths were aligned across compiler, VM, and JIT for consistent behavior.
- Primop-related tests were updated to match current diagnostic/error wording.

### Fixed
- VM/JIT primop parity failures caused by stale expected error-message substrings.
- Inconsistent primop error assertions in phase-2 parity tests (`contains`, `concat`, `delete`).

### Performance
- Improved fast-path consistency for primop calls by reducing divergence between VM and JIT execution behavior.

### Docs
- Updated primop documentation and examples to reflect the current primop surface and behavior.
