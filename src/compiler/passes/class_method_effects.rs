//! Proposal 0151, Phase 4a: instance-method effect floor validation.
//!
//! When a `class` method declares a `with` clause, that row is treated
//! as a *floor*: every conforming `instance` method must declare an
//! effect row that is a superset of the class row. This pass walks the
//! program after `collect_class_declarations` has populated `class_env`
//! and emits **E452** for each missing concrete effect on an instance
//! method.
//!
//! ## Why floor semantics
//!
//! See `docs/proposals/0151_module_scoped_type_classes.md` (Phase 4 —
//! Reading 2). The class row pins the *minimum* effect surface every
//! call site must be ready to handle when dispatching through the
//! class. Instances may add more effects beyond the floor; the resolved
//! row is propagated to the caller during type inference (Phase 4b).
//!
//! ## Scope of this pass
//!
//! Only the *concrete-named* effects from the class row are enforced
//! here. Row variables (`|e`) are not part of the floor — they describe
//! caller-side openness, not a required effect atom. Subtractions on
//! either side are normalized through `EffectExpr::normalized_names`.

use std::collections::HashSet;

use crate::diagnostics::{
    DiagnosticBuilder, compiler_errors::INSTANCE_METHOD_EFFECT_FLOOR, diagnostic_for,
};
use crate::syntax::{
    Identifier,
    program::Program,
    statement::Statement,
    type_class::{ClassMethod, InstanceMethod},
};

use super::super::Compiler;

impl Compiler {
    /// Walk the program and fire E452 for each instance method whose
    /// declared effect row is missing a concrete effect listed on the
    /// matching class method.
    pub(in crate::compiler) fn validate_class_method_effect_floor(
        &mut self,
        program: &Program,
    ) {
        self.walk_for_floor(&program.statements);
    }

    fn walk_for_floor(&mut self, statements: &[Statement]) {
        // Collect class methods (by class name → method name → ClassMethod)
        // for every class declared at this scope. We don't need a proper
        // ClassId here because the instance head names the class textually,
        // and the rule is local to the (class, method) pair.
        let mut class_methods: std::collections::HashMap<
            Identifier,
            std::collections::HashMap<Identifier, ClassMethod>,
        > = std::collections::HashMap::new();

        for stmt in statements {
            if let Statement::Class { name, methods, .. } = stmt {
                let entry = class_methods.entry(*name).or_default();
                for m in methods {
                    entry.insert(m.name, m.clone());
                }
            }
        }

        // Walk instances and check the floor.
        for stmt in statements {
            if let Statement::Instance {
                class_name,
                methods,
                ..
            } = stmt
            {
                // Resolve the class either from the local scope (this
                // module/file) or from the global class_env. Local wins
                // because the textual name binds against in-scope decls.
                let local = class_methods.get(class_name);
                for instance_method in methods {
                    let class_method_local = local.and_then(|m| m.get(&instance_method.name));
                    if let Some(class_method) = class_method_local {
                        self.check_floor(class_method, instance_method);
                        continue;
                    }
                    // Fallback: look up via class_env for cross-module classes.
                    let cloned_effects =
                        self.class_env
                            .lookup_class(*class_name)
                            .and_then(|class_def| {
                                class_def
                                    .methods
                                    .iter()
                                    .find(|s| s.name == instance_method.name)
                                    .map(|s| s.effects.clone())
                            });
                    if let Some(effects) = cloned_effects {
                        self.check_floor_against_sig(&effects, instance_method);
                    }
                }
            }
        }

        // Recurse into module bodies.
        for stmt in statements {
            if let Statement::Module { body, .. } = stmt {
                self.walk_for_floor(&body.statements);
            }
        }
    }

    fn check_floor(&mut self, class_method: &ClassMethod, instance_method: &InstanceMethod) {
        self.check_floor_against_sig(&class_method.effects, instance_method);
    }

    fn check_floor_against_sig(
        &mut self,
        class_effects: &[crate::syntax::effect_expr::EffectExpr],
        instance_method: &InstanceMethod,
    ) {
        if class_effects.is_empty() {
            return;
        }
        let class_concrete: HashSet<Identifier> = class_effects
            .iter()
            .flat_map(|e| e.normalized_names())
            .collect();
        let instance_concrete: HashSet<Identifier> = instance_method
            .effects
            .iter()
            .flat_map(|e| e.normalized_names())
            .collect();

        for missing in class_concrete.difference(&instance_concrete) {
            let method_name = self.interner.resolve(instance_method.name).to_string();
            let effect_name = self.interner.resolve(*missing).to_string();
            let diag = diagnostic_for(&INSTANCE_METHOD_EFFECT_FLOOR)
                .with_span(instance_method.span)
                .with_message(format!(
                    "Instance method `{method_name}` is missing class-declared effect \
                     `{effect_name}`."
                ))
                .with_hint_text(format!(
                    "Add `{effect_name}` to the instance method's `with` clause, or remove \
                     it from the class declaration if it should not be required."
                ));
            self.errors.push(diag);
        }
    }
}
