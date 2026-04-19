### Added

- Bidirectional type checker for annotated bindings (Proposal 0159). New `check_expression` dispatcher in `src/ast/type_infer/expression/checked.rs` with specialised rules for `If`, `Match`, `DoBlock`, and `Lambda`. Propagates the expected type into sub-expressions so branch/arm/body mismatches report at the offending sub-expression span instead of only at the outer annotation.
- Rigid skolem type variables for declared signature parameters. `InferCtx` gains `skolem_vars` state and `mark_skolem` / `unmark_skolems` helpers; `infer_function_declaration` marks declared type parameters for the duration of body inference. `unify_core` rejects binding a skolem to a non-identical type via a new `UnifyErrorKind::RigidBind`, threaded through all unification helpers via a `skolems: &HashSet<TypeVarId>` parameter.
- `E305` (RIGID TYPE VARIABLE ESCAPE): new diagnostic emitted when a declared type parameter would be unified with a concrete type inside the function body. Uses the source-level parameter name in the message.
- Recursive-group pre-binding with declared polymorphic schemes. `declared_fn_scheme` builds a scheme from a complete explicit signature (all parameters + return type annotated) so recursive call sites instantiate at fresh types rather than collapsing through `Scheme::mono(fresh)`. Wired in `infer_program` (top level) and `infer_module` (annotation-gated — unannotated module helpers remain non-predeclared).
- Higher-order call-site check mode. When an argument is a lambda and the expected parameter type has fully-concrete parameters, `infer_call_higher_order_path` propagates via `check_expression`, surfacing body-level mismatches at the offending sub-expression. Non-lambda args silently unify against the expected type so later lambdas see resolved callee type variables.
- Investigation note `docs/internals/proposal_0159_investigation.md` documenting the `Scheme::mono(fresh_var)` polymorphism-collapse that motivated annotation-gated module Phase A.

### Changed

- `advance_counter_past_preloaded_schemes`: `InferCtx::new` now bumps the type-var counter past any TypeVarId used in preloaded schemes so freshly-allocated vars in a later pass cannot collide with IDs baked into cross-pass scheme bodies. Prevents fallback-var expansion in `resolve_binding_schemes` from tainting quantified parameters of preloaded schemes.
- `seen_error_keys` dedup gate: bypasses the concreteness check for `RigidBind` errors since the expected side is necessarily a type variable.
- Typed-`let` bindings whose initializer is an `If`, `Match`, or `DoBlock` now run `check_expression` in addition to the canonical `let_annotation_type_mismatch` E300, emitting per-branch diagnostics at the offending sub-expression while preserving the pinned canonical diagnostic.

### Fixed

- Snapshot acceptance for ~20 `type_mismatch_argument*` / `adversarial__*` / `type_system__failing__*` fixtures whose call-argument diagnostics now report narrower spans reflecting the offending sub-expression.

### Docs

- `changes/2026-04-17-signature-directed-checking.md` — this fragment.
- `docs/internals/proposal_0159_investigation.md` — pin-down of the module Phase A predeclaration collapse documented during Commit 0 of the phased delivery.
