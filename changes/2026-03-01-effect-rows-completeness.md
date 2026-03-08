### Added

- Effect-row constraint solver (`src/bytecode/compiler/effect_rows.rs`): `EffectRow`, `RowConstraint`, `RowSolution`, and `solve_row_constraints` implementing set-based row arithmetic with var binding, link propagation, and worklist-based resolution.
- New error codes `E419` (unresolved single effect variable), `E420` (ambiguous multiple effect variables), `E421` (invalid effect subtraction), `E422` (unsatisfied effect subset) with deterministic sorted diagnostics.
- Pass fixtures: `100_effect_row_order_equivalence_ok.flx`, `101_effect_row_subtract_concrete_ok.flx`, `102_effect_row_subtract_var_satisfied_ok.flx`, `103_effect_row_multivar_disambiguated_ok.flx`, `104_effect_row_absent_ordering_linked_ok.flx`.
- Fail fixtures: `194_effect_row_multi_missing_deterministic_e400.flx`, `195_effect_row_invalid_subtract_e421.flx`, `196_effect_row_subtract_unresolved_single_e419.flx`, `197_effect_row_subtract_unresolved_multi_e420.flx`, `198_effect_row_subset_unsatisfied_e422.flx`, `199_effect_row_subset_ordered_missing_e422.flx`, `200_effect_row_absent_ordering_linked_violation_e421.flx`.

### Changed

- `collect_effect_row_constraints` and `collect_effect_expr_absence_constraints` in `expression.rs` integrate the new solver for all call-site effect-row validation (subset checks, absence constraints, unresolved-var detection).
- CI manifest (`ci/examples_manifest.tsv`) extended with all new pass/fail fixtures (tier 2, both VM and JIT).

### Docs

- Proposals `0042` and `0049` marked `Implemented | have` in `docs/proposals/0000_index.md` with full closure evidence.
- `examples/type_system/README.md` and `examples/type_system/failing/README.md` updated with new fixture entries and 0049 run-command section.
