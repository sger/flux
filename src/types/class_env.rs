//! Type class environment — collects and validates `class` and `instance`
//! declarations from the AST.
//!
//! Built during the collection phase (before type inference). The class
//! environment will later be used by the constraint solver to resolve
//! type class constraints and by dictionary elaboration to generate code.

use std::collections::HashMap;

use crate::{
    diagnostics::{Diagnostic, DiagnosticBuilder, diagnostic_for, position::Span},
    syntax::{
        Identifier,
        interner::Interner,
        statement::Statement,
        type_class::ClassConstraint,
        type_expr::TypeExpr,
    },
};

use super::super::diagnostics::compiler_errors::{
    DUPLICATE_CLASS, DUPLICATE_INSTANCE, INSTANCE_MISSING_METHOD, INSTANCE_UNKNOWN_CLASS,
};

/// A type class definition collected from a `class` declaration.
#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: Identifier,
    pub type_param: Identifier,
    pub superclasses: Vec<ClassConstraint>,
    pub methods: Vec<MethodSig>,
    /// Methods that have default implementations in the class body.
    pub default_methods: Vec<Identifier>,
    pub span: Span,
}

/// A method signature within a class definition.
#[derive(Debug, Clone)]
pub struct MethodSig {
    pub name: Identifier,
    pub param_types: Vec<TypeExpr>,
    pub return_type: TypeExpr,
    pub arity: usize,
}

/// An instance definition collected from an `instance` declaration.
#[derive(Debug, Clone)]
pub struct InstanceDef {
    pub class_name: Identifier,
    pub type_args: Vec<TypeExpr>,
    pub context: Vec<ClassConstraint>,
    pub method_names: Vec<Identifier>,
    pub span: Span,
}

/// The class environment — registry of all declared classes and instances.
///
/// Built from the program AST during the collection phase. Provides lookup
/// and validation for downstream phases (constraint generation, solving,
/// dictionary elaboration).
#[derive(Debug, Clone, Default)]
pub struct ClassEnv {
    /// class name → class definition
    pub classes: HashMap<Identifier, ClassDef>,
    /// All instance definitions (validated against their class)
    pub instances: Vec<InstanceDef>,
}

impl ClassEnv {
    /// Create a new empty class environment.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a `ClassEnv` from a program's top-level statements.
    /// Returns the environment and any validation diagnostics.
    pub fn from_statements(
        statements: &[Statement],
        interner: &Interner,
    ) -> (Self, Vec<Diagnostic>) {
        let mut env = ClassEnv::new();
        let mut diagnostics = Vec::new();

        // First pass: collect all class declarations
        Self::collect_classes(statements, &mut env, &mut diagnostics, interner);

        // Second pass: collect and validate instance declarations
        Self::collect_instances(statements, &mut env, &mut diagnostics, interner);

        (env, diagnostics)
    }

    /// Collect class declarations recursively (handles modules).
    fn collect_classes(
        statements: &[Statement],
        env: &mut ClassEnv,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for stmt in statements {
            match stmt {
                Statement::Class {
                    name,
                    type_params,
                    superclasses,
                    methods,
                    span,
                } => {
                    if env.classes.contains_key(name) {
                        let display_name = interner.resolve(*name);
                        diagnostics.push(
                            diagnostic_for(&DUPLICATE_CLASS)
                                .with_span(*span)
                                .with_message(format!(
                                    "Type class `{display_name}` is already defined."
                                )),
                        );
                        continue;
                    }

                    let type_param = type_params.first().copied().unwrap_or(*name);

                    let method_sigs: Vec<MethodSig> = methods
                        .iter()
                        .map(|m| MethodSig {
                            name: m.name,
                            param_types: m.param_types.clone(),
                            return_type: m.return_type.clone(),
                            arity: m.params.len(),
                        })
                        .collect();

                    let default_methods: Vec<Identifier> = methods
                        .iter()
                        .filter(|m| m.default_body.is_some())
                        .map(|m| m.name)
                        .collect();

                    env.classes.insert(
                        *name,
                        ClassDef {
                            name: *name,
                            type_param,
                            superclasses: superclasses.clone(),
                            methods: method_sigs,
                            default_methods,
                            span: *span,
                        },
                    );
                }
                Statement::Module { body, .. } => {
                    Self::collect_classes(&body.statements, env, diagnostics, interner);
                }
                _ => {}
            }
        }
    }

    /// Collect instance declarations and validate against known classes.
    fn collect_instances(
        statements: &[Statement],
        env: &mut ClassEnv,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for stmt in statements {
            match stmt {
                Statement::Instance {
                    class_name,
                    type_args,
                    context,
                    methods,
                    span,
                } => {
                    // Check that the class exists
                    let class_def = match env.classes.get(class_name) {
                        Some(def) => def,
                        None => {
                            let display_name = interner.resolve(*class_name);
                            diagnostics.push(
                                diagnostic_for(&INSTANCE_UNKNOWN_CLASS)
                                    .with_span(*span)
                                    .with_message(format!(
                                        "No type class `{display_name}` is defined."
                                    ))
                                    .with_hint_text(format!(
                                        "Declare the class first: `class {display_name}<a> {{ ... }}`"
                                    )),
                            );
                            continue;
                        }
                    };

                    // Check for duplicate instances (same class + same head type)
                    let is_duplicate = env.instances.iter().any(|existing| {
                        existing.class_name == *class_name
                            && format!("{:?}", existing.type_args) == format!("{:?}", type_args)
                    });
                    if is_duplicate {
                        let display_class = interner.resolve(*class_name);
                        let display_type: Vec<String> =
                            type_args.iter().map(|t| t.display_with(interner)).collect();
                        diagnostics.push(
                            diagnostic_for(&DUPLICATE_INSTANCE)
                                .with_span(*span)
                                .with_message(format!(
                                    "Duplicate instance for `{display_class}<{}>`.",
                                    display_type.join(", ")
                                )),
                        );
                        continue;
                    }

                    // Validate: all required methods are implemented
                    let method_names: Vec<Identifier> =
                        methods.iter().map(|m| m.name).collect();

                    for required in &class_def.methods {
                        let has_impl = method_names.contains(&required.name);
                        let has_default = class_def.default_methods.contains(&required.name);
                        if !has_impl && !has_default {
                            let display_class = interner.resolve(*class_name);
                            let display_method = interner.resolve(required.name);
                            diagnostics.push(
                                diagnostic_for(&INSTANCE_MISSING_METHOD)
                                    .with_span(*span)
                                    .with_message(format!(
                                        "Missing method `{display_method}` in instance `{display_class}`."
                                    ))
                                    .with_hint_text(format!(
                                        "`{display_class}` requires: fn {display_method}(...)"
                                    )),
                            );
                        }
                    }

                    env.instances.push(InstanceDef {
                        class_name: *class_name,
                        type_args: type_args.clone(),
                        context: context.clone(),
                        method_names,
                        span: *span,
                    });
                }
                Statement::Module { body, .. } => {
                    Self::collect_instances(&body.statements, env, diagnostics, interner);
                }
                _ => {}
            }
        }
    }

    /// Look up a class definition by name.
    pub fn lookup_class(&self, name: Identifier) -> Option<&ClassDef> {
        self.classes.get(&name)
    }

    /// Find all instances for a given class.
    pub fn instances_for(&self, class_name: Identifier) -> Vec<&InstanceDef> {
        self.instances
            .iter()
            .filter(|i| i.class_name == class_name)
            .collect()
    }
}
