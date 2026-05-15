### Added

- LSP goto-definition now resolves class-method calls (e.g. `show(42)`) to the matching `instance Show<Int> { fn show(...) { ... } }` arm rather than the `class Show { ... }` declaration. F12 lands on the precise instance method implementation, mirroring rust-analyzer's trait-method navigation.
- New `class_method_dispatch: HashMap<ExprId, ClassDispatch>` field on [`InferProgramResult`](src/ast/type_infer/mod.rs) keyed by the function-position `ExprId`. Populated inside `propagate_resolved_class_call_effects` ([src/ast/type_infer/expression/calls.rs](src/ast/type_infer/expression/calls.rs)) after the type checker resolves the instance — the same resolution step that emits class constraints and mangles `__tc_*` scheme names, just persisted instead of consumed in-place. `ClassDispatch` carries `(class_name, head_type_ctor, method_name)`.
- New LSP module [`crates/flux-lsp/src/instance_index.rs`](crates/flux-lsp/src/instance_index.rs) — `InstanceIndex` built per snapshot from `Statement::Instance` blocks, keyed by `(class_name, head_type_ctor, method_name)` for O(1) lookup of `(full_span, focus_span)`. Stored on `Snapshot.instance_index`.
- New `pub fn collect_classes_for_lsp(&mut self, program: &Program)` on `Compiler` ([src/compiler/passes/reset.rs](src/compiler/passes/reset.rs)) and its `lsp_support` wrapper, so the LSP can populate `class_env` from a buffer's class declarations without invoking the full `phase_collection`. The snapshot calls it in `run_inference` ([crates/flux-lsp/src/snapshot.rs](crates/flux-lsp/src/snapshot.rs)) before `build_infer_config_for_program`.
- Three integration tests asserting end-to-end class-method goto-def: `goto_definition_resolves_class_method_to_instance` (single instance lands on instance arm, not class decl), `goto_definition_class_method_dispatch_picks_correct_instance` (two instances on different head types each route to the right arm), `goto_definition_class_method_falls_through_when_dispatch_unavailable` (polymorphic receiver — no panic, graceful fallthrough).

### Changed

- `ResolvedClassMethodCall` ([src/ast/type_infer/expression/calls.rs](src/ast/type_infer/expression/calls.rs)) now carries `function_expr_id: ExprId` in addition to `first_arg_id`. The LSP keys `class_method_dispatch` by the function-position id because that's the `ExprId` the locator's `NodeRef::Expr(Expression::Identifier { ... })` carries when the cursor is on the method name.
- Dispatch recording now also fires in the unresolved-callee path of `infer_function_call`, not just the typed-callee path. Buffer-declared class methods aren't pre-bound in HM env (their short name `same` isn't in `preloaded_base_schemes`), so their calls take the unresolved-callee path; without this change, the dispatch resolution would be discarded for in-buffer class methods.
- `Compiler::collect_class_declarations` is now `pub(in crate::compiler)` so submodules under `compiler/passes/` can call it.

### Closed deferrals

- "Instance-method goto-def" deferral noted in `changes/2026-05-15-lsp-keyword-and-goto-def-coverage.md` is now landed for intra-buffer instances. Cross-module instance dispatch (Flow prelude / user modules) remains a follow-up — extending `InstanceIndex::build` to also walk `snapshot.module_programs` is the natural next step.
