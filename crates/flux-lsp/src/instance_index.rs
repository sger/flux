//! Per-snapshot index of `instance` blocks for goto-definition on
//! class-method calls.
//!
//! Keyed by `(class_name, head_type_ctor, method_name)` so a dispatch
//! tuple from
//! [`flux::ast::type_infer::ClassDispatch`](../../../../src/ast/type_infer/mod.rs)
//! resolves in O(1) to the matching `InstanceMethod`'s spans. The same
//! triple is what the type checker uses to mangle `__tc_*` scheme
//! names — see `propagate_resolved_class_call_effects` in
//! [src/ast/type_infer/expression/calls.rs](../../../../src/ast/type_infer/expression/calls.rs).
//!
//! Today only the current snapshot's program contributes entries.
//! Cross-module instance dispatch (instances in Flow prelude or user
//! modules) is a follow-up — extending `build` to also walk
//! `snapshot.module_programs` is the natural next step.

use std::collections::HashMap;

use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;

/// `(class_name, head_type_ctor, method_name) → Entry`. Built once per
/// snapshot from `Statement::Instance` blocks.
pub struct InstanceIndex {
    by_key: HashMap<(Identifier, Identifier, Identifier), Entry>,
}

#[derive(Clone)]
pub struct Entry {
    /// Whole `instance ... { ... }` block — `target_range` in the LSP
    /// `LocationLink`. Lets the peek view show the surrounding
    /// instance context (class + head type + sibling methods).
    pub full_span: FluxSpan,
    /// The method-name position inside the instance block —
    /// `target_selection_range`, the precise focus the cursor lands on.
    pub focus_span: FluxSpan,
}

impl InstanceIndex {
    /// Walk every `Statement::Instance` block; emit one entry per
    /// declared method. Instances whose first type argument isn't a
    /// `TypeExpr::Named` (e.g. tuple or function shapes) are skipped —
    /// the type checker's dispatch path can still match them, but the
    /// LSP-side key isn't well-defined without a head identifier.
    pub fn build(program: &Program) -> Self {
        let mut by_key = HashMap::new();
        for stmt in &program.statements {
            let Statement::Instance {
                class_name,
                type_args,
                methods,
                ..
            } = stmt
            else {
                continue;
            };
            let Some(head) = head_ctor_of(type_args.first()) else {
                continue;
            };
            for m in methods {
                by_key.insert(
                    (*class_name, head, m.name),
                    Entry {
                        full_span: stmt.span(),
                        focus_span: m.span,
                    },
                );
            }
        }
        Self { by_key }
    }

    pub fn lookup(
        &self,
        class: Identifier,
        head: Identifier,
        method: Identifier,
    ) -> Option<&Entry> {
        self.by_key.get(&(class, head, method))
    }
}

fn head_ctor_of(t: Option<&TypeExpr>) -> Option<Identifier> {
    match t? {
        TypeExpr::Named { name, .. } => Some(*name),
        _ => None,
    }
}
