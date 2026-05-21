### Changed
- `type_infer`: split two over-budget functions in
  `src/ast/type_infer/expression/calls.rs` and reformatted the file, so
  `cargo fmt --all --check` and the `type_infer_function_complexity_budget`
  guard pass again. `infer_function_call`'s typed-path class-constraint emission
  moved to `emit_typed_class_method_constraint`, and the instance/scheme lookup
  inside `propagate_resolved_class_call_effects` moved to
  `resolve_class_method_instance`. Pure extraction — class-method inference,
  effect propagation, and the LSP class-method dispatch entry are unchanged.
