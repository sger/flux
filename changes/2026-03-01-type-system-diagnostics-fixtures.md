### Fixed
- Hardened strict type/effect diagnostics for unresolved `perform` argument paths (locked with new failing fixture `192_perform_arg_unresolved_strict_e425.flx`).
- Added regression coverage for unreachable pattern-arm warnings via new fixture `193_unreachable_pattern_arm_w202.flx`.

### Changed
- Extended example fixture manifest and snapshot coverage to keep these diagnostics/warnings stable in CI.
