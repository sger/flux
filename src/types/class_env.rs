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
    INSTANCE_EXTRA_METHOD, MISSING_SUPERCLASS_INSTANCE,
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
    /// Per-method type parameters (e.g., `<a, b>` on `fn fmap<a, b>`).
    pub type_params: Vec<Identifier>,
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
        let diagnostics = env.collect_from_statements(statements, interner);
        (env, diagnostics)
    }

    /// Collect class, instance, and deriving declarations from statements
    /// into this (possibly pre-populated) environment.
    pub fn collect_from_statements(
        &mut self,
        statements: &[Statement],
        interner: &Interner,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        Self::collect_classes(statements, self, &mut diagnostics, interner);
        Self::collect_instances(statements, self, &mut diagnostics, interner);
        Self::collect_deriving(statements, self, &mut diagnostics, interner);
        diagnostics
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
                            type_params: m.type_params.clone(),
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

                    // Check for duplicate instances (same class + same head type).
                    // Uses structural equality ignoring source spans.
                    let is_duplicate = env.instances.iter().any(|existing| {
                        existing.class_name == *class_name
                            && existing.type_args.len() == type_args.len()
                            && existing
                                .type_args
                                .iter()
                                .zip(type_args.iter())
                                .all(|(a, b)| a.structural_eq(b))
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

                    // Validate: no extra methods beyond what the class declares.
                    for method in methods {
                        let is_known = class_def.methods.iter().any(|m| m.name == method.name);
                        if !is_known {
                            let display_class = interner.resolve(*class_name);
                            let display_method = interner.resolve(method.name);
                            let known_methods: Vec<String> = class_def
                                .methods
                                .iter()
                                .map(|m| interner.resolve(m.name).to_string())
                                .collect();
                            diagnostics.push(
                                diagnostic_for(&INSTANCE_EXTRA_METHOD)
                                    .with_span(method.span)
                                    .with_message(format!(
                                        "`{display_method}` is not a method of class `{display_class}`."
                                    ))
                                    .with_hint_text(format!(
                                        "`{display_class}` declares: {}",
                                        known_methods.join(", ")
                                    )),
                            );
                        }
                    }

                    // Validate superclass instances exist.
                    // If class Ord has superclass Eq, then instance Ord<Int>
                    // requires instance Eq<Int> to already exist.
                    for superclass in &class_def.superclasses {
                        let super_class_name = superclass.class_name;
                        let super_display = interner.resolve(super_class_name);
                        let type_display: Vec<String> =
                            type_args.iter().map(|t| t.display_with(interner)).collect();
                        let type_display_str = type_display.join(", ");

                        let has_super_instance = env.instances.iter().any(|inst| {
                            if inst.class_name != super_class_name {
                                return false;
                            }
                            let inst_types: Vec<String> =
                                inst.type_args.iter().map(|t| t.display_with(interner)).collect();
                            inst_types.join(", ") == type_display_str
                        });

                        if !has_super_instance {
                            let display_class = interner.resolve(*class_name);
                            diagnostics.push(
                                diagnostic_for(&MISSING_SUPERCLASS_INSTANCE)
                                    .with_span(*span)
                                    .with_message(format!(
                                        "No instance for `{super_display}<{type_display_str}>` \
                                         (required by `{display_class}<{type_display_str}>`)."
                                    ))
                                    .with_hint_text(format!(
                                        "`{display_class}` requires `{super_display}` as a superclass. \
                                         Add: `instance {super_display}<{type_display_str}> {{ ... }}`"
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

    /// Collect derived instances from `deriving` clauses on data declarations.
    fn collect_deriving(
        statements: &[Statement],
        env: &mut ClassEnv,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for stmt in statements {
            match stmt {
                Statement::Data {
                    name,
                    deriving,
                    span,
                    ..
                } if !deriving.is_empty() => {
                    for class_name in deriving {
                        // Check that the class exists
                        if !env.classes.contains_key(class_name) {
                            let class_display = interner.resolve(*class_name);
                            let type_display = interner.resolve(*name);
                            diagnostics.push(
                                diagnostic_for(&INSTANCE_UNKNOWN_CLASS)
                                    .with_span(*span)
                                    .with_message(format!(
                                        "Cannot derive `{class_display}` for `{type_display}`: \
                                         no class `{class_display}` is defined."
                                    )),
                            );
                            continue;
                        }

                        // Register a derived instance (no method bodies —
                        // the constraint solver just needs to know it exists).
                        let type_arg = builtin_type(*name);
                        env.instances.push(InstanceDef {
                            class_name: *class_name,
                            type_args: vec![type_arg],
                            context: vec![],
                            method_names: vec![],
                            span: *span,
                        });
                    }
                }
                Statement::Module { body, .. } => {
                    Self::collect_deriving(&body.statements, env, diagnostics, interner);
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

    /// Given a method name, find which class it belongs to.
    /// Returns `(class_name, &ClassDef)` if the method is declared in any class.
    pub fn method_to_class(&self, method_name: Identifier) -> Option<(Identifier, &ClassDef)> {
        for (&class_name, class_def) in &self.classes {
            if class_def.methods.iter().any(|m| m.name == method_name) {
                return Some((class_name, class_def));
            }
        }
        None
    }

    /// Return the positional index of a method within its class definition.
    ///
    /// This canonical ordering is used for both dictionary construction
    /// (which methods go at which tuple position) and method extraction
    /// (which `TupleField` index to use).
    pub fn method_index(&self, class_name: Identifier, method_name: Identifier) -> Option<usize> {
        let class_def = self.classes.get(&class_name)?;
        class_def.methods.iter().position(|m| m.name == method_name)
    }

    /// Resolve a class instance for a concrete type name (e.g., "Int", "String").
    /// Matches against the first `type_arg` of each instance declaration.
    pub fn resolve_instance_for_type(
        &self,
        class_name: Identifier,
        type_name: &str,
        interner: &Interner,
    ) -> Option<&InstanceDef> {
        self.instances.iter().find(|inst| {
            inst.class_name == class_name
                && inst.type_args.first().is_some_and(|ta| {
                    if let TypeExpr::Named { name, args, .. } = ta {
                        args.is_empty() && interner.resolve(*name) == type_name
                    } else {
                        false
                    }
                })
        })
    }

    /// Register built-in type classes and instances.
    ///
    /// These are "phantom" entries — no real method bodies. They exist so the
    /// constraint solver can verify operator usage at compile time without
    /// users writing explicit class/instance declarations.
    pub fn register_builtins(&mut self, interner: &mut Interner) {
        let eq = interner.intern("Eq");
        let ord = interner.intern("Ord");
        let num = interner.intern("Num");
        let show = interner.intern("Show");
        let semigroup = interner.intern("Semigroup");

        let eq_method = interner.intern("eq");
        let compare_method = interner.intern("compare");
        let add_method = interner.intern("add");
        let sub_method = interner.intern("sub");
        let mul_method = interner.intern("mul");
        let show_method = interner.intern("show");
        let append_method = interner.intern("append");

        let int_name = interner.intern("Int");
        let float_name = interner.intern("Float");
        let string_name = interner.intern("String");
        let bool_name = interner.intern("Bool");

        let a_param = interner.intern("a");

        // ── Class definitions ──────────────────────────────────────────

        // Eq: eq(a, a) -> Bool
        self.register_builtin_class(eq, a_param, vec![
            MethodSig { type_params: vec![], name: eq_method, param_types: vec![], return_type: builtin_type(bool_name), arity: 2 },
        ]);

        // Ord: compare(a, a) -> Int
        self.register_builtin_class(ord, a_param, vec![
            MethodSig { type_params: vec![], name: compare_method, param_types: vec![], return_type: builtin_type(int_name), arity: 2 },
        ]);

        // Num: add(a, a) -> a, sub(a, a) -> a, mul(a, a) -> a
        self.register_builtin_class(num, a_param, vec![
            MethodSig { type_params: vec![], name: add_method, param_types: vec![], return_type: builtin_type(a_param), arity: 2 },
            MethodSig { type_params: vec![], name: sub_method, param_types: vec![], return_type: builtin_type(a_param), arity: 2 },
            MethodSig { type_params: vec![], name: mul_method, param_types: vec![], return_type: builtin_type(a_param), arity: 2 },
        ]);

        // Show: show(a) -> String
        self.register_builtin_class(show, a_param, vec![
            MethodSig { type_params: vec![], name: show_method, param_types: vec![], return_type: builtin_type(string_name), arity: 1 },
        ]);

        // Semigroup: append(a, a) -> a
        self.register_builtin_class(semigroup, a_param, vec![
            MethodSig { type_params: vec![], name: append_method, param_types: vec![], return_type: builtin_type(a_param), arity: 2 },
        ]);

        // ── Instance definitions ───────────────────────────────────────

        // Eq instances: Int, Float, String, Bool
        for ty in [int_name, float_name, string_name, bool_name] {
            self.register_builtin_instance(eq, ty);
        }

        // Ord instances: Int, Float, String
        for ty in [int_name, float_name, string_name] {
            self.register_builtin_instance(ord, ty);
        }

        // Num instances: Int, Float
        for ty in [int_name, float_name] {
            self.register_builtin_instance(num, ty);
        }

        // Show instances: Int, Float, String, Bool
        for ty in [int_name, float_name, string_name, bool_name] {
            self.register_builtin_instance(show, ty);
        }

        // Semigroup instances: String
        self.register_builtin_instance(semigroup, string_name);
    }

    /// Register a single built-in class definition.
    fn register_builtin_class(
        &mut self,
        name: Identifier,
        type_param: Identifier,
        methods: Vec<MethodSig>,
    ) {
        // Don't override user-declared classes.
        if self.classes.contains_key(&name) {
            return;
        }
        self.classes.insert(name, ClassDef {
            name,
            type_param,
            superclasses: vec![],
            methods,
            default_methods: vec![],
            span: Span::default(),
        });
    }

    /// Register a single built-in instance.
    fn register_builtin_instance(&mut self, class_name: Identifier, type_name: Identifier) {
        // Don't duplicate if user already declared this instance.
        let expected = builtin_type(type_name);
        let already_exists = self.instances.iter().any(|i| {
            i.class_name == class_name
                && i.type_args.first().is_some_and(|t| t.structural_eq(&expected))
        });
        if already_exists {
            return;
        }
        self.instances.push(InstanceDef {
            class_name,
            type_args: vec![builtin_type(type_name)],
            context: vec![],
            method_names: vec![],
            span: Span::default(),
        });
    }
}

/// Create a simple named TypeExpr for built-in type references.
fn builtin_type(name: Identifier) -> TypeExpr {
    TypeExpr::Named {
        name,
        args: vec![],
        span: Span::default(),
    }
}
