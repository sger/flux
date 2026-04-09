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
        Identifier, interner::Interner, statement::Statement, type_class::ClassConstraint,
        type_expr::TypeExpr,
    },
    types::{
        class_id::{ClassId, ModulePath},
        infer_type::InferType,
        type_constructor::TypeConstructor,
    },
};

use super::super::diagnostics::compiler_errors::{
    DUPLICATE_CLASS, DUPLICATE_INSTANCE, INSTANCE_EXTRA_METHOD, INSTANCE_METHOD_ARITY,
    INSTANCE_MISSING_METHOD, INSTANCE_TYPE_ARG_ARITY, INSTANCE_UNKNOWN_CLASS,
    MISSING_SUPERCLASS_INSTANCE,
};

/// A type class definition collected from a `class` declaration.
#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: Identifier,
    /// Owning module of this class declaration (Proposal 0151, Phase 1b Step 1).
    ///
    /// For module-scoped classes, this is the dotted path of the enclosing
    /// `module` block, e.g. `Flow.Foldable`. For top-level (legacy) class
    /// declarations and built-in classes, this is `ModulePath::EMPTY`.
    ///
    /// Phase 1b Step 1 only **records** the owning module — `ClassEnv` is
    /// still keyed by the bare class name, so two classes with the same
    /// short name in different modules will currently still collide via the
    /// duplicate-class diagnostic. The storage flip lands in a later step.
    pub module: ModulePath,
    pub type_params: Vec<Identifier>,
    pub superclasses: Vec<ClassConstraint>,
    pub methods: Vec<MethodSig>,
    /// Methods that have default implementations in the class body.
    pub default_methods: Vec<Identifier>,
    pub span: Span,
}

impl ClassDef {
    /// Returns the canonical `ClassId` for this class definition.
    ///
    /// In Phase 1b Step 1 this is `(self.module, self.name)`. Once the storage
    /// flip lands the `ClassEnv` will key on this directly.
    pub fn class_id(&self) -> ClassId {
        ClassId::new(self.module, self.name)
    }
}

/// A method signature within a class definition.
#[derive(Debug, Clone)]
pub struct MethodSig {
    pub name: Identifier,
    /// Per-method type parameters (e.g., `<a, b>` on `fn fmap<a, b>`).
    pub type_params: Vec<Identifier>,
    /// Value-parameter types in source order.
    ///
    /// Invariant: this should contain one entry per value parameter, while
    /// `arity` remains the canonical call arity used by downstream consumers.
    pub param_types: Vec<TypeExpr>,
    pub return_type: TypeExpr,
    pub arity: usize,
}

/// An instance definition collected from an `instance` declaration.
#[derive(Debug, Clone)]
pub struct InstanceDef {
    /// Short name of the class being implemented. Retained as a parallel
    /// field next to `class_id` so that pre-Phase-1b call sites which only
    /// need the short name keep working without churn.
    pub class_name: Identifier,
    /// Canonical `ClassId` of the class being implemented (Proposal 0151,
    /// Phase 1b Step 4).
    ///
    /// This identifies the **class** this instance implements, including its
    /// owning module. It is distinct from [`instance_module`], which is the
    /// module where the `instance` block itself lives. The two can differ
    /// (e.g., a same-file instance for a foreign class).
    ///
    /// For instances built before class resolution can complete (such as
    /// the synthetic placeholders used by built-in instance registration),
    /// `class_id` is `ClassId::from_local_name(class_name)` — i.e. an empty
    /// `ModulePath`.
    pub class_id: ClassId,
    /// Owning module of this instance declaration (Proposal 0151, Phase 1b
    /// Step 2).
    ///
    /// This is the module where the `instance` block lives — *not* the module
    /// of the class being implemented (use [`class_id`] for that). Phase 2
    /// uses this for the orphan rule check: "an instance is legal in module
    /// M only if either the class or the head type is defined in M."
    ///
    /// For top-level (legacy) instance declarations and built-in instances,
    /// this is `ModulePath::EMPTY`.
    pub instance_module: ModulePath,
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
///
/// ## Proposal 0151, Phase 1b Step 3
///
/// Storage is now keyed on [`ClassId`] (`(ModulePath, Identifier)`) so two
/// classes with the same short name in different modules coexist as distinct
/// entries.
///
/// **Compatibility shims:** the legacy bare-`Identifier` lookup methods
/// ([`lookup_class`](Self::lookup_class), [`method_to_class`](Self::method_to_class),
/// [`method_index`](Self::method_index)) still exist and perform a linear
/// scan finding the first class with a matching short name. This keeps the
/// pre-Step-3 call sites working without forcing a flag-day migration.
/// The shims are first-match-wins and non-deterministic when two classes
/// share a short name across modules; they exist to bridge to a later
/// step which migrates callers to `ClassId`-keyed lookups.
#[derive(Debug, Clone, Default)]
pub struct ClassEnv {
    /// `ClassId` → class definition. (Phase 1b Step 3 — was previously
    /// keyed on bare `Identifier`.)
    pub classes: HashMap<ClassId, ClassDef>,
    /// All instance definitions (validated against their class)
    pub instances: Vec<InstanceDef>,
}

/// A resolved dictionary reference for a concrete class application.
///
/// `dict_name` identifies the dictionary global or dictionary-constructor
/// function for the matched instance head. `context_args` recursively describes
/// the dictionaries that must be supplied to contextual instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDictionaryRef {
    pub dict_name: Identifier,
    pub context_args: Vec<ResolvedDictionaryRef>,
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
        Self::collect_classes(statements, ModulePath::EMPTY, self, &mut diagnostics, interner);
        Self::collect_instances(statements, ModulePath::EMPTY, self, &mut diagnostics, interner);
        Self::collect_deriving(statements, ModulePath::EMPTY, self, &mut diagnostics, interner);
        diagnostics
    }

    /// Collect class declarations recursively (handles modules).
    ///
    /// `current_module` is the dotted path of the enclosing `module` block,
    /// or [`ModulePath::EMPTY`] for top-level (legacy) declarations. Each
    /// recursive descent into a `Statement::Module { name, body, .. }` block
    /// passes the module's interned name as the new `current_module`.
    /// (Proposal 0151, Phase 1b Step 1.)
    fn collect_classes(
        statements: &[Statement],
        current_module: ModulePath,
        env: &mut ClassEnv,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for stmt in statements {
            match stmt {
                Statement::Class {
                    is_public: _,
                    name,
                    type_params,
                    superclasses,
                    methods,
                    span,
                } => {
                    // Phase 1b Step 3: classes are keyed by ClassId, so two
                    // class declarations with the same short name in
                    // different modules are NO LONGER duplicates. The
                    // duplicate check fires only on a same-module collision.
                    let class_id = ClassId::new(current_module, *name);
                    if env.classes.contains_key(&class_id) {
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
                        class_id,
                        ClassDef {
                            name: *name,
                            module: current_module,
                            type_params: type_params.clone(),
                            superclasses: superclasses.clone(),
                            methods: method_sigs,
                            default_methods,
                            span: *span,
                        },
                    );
                }
                Statement::Module { name, body, .. } => {
                    // Recurse with the module's interned dotted name as the
                    // new owning module path.
                    let module_path = ModulePath::from_identifier(*name);
                    Self::collect_classes(
                        &body.statements,
                        module_path,
                        env,
                        diagnostics,
                        interner,
                    );
                }
                _ => {}
            }
        }
    }

    /// Collect instance declarations and validate against known classes.
    ///
    /// `current_module` follows the same convention as `collect_classes`:
    /// the dotted path of the enclosing `module` block, or
    /// [`ModulePath::EMPTY`] for top-level / legacy declarations. Each
    /// collected `InstanceDef` records its owning module so the orphan rule
    /// (Phase 2) can later check it. (Proposal 0151, Phase 1b Step 2.)
    fn collect_instances(
        statements: &[Statement],
        current_module: ModulePath,
        env: &mut ClassEnv,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for stmt in statements {
            match stmt {
                Statement::Instance {
                    is_public: _,
                    class_name,
                    type_args,
                    context,
                    methods,
                    span,
                } => {
                    // Check that the class exists. Phase 1b Step 4: prefer
                    // a class in the same module as the instance being
                    // collected, falling back to the bare-name shim. This
                    // ensures the instance's `class_id` correctly identifies
                    // the local class when both modules declare the same
                    // short name.
                    //
                    // We clone the `ClassDef` here because subsequent
                    // validation logic needs to mutate `env.instances`
                    // (duplicate-instance removal), which would conflict
                    // with the immutable `&ClassDef` borrow returned by
                    // the lookup. Cloning is cheap relative to the
                    // surrounding parser/HM work and only happens during
                    // instance collection.
                    let class_def = match env
                        .lookup_class_in_module_or_global(current_module, *class_name)
                        .cloned()
                    {
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

                    if type_args.len() != class_def.type_params.len() {
                        let display_class = interner.resolve(*class_name);
                        diagnostics.push(
                            diagnostic_for(&INSTANCE_TYPE_ARG_ARITY)
                                .with_span(*span)
                                .with_message(format!(
                                    "Instance for `{display_class}` uses {} type argument(s), \
                                     but the class declares {}.",
                                    type_args.len(),
                                    class_def.type_params.len()
                                ))
                                .with_hint_text(format!(
                                    "`{display_class}` expects {} type argument(s) in its instance head.",
                                    class_def.type_params.len()
                                )),
                        );
                        continue;
                    }

                    // Check for duplicate instances (same class + same head type).
                    // Uses structural equality ignoring source spans.
                    //
                    // Phase 1b Step 4: compare by `class_id`, not by
                    // `class_name`. This means `Mod.A.Foo<Int>` and
                    // `Mod.B.Foo<Int>` are NO LONGER duplicates because
                    // they implement different classes.
                    let new_class_id = class_def.class_id();
                    let duplicate_idx = env.instances.iter().position(|existing| {
                        existing.class_id == new_class_id
                            && existing.type_args.len() == type_args.len()
                            && existing
                                .type_args
                                .iter()
                                .zip(type_args.iter())
                                .all(|(a, b)| a.structural_eq(b))
                    });
                    if let Some(idx) = duplicate_idx {
                        let existing = &env.instances[idx];
                        let is_builtin_placeholder =
                            existing.span == Span::default() && existing.method_names.is_empty();
                        if is_builtin_placeholder {
                            env.instances.remove(idx);
                        } else {
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
                    }

                    // Validate: all required methods are implemented
                    let method_names: Vec<Identifier> = methods.iter().map(|m| m.name).collect();

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

                    // Validate: method arity matches class signature.
                    for method in methods {
                        if let Some(class_method) =
                            class_def.methods.iter().find(|m| m.name == method.name)
                        {
                            if method.params.len() != class_method.arity {
                                let display_class = interner.resolve(*class_name);
                                let display_method = interner.resolve(method.name);
                                diagnostics.push(
                                    diagnostic_for(&INSTANCE_METHOD_ARITY)
                                        .with_span(method.span)
                                        .with_message(format!(
                                            "Method `{display_method}` in instance `{display_class}` \
                                             has {} parameter(s), but the class declares {}.",
                                            method.params.len(),
                                            class_method.arity
                                        ))
                                        .with_hint_text(format!(
                                            "`{display_class}.{display_method}` expects {} parameter(s).",
                                            class_method.arity
                                        )),
                                );
                            }
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
                            let inst_types: Vec<String> = inst
                                .type_args
                                .iter()
                                .map(|t| t.display_with(interner))
                                .collect();
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
                        // Phase 1b Step 4: canonical ClassId of the class
                        // being implemented. We resolved the class above
                        // (cloned into `class_def`) and use its `class_id()`
                        // accessor to roll its (module, name) into a
                        // ClassId. Two same-named classes in different
                        // modules now have distinct instance buckets.
                        class_id: class_def.class_id(),
                        instance_module: current_module,
                        type_args: type_args.clone(),
                        context: context.clone(),
                        method_names,
                        span: *span,
                    });
                }
                Statement::Module { name, body, .. } => {
                    let module_path = ModulePath::from_identifier(*name);
                    Self::collect_instances(
                        &body.statements,
                        module_path,
                        env,
                        diagnostics,
                        interner,
                    );
                }
                _ => {}
            }
        }
    }

    /// Collect derived instances from `deriving` clauses on data declarations.
    ///
    /// `current_module` is the dotted path of the enclosing `module` block,
    /// or [`ModulePath::EMPTY`] for top-level data declarations. The derived
    /// instance inherits the data declaration's owning module — under the
    /// orphan rule (Phase 2), `deriving` instances are always legal because
    /// the head type and the derived instance live in the same module.
    /// (Proposal 0151, Phase 1b Step 2.)
    fn collect_deriving(
        statements: &[Statement],
        current_module: ModulePath,
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
                        // Check that the class exists. Phase 1b Step 4: prefer
                        // a class in the same module as the data declaration,
                        // falling back to the bare-name shim. Mirrors the
                        // disambiguation rule used by `collect_instances`.
                        let class_id = match env
                            .lookup_class_in_module_or_global(current_module, *class_name)
                        {
                            Some(def) => def.class_id(),
                            None => {
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
                        };

                        // Register a derived instance (no method bodies —
                        // the constraint solver just needs to know it exists).
                        let type_arg = builtin_type(*name);
                        env.instances.push(InstanceDef {
                            class_name: *class_name,
                            class_id,
                            instance_module: current_module,
                            type_args: vec![type_arg],
                            context: vec![],
                            method_names: vec![],
                            span: *span,
                        });
                    }
                }
                Statement::Module { name, body, .. } => {
                    let module_path = ModulePath::from_identifier(*name);
                    Self::collect_deriving(
                        &body.statements,
                        module_path,
                        env,
                        diagnostics,
                        interner,
                    );
                }
                _ => {}
            }
        }
    }

    // ========================================================================
    // Proposal 0151 — Phase 1b Step 3: bare-name compatibility shims.
    //
    // These methods exist so that pre-Step-3 call sites which only have a
    // bare `Identifier` (the class's short name) keep working without the
    // owning module path. They perform a linear scan over `self.classes`
    // and return the first match. When two classes share a short name
    // across modules, the result is non-deterministic — call sites that
    // need to disambiguate must migrate to the `_by_id` API below.
    //
    // A future commit will migrate the remaining bare-name callers to
    // ClassId and delete these shims.
    // ========================================================================

    /// Look up a class definition by short name (compatibility shim).
    ///
    /// Performs a linear scan over `self.classes` and returns the first
    /// `ClassDef` whose `name` matches. If multiple classes share the short
    /// name across modules the choice is non-deterministic — use
    /// [`lookup_class_by_id`](Self::lookup_class_by_id) to disambiguate.
    pub fn lookup_class(&self, name: Identifier) -> Option<&ClassDef> {
        self.classes.values().find(|def| def.name == name)
    }

    /// Look up a class definition by short name, **preferring** a class
    /// declared in `current_module` if one exists with that short name.
    ///
    /// This is the disambiguation rule used by [`collect_instances`] and
    /// [`collect_deriving`] in Phase 1b Step 4: an `instance Foo<...>`
    /// declaration written inside `module Mod.A` should refer to `Mod.A.Foo`
    /// when `Mod.A.Foo` exists, even if other modules also declare a `Foo`.
    /// Falls back to the bare-name shim ([`lookup_class`]) when no class
    /// with the matching name lives in `current_module`. This is
    /// approximate — proper import-aware resolution lands in Phase 2.
    pub fn lookup_class_in_module_or_global(
        &self,
        current_module: ModulePath,
        name: Identifier,
    ) -> Option<&ClassDef> {
        // Same-module preference: walk only classes whose owning module
        // matches `current_module`.
        if let Some(def) = self
            .classes
            .values()
            .find(|def| def.name == name && def.module == current_module)
        {
            return Some(def);
        }
        // Fall back to global bare-name lookup (any visible class with
        // matching short name).
        self.lookup_class(name)
    }

    /// Find all instances for a given class short name (compatibility shim).
    ///
    /// Returns all instances whose `class_name` matches, regardless of which
    /// owning module the class lives in. Use
    /// [`instances_for_id`](Self::instances_for_id) to disambiguate.
    pub fn instances_for(&self, class_name: Identifier) -> Vec<&InstanceDef> {
        self.instances
            .iter()
            .filter(|i| i.class_name == class_name)
            .collect()
    }

    /// Given a method name, find which class declares it (compatibility shim).
    ///
    /// Returns `(class_name, &ClassDef)` for the first class found whose
    /// methods include `method_name`. Linear scan over all classes.
    pub fn method_to_class(&self, method_name: Identifier) -> Option<(Identifier, &ClassDef)> {
        for class_def in self.classes.values() {
            if class_def.methods.iter().any(|m| m.name == method_name) {
                return Some((class_def.name, class_def));
            }
        }
        None
    }

    /// Return the positional index of a method within its class definition,
    /// looking the class up by short name (compatibility shim).
    ///
    /// Linear scan via [`lookup_class`](Self::lookup_class).
    pub fn method_index(&self, class_name: Identifier, method_name: Identifier) -> Option<usize> {
        let class_def = self.lookup_class(class_name)?;
        class_def.methods.iter().position(|m| m.name == method_name)
    }

    // ========================================================================
    // Proposal 0151 — Phase 1b Step 3: canonical ClassId-keyed API.
    //
    // These methods are the canonical lookups now that storage is keyed on
    // `ClassId`. They respect both the owning module and the class name and
    // return distinct results for two same-named classes in different modules.
    // ========================================================================

    /// Look up a class definition by its canonical `ClassId`.
    pub fn lookup_class_by_id(&self, id: ClassId) -> Option<&ClassDef> {
        self.classes.get(&id)
    }

    /// Find all instances for a given class identified by `ClassId`.
    ///
    /// Phase 1b Step 4: filters strictly on the instance's `class_id`,
    /// so two same-named classes in different modules return disjoint
    /// instance lists.
    pub fn instances_for_id(&self, id: ClassId) -> Vec<&InstanceDef> {
        self.instances
            .iter()
            .filter(|inst| inst.class_id == id)
            .collect()
    }

    /// Return the positional index of a method within a class identified by
    /// `ClassId`.
    pub fn method_index_by_id(
        &self,
        id: ClassId,
        method_name: Identifier,
    ) -> Option<usize> {
        let class_def = self.lookup_class_by_id(id)?;
        class_def.methods.iter().position(|m| m.name == method_name)
    }

    /// Resolve an instance against concrete inferred type arguments, using a
    /// `ClassId` to identify the class.
    ///
    /// Phase 1b Step 4: filters by `class_id` so the lookup is correctly
    /// scoped to the requested class even when another class with the same
    /// short name lives in a different module.
    pub fn resolve_instance_with_subst_by_id(
        &self,
        id: ClassId,
        actual_type_args: &[InferType],
        interner: &Interner,
    ) -> Option<(&InstanceDef, HashMap<Identifier, InferType>)> {
        self.instances.iter().find_map(|inst| {
            if inst.class_id != id || inst.type_args.len() != actual_type_args.len() {
                return None;
            }

            let mut subst = HashMap::new();
            let matches =
                inst.type_args
                    .iter()
                    .zip(actual_type_args.iter())
                    .all(|(pattern, actual)| {
                        Self::match_instance_type_expr(pattern, actual, &mut subst, interner)
                    });

            matches.then_some((inst, subst))
        })
    }

    /// Resolve a class instance for a concrete type name (e.g., "Int", "String").
    /// Matches against the first `type_arg` of each instance declaration.
    pub fn resolve_instance_for_type(
        &self,
        class_name: Identifier,
        type_name: &str,
        interner: &Interner,
    ) -> Option<&InstanceDef> {
        let actual = match type_name {
            "Int" => InferType::Con(TypeConstructor::Int),
            "Float" => InferType::Con(TypeConstructor::Float),
            "Bool" => InferType::Con(TypeConstructor::Bool),
            "String" => InferType::Con(TypeConstructor::String),
            "Unit" => InferType::Con(TypeConstructor::Unit),
            "List" => InferType::Con(TypeConstructor::List),
            "Array" => InferType::Con(TypeConstructor::Array),
            "Option" => InferType::Con(TypeConstructor::Option),
            other => InferType::Con(TypeConstructor::Adt(interner.lookup(other)?)),
        };
        self.resolve_instance_with_subst(class_name, &[actual], interner)
            .map(|(inst, _)| inst)
    }

    /// Resolve an instance against concrete inferred type arguments.
    ///
    /// Returns the matched instance and the type-variable substitution induced
    /// by matching the instance head against the concrete type arguments.
    pub fn resolve_instance_with_subst(
        &self,
        class_name: Identifier,
        actual_type_args: &[InferType],
        interner: &Interner,
    ) -> Option<(&InstanceDef, HashMap<Identifier, InferType>)> {
        self.instances.iter().find_map(|inst| {
            if inst.class_name != class_name || inst.type_args.len() != actual_type_args.len() {
                return None;
            }

            let mut subst = HashMap::new();
            let matches =
                inst.type_args
                    .iter()
                    .zip(actual_type_args.iter())
                    .all(|(pattern, actual)| {
                        Self::match_instance_type_expr(pattern, actual, &mut subst, interner)
                    });

            matches.then_some((inst, subst))
        })
    }

    /// Resolve the dictionary reference needed for a concrete class application.
    ///
    /// For plain instances this returns a leaf `ResolvedDictionaryRef` pointing
    /// at `__dict_{Class}_{Type}`. For contextual instances it recursively
    /// resolves the dictionaries required by the instance context so callers can
    /// either capture them (dictionary construction) or pass them as arguments.
    pub fn resolve_dictionary_ref(
        &self,
        class_name: Identifier,
        actual_type_args: &[InferType],
        interner: &Interner,
    ) -> Option<ResolvedDictionaryRef> {
        let (instance, subst) =
            self.resolve_instance_with_subst(class_name, actual_type_args, interner)?;
        let class_str = interner.resolve(class_name);
        let type_name = instance
            .type_args
            .iter()
            .map(|arg| arg.display_with(interner))
            .collect::<Vec<_>>()
            .join("_");
        let dict_name = interner.lookup(&format!("__dict_{class_str}_{type_name}"))?;
        let context_args = instance
            .context
            .iter()
            .map(|constraint| {
                let concrete_args = constraint
                    .type_args
                    .iter()
                    .map(|arg| instantiate_instance_type_expr(arg, &subst, interner))
                    .collect::<Option<Vec<_>>>()?;
                self.resolve_dictionary_ref(constraint.class_name, &concrete_args, interner)
            })
            .collect::<Option<Vec<_>>>()?;

        Some(ResolvedDictionaryRef {
            dict_name,
            context_args,
        })
    }

    /// Resolve only the context dictionaries required by the matched instance.
    ///
    /// This is used by direct monomorphic calls to a mangled `__tc_*` method:
    /// the caller needs the instance context arguments, not the whole instance
    /// dictionary constructor.
    pub fn resolve_instance_context_dictionaries(
        &self,
        class_name: Identifier,
        actual_type_args: &[InferType],
        interner: &Interner,
    ) -> Option<Vec<ResolvedDictionaryRef>> {
        let (instance, subst) =
            self.resolve_instance_with_subst(class_name, actual_type_args, interner)?;
        instance
            .context
            .iter()
            .map(|constraint| {
                let concrete_args = constraint
                    .type_args
                    .iter()
                    .map(|arg| instantiate_instance_type_expr(arg, &subst, interner))
                    .collect::<Option<Vec<_>>>()?;
                self.resolve_dictionary_ref(constraint.class_name, &concrete_args, interner)
            })
            .collect()
    }

    /// Expand a pre-interned `__dict_{Class}_{Type}` name into the ordered
    /// mangled method symbols that make up the dictionary tuple, if this name
    /// corresponds to a known instance.
    pub fn dictionary_method_symbols(
        &self,
        dict_name: Identifier,
        interner: &Interner,
    ) -> Option<Vec<Identifier>> {
        let dict_name_str = interner.resolve(dict_name);
        self.instances.iter().find_map(|instance| {
            let class_def = self.lookup_class(instance.class_name)?;
            let class_str = interner.resolve(instance.class_name);
            let type_name = instance
                .type_args
                .iter()
                .map(|arg| arg.display_with(interner))
                .collect::<Vec<_>>()
                .join("_");
            let expected = format!("__dict_{class_str}_{type_name}");
            if dict_name_str != expected {
                return None;
            }

            class_def
                .methods
                .iter()
                .map(|method_sig| {
                    let method_str = interner.resolve(method_sig.name);
                    interner.lookup(&format!("__tc_{class_str}_{type_name}_{method_str}"))
                })
                .collect()
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
        let neq_method = interner.intern("neq");
        let compare_method = interner.intern("compare");
        let lt_method = interner.intern("lt");
        let lte_method = interner.intern("lte");
        let gt_method = interner.intern("gt");
        let gte_method = interner.intern("gte");
        let add_method = interner.intern("add");
        let sub_method = interner.intern("sub");
        let mul_method = interner.intern("mul");
        let div_method = interner.intern("div");
        let show_method = interner.intern("show");
        let append_method = interner.intern("append");

        let int_name = interner.intern("Int");
        let float_name = interner.intern("Float");
        let string_name = interner.intern("String");
        let bool_name = interner.intern("Bool");

        let a_param = interner.intern("a");

        // ── Class definitions ──────────────────────────────────────────

        let a_ty = builtin_type(a_param);
        let bool_ty = builtin_type(bool_name);
        let int_ty = builtin_type(int_name);
        let string_ty = builtin_type(string_name);

        // Eq: eq(a, a) -> Bool, neq(a, a) -> Bool
        self.register_builtin_class(
            eq,
            vec![a_param],
            vec![
                MethodSig {
                    type_params: vec![],
                    name: eq_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: neq_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                },
            ],
        );

        // Ord: compare(a, a) -> Int plus relational helpers.
        self.register_builtin_class(
            ord,
            vec![a_param],
            vec![
                MethodSig {
                    type_params: vec![],
                    name: compare_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: int_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: lt_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: lte_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: gt_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: gte_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                },
            ],
        );

        // Num: add/sub/mul/div.
        self.register_builtin_class(
            num,
            vec![a_param],
            vec![
                MethodSig {
                    type_params: vec![],
                    name: add_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: sub_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: mul_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                },
                MethodSig {
                    type_params: vec![],
                    name: div_method,
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                },
            ],
        );

        // Show: show(a) -> String
        self.register_builtin_class(
            show,
            vec![a_param],
            vec![MethodSig {
                type_params: vec![],
                name: show_method,
                param_types: vec![a_ty.clone()],
                return_type: string_ty,
                arity: 1,
            }],
        );

        // Semigroup: append(a, a) -> a
        self.register_builtin_class(
            semigroup,
            vec![a_param],
            vec![MethodSig {
                type_params: vec![],
                name: append_method,
                param_types: vec![a_ty.clone(), a_ty],
                return_type: builtin_type(a_param),
                arity: 2,
            }],
        );

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
        type_params: Vec<Identifier>,
        methods: Vec<MethodSig>,
    ) {
        // Don't override user-declared classes. The "user-declared" check
        // looks up by short name across all owning modules — if any user
        // class shares the short name we skip registration. (Built-ins live
        // in the implicit prelude with `ModulePath::EMPTY`, so this check
        // also catches the same-module collision case.)
        if self.lookup_class(name).is_some() {
            return;
        }
        let class_id = ClassId::from_local_name(name);
        self.classes.insert(
            class_id,
            ClassDef {
                name,
                // Built-in classes have no owning module — they live in the
                // implicit prelude. Phase 2's orphan rule treats `EMPTY` as
                // "owned by the prelude" so users cannot declare orphan
                // instances for built-in classes outside the class's own
                // module.
                module: ModulePath::EMPTY,
                type_params,
                superclasses: vec![],
                methods,
                default_methods: vec![],
                span: Span::default(),
            },
        );
    }

    /// Register a single built-in instance.
    fn register_builtin_instance(&mut self, class_name: Identifier, type_name: Identifier) {
        // Don't duplicate if user already declared this instance.
        let expected = builtin_type(type_name);
        let already_exists = self.instances.iter().any(|i| {
            i.class_name == class_name
                && i.type_args
                    .first()
                    .is_some_and(|t| t.structural_eq(&expected))
        });
        if already_exists {
            return;
        }
        self.instances.push(InstanceDef {
            class_name,
            // Phase 1b Step 4: built-in classes live in the implicit prelude
            // (`ModulePath::EMPTY`), so the class_id is constructed via
            // `from_local_name`. This matches the storage key used in
            // `register_builtin_class` above.
            class_id: ClassId::from_local_name(class_name),
            // Built-in instances live in the implicit prelude — same `EMPTY`
            // sentinel as built-in classes.
            instance_module: ModulePath::EMPTY,
            type_args: vec![builtin_type(type_name)],
            context: vec![],
            method_names: vec![],
            span: Span::default(),
        });
    }
}

fn instantiate_instance_type_expr(
    ty: &TypeExpr,
    subst: &HashMap<Identifier, InferType>,
    interner: &Interner,
) -> Option<InferType> {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            if args.is_empty()
                && let Some(mapped) = subst.get(name)
            {
                return Some(mapped.clone());
            }

            let resolved_args = args
                .iter()
                .map(|arg| instantiate_instance_type_expr(arg, subst, interner))
                .collect::<Option<Vec<_>>>()?;

            Some(match interner.resolve(*name) {
                "Int" => InferType::Con(TypeConstructor::Int),
                "Float" => InferType::Con(TypeConstructor::Float),
                "Bool" => InferType::Con(TypeConstructor::Bool),
                "String" => InferType::Con(TypeConstructor::String),
                "Unit" => InferType::Con(TypeConstructor::Unit),
                "List" => InferType::App(TypeConstructor::List, resolved_args),
                "Array" => InferType::App(TypeConstructor::Array, resolved_args),
                "Option" => InferType::App(TypeConstructor::Option, resolved_args),
                "Either" => InferType::App(TypeConstructor::Either, resolved_args),
                "Map" => InferType::App(TypeConstructor::Map, resolved_args),
                _ => {
                    if resolved_args.is_empty() {
                        InferType::Con(TypeConstructor::Adt(*name))
                    } else {
                        InferType::App(TypeConstructor::Adt(*name), resolved_args)
                    }
                }
            })
        }
        TypeExpr::Tuple { elements, .. } => Some(InferType::Tuple(
            elements
                .iter()
                .map(|elem| instantiate_instance_type_expr(elem, subst, interner))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeExpr::Function { params, ret, .. } => Some(InferType::Fun(
            params
                .iter()
                .map(|param| instantiate_instance_type_expr(param, subst, interner))
                .collect::<Option<Vec<_>>>()?,
            Box::new(instantiate_instance_type_expr(ret, subst, interner)?),
            crate::types::infer_effect_row::InferEffectRow::closed_empty(),
        )),
    }
}

impl ClassEnv {
    fn match_instance_type_expr(
        pattern: &TypeExpr,
        actual: &InferType,
        subst: &mut HashMap<Identifier, InferType>,
        interner: &Interner,
    ) -> bool {
        match pattern {
            TypeExpr::Named { name, args, .. }
                if args.is_empty() && Self::is_instance_type_var(*name, interner) =>
            {
                if let Some(bound) = subst.get(name) {
                    bound == actual
                } else {
                    subst.insert(*name, actual.clone());
                    true
                }
            }
            TypeExpr::Named { name, args, .. } => match actual {
                InferType::Con(tc) => {
                    args.is_empty() && Self::type_constructor_matches(*name, tc, interner)
                }
                InferType::App(tc, actual_args) => {
                    if args.is_empty() {
                        Self::type_constructor_matches(*name, tc, interner)
                    } else {
                        Self::type_constructor_matches(*name, tc, interner)
                            && args.len() == actual_args.len()
                            && args
                                .iter()
                                .zip(actual_args.iter())
                                .all(|(p, a)| Self::match_instance_type_expr(p, a, subst, interner))
                    }
                }
                InferType::HktApp(head, actual_args) => match head.as_ref() {
                    InferType::Con(tc) => {
                        if args.is_empty() {
                            Self::type_constructor_matches(*name, tc, interner)
                        } else {
                            Self::type_constructor_matches(*name, tc, interner)
                                && args.len() == actual_args.len()
                                && args
                                    .iter()
                                    .zip(actual_args.iter())
                                    .all(|(p, a)| {
                                        Self::match_instance_type_expr(p, a, subst, interner)
                                    })
                        }
                    }
                    _ => false,
                },
                _ => false,
            },
            TypeExpr::Tuple { elements, .. } => match actual {
                InferType::Tuple(actual_elems) => {
                    elements.len() == actual_elems.len()
                        && elements
                            .iter()
                            .zip(actual_elems.iter())
                            .all(|(p, a)| Self::match_instance_type_expr(p, a, subst, interner))
                }
                _ => false,
            },
            TypeExpr::Function { params, ret, .. } => match actual {
                InferType::Fun(actual_params, actual_ret, _) => {
                    params.len() == actual_params.len()
                        && params
                            .iter()
                            .zip(actual_params.iter())
                            .all(|(p, a)| Self::match_instance_type_expr(p, a, subst, interner))
                        && Self::match_instance_type_expr(ret, actual_ret, subst, interner)
                }
                _ => false,
            },
        }
    }

    fn is_instance_type_var(name: Identifier, interner: &Interner) -> bool {
        interner
            .resolve(name)
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
    }

    fn type_constructor_matches(
        expected_name: Identifier,
        actual: &TypeConstructor,
        interner: &Interner,
    ) -> bool {
        match actual {
            TypeConstructor::Int => interner.resolve(expected_name) == "Int",
            TypeConstructor::Float => interner.resolve(expected_name) == "Float",
            TypeConstructor::Bool => interner.resolve(expected_name) == "Bool",
            TypeConstructor::String => interner.resolve(expected_name) == "String",
            TypeConstructor::Unit => interner.resolve(expected_name) == "Unit",
            TypeConstructor::List => interner.resolve(expected_name) == "List",
            TypeConstructor::Array => interner.resolve(expected_name) == "Array",
            TypeConstructor::Option => interner.resolve(expected_name) == "Option",
            TypeConstructor::Map => interner.resolve(expected_name) == "Map",
            TypeConstructor::Either => interner.resolve(expected_name) == "Either",
            TypeConstructor::Adt(sym) => *sym == expected_name,
            _ => false,
        }
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

#[cfg(test)]
mod tests {
    use super::{ClassEnv, InstanceDef, builtin_type};
    use crate::{
        diagnostics::position::Span,
        syntax::interner::Interner,
        types::{
            class_id::ModulePath, infer_type::InferType, type_constructor::TypeConstructor,
        },
    };

    fn s() -> Span {
        Span::default()
    }

    /// Proposal 0151, Phase 1b Step 1: a top-level (legacy) class declaration
    /// is collected with `module: ModulePath::EMPTY`.
    #[test]
    fn top_level_class_has_empty_module_path() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
class TopLvlClass<a> {
    fn doit(x: a) -> Bool
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        let class_sym = interner
            .lookup("TopLvlClass")
            .expect("class name should be interned");
        let class_def = env
            .lookup_class(class_sym)
            .expect("TopLvlClass should be in the env");
        assert_eq!(
            class_def.module,
            ModulePath::EMPTY,
            "top-level classes should have empty module path"
        );
    }

    /// Proposal 0151, Phase 1b Step 1: a class declared inside a module body
    /// is collected with `module: ModulePath::from_identifier(<dotted name>)`.
    #[test]
    fn module_scoped_class_has_module_path_populated() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Phase1b.Step1 {
    class ModScoped<a> {
        fn doit(x: a) -> Bool
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        let class_sym = interner.lookup("ModScoped").expect("class interned");
        let class_def = env
            .lookup_class(class_sym)
            .expect("ModScoped should be in env");

        let expected_module_sym = interner
            .lookup("Phase1b.Step1")
            .expect("module name should be interned");
        assert_eq!(
            class_def.module,
            ModulePath::from_identifier(expected_module_sym),
            "module-scoped class should carry its owning module path"
        );

        // The synthesized ClassId rolls module + name together.
        assert!(
            !class_def.class_id().is_local(),
            "class_id should not report local for a module-scoped class"
        );
    }

    /// Proposal 0151, Phase 1b Step 2: a top-level (legacy) instance
    /// declaration is collected with `instance_module: ModulePath::EMPTY`.
    #[test]
    fn top_level_instance_has_empty_instance_module() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
class Step2Eq<a> {
    fn step2eq(x: a, y: a) -> Bool
}

instance Step2Eq<Int> {
    fn step2eq(x, y) { x == y }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        let class_sym = interner.lookup("Step2Eq").unwrap();
        let inst = env
            .instances
            .iter()
            .find(|i| i.class_name == class_sym)
            .expect("instance should be present");
        assert_eq!(
            inst.instance_module,
            ModulePath::EMPTY,
            "top-level instances should have empty instance_module"
        );
    }

    /// Proposal 0151, Phase 1b Step 2: a module-scoped instance carries the
    /// owning module's dotted path in `instance_module`.
    #[test]
    fn module_scoped_instance_has_module_path_populated() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Phase1b.Step2 {
    class ModEq<a> {
        fn modeq(x: a, y: a) -> Bool
    }

    instance ModEq<Int> {
        fn modeq(x, y) { x == y }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        let class_sym = interner.lookup("ModEq").unwrap();
        let inst = env
            .instances
            .iter()
            .find(|i| i.class_name == class_sym)
            .expect("instance should be present");

        let expected = interner.lookup("Phase1b.Step2").unwrap();
        assert_eq!(
            inst.instance_module,
            ModulePath::from_identifier(expected),
            "module-scoped instance should carry its owning module path"
        );
    }

    /// Proposal 0151, Phase 1b Step 2: a `deriving` clause inside a module
    /// records the data declaration's owning module on the synthesized
    /// instance — preparing for Phase 2's orphan rule, which will accept
    /// derived instances by construction (the head type and the derived
    /// instance live in the same module).
    #[test]
    fn module_scoped_deriving_records_owning_module() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        // Declare the class in-source so we don't depend on built-in
        // pre-registration (which only happens in the bytecode compiler).
        // `public data` isn't parsed yet — bare `data` is sufficient here.
        let source = r#"
module Phase1b.Step2Derive {
    class DerivableShow<a> {
        fn show_it(x: a) -> Bool
    }

    data Color { Red, Green, Blue } deriving (DerivableShow)
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.is_empty(),
            "unexpected collection errors: {:?}",
            diags
        );

        let class_sym = interner.lookup("DerivableShow").unwrap();
        let color_sym = interner.lookup("Color").unwrap();
        // Find the synthesized derived instance for DerivableShow<Color>.
        let derived = env.instances.iter().find(|i| {
            i.class_name == class_sym
                && i.type_args.first().is_some_and(|ty| match ty {
                    crate::syntax::type_expr::TypeExpr::Named { name, .. } => *name == color_sym,
                    _ => false,
                })
        });
        let derived = derived.expect("derived DerivableShow<Color> instance should be present");

        let expected = interner.lookup("Phase1b.Step2Derive").unwrap();
        assert_eq!(
            derived.instance_module,
            ModulePath::from_identifier(expected),
            "module-scoped derived instance should inherit the data's owning module"
        );
    }

    /// Proposal 0151, Phase 1b Step 3: **the headline test for the storage
    /// flip.** Two classes with the same short name `Foo` in different
    /// modules `Mod.A` and `Mod.B` must coexist in `ClassEnv` as distinct
    /// entries, no `DUPLICATE_CLASS` diagnostic, and `lookup_class_by_id`
    /// returns the right one for each `ClassId`.
    ///
    /// Before Step 3 this would have collided on the bare-`Identifier` key.
    #[test]
    fn two_classes_with_same_short_name_in_different_modules_coexist() {
        use crate::syntax::{lexer::Lexer, parser::Parser};
        use crate::types::class_id::ClassId;

        let source = r#"
module Mod.A {
    class Foo<a> {
        fn foo_method(x: a) -> Bool
    }
}

module Mod.B {
    class Foo<a> {
        fn foo_method(x: a) -> Bool
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.is_empty(),
            "two same-name classes in different modules should NOT trigger DUPLICATE_CLASS, \
             got: {:?}",
            diags
        );

        // Both classes are present as distinct entries.
        assert_eq!(
            env.classes.len(),
            2,
            "expected exactly 2 distinct ClassDef entries"
        );

        let foo_sym = interner.lookup("Foo").unwrap();
        let mod_a = interner.lookup("Mod.A").unwrap();
        let mod_b = interner.lookup("Mod.B").unwrap();

        let id_a = ClassId::new(ModulePath::from_identifier(mod_a), foo_sym);
        let id_b = ClassId::new(ModulePath::from_identifier(mod_b), foo_sym);

        let def_a = env
            .lookup_class_by_id(id_a)
            .expect("Mod.A.Foo should be findable");
        let def_b = env
            .lookup_class_by_id(id_b)
            .expect("Mod.B.Foo should be findable");

        // Both have the same short name but different owning modules.
        assert_eq!(def_a.name, foo_sym);
        assert_eq!(def_b.name, foo_sym);
        assert_eq!(def_a.module, ModulePath::from_identifier(mod_a));
        assert_eq!(def_b.module, ModulePath::from_identifier(mod_b));

        // The bare-name compatibility shim picks one (non-deterministic but
        // a valid result), and `instances_for(Foo)` would return both
        // instance lists if any existed.
        let bare = env.lookup_class(foo_sym);
        assert!(bare.is_some(), "bare-name shim should still find a class");
    }

    /// Proposal 0151, Phase 1b Step 4: when two same-named classes in
    /// different modules each have an instance for `Int`, `instances_for_id`
    /// must return disjoint lists keyed strictly on `ClassId` — *not* on the
    /// class's short name.
    ///
    /// Before Step 4, `instances_for_id` proxied to `instances_for(id.name)`
    /// and would have returned both instances for either query (because the
    /// short-name shim ignores the owning module). Step 4 tightens the
    /// filter to use `inst.class_id == id`.
    #[test]
    fn instances_for_id_returns_disjoint_buckets_for_same_named_classes() {
        use crate::syntax::{lexer::Lexer, parser::Parser};
        use crate::types::class_id::ClassId;

        let source = r#"
module Mod.A {
    class Foo<a> {
        fn foo_method(x: a) -> Bool
    }

    instance Foo<Int> {
        fn foo_method(x) { x == 0 }
    }
}

module Mod.B {
    class Foo<a> {
        fn foo_method(x: a) -> Bool
    }

    instance Foo<Int> {
        fn foo_method(x) { x == 1 }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        // Both classes coexist (Step 3 invariant).
        assert_eq!(env.classes.len(), 2);

        // Both instances coexist as separate entries.
        assert_eq!(env.instances.len(), 2);

        let foo_sym = interner.lookup("Foo").unwrap();
        let mod_a = interner.lookup("Mod.A").unwrap();
        let mod_b = interner.lookup("Mod.B").unwrap();

        let id_a = ClassId::new(ModulePath::from_identifier(mod_a), foo_sym);
        let id_b = ClassId::new(ModulePath::from_identifier(mod_b), foo_sym);

        let insts_a = env.instances_for_id(id_a);
        let insts_b = env.instances_for_id(id_b);

        // Each query returns exactly its own instance — not the union.
        assert_eq!(insts_a.len(), 1, "Mod.A.Foo should have exactly 1 instance");
        assert_eq!(insts_b.len(), 1, "Mod.B.Foo should have exactly 1 instance");

        // The two instance entries point at different ClassIds.
        assert_eq!(insts_a[0].class_id, id_a);
        assert_eq!(insts_b[0].class_id, id_b);

        // Their owning modules also differ.
        assert_eq!(insts_a[0].instance_module, ModulePath::from_identifier(mod_a));
        assert_eq!(insts_b[0].instance_module, ModulePath::from_identifier(mod_b));

        // The bare-name shim still returns BOTH (it can't disambiguate).
        let bare = env.instances_for(foo_sym);
        assert_eq!(bare.len(), 2, "bare-name shim returns the union");
    }

    /// Proposal 0151, Phase 1b Step 4: `resolve_instance_with_subst_by_id`
    /// scopes its instance scan to the requested `ClassId` and refuses to
    /// match an instance defined under a different (same-short-name) class.
    #[test]
    fn resolve_instance_with_subst_by_id_respects_class_id() {
        use crate::syntax::{lexer::Lexer, parser::Parser};
        use crate::types::class_id::ClassId;
        use crate::types::infer_type::InferType;
        use crate::types::type_constructor::TypeConstructor;

        let source = r#"
module Mod.A {
    class Foo<a> {
        fn foo_method(x: a) -> Bool
    }

    instance Foo<Int> {
        fn foo_method(x) { x == 0 }
    }
}

module Mod.B {
    class Foo<a> {
        fn foo_method(x: a) -> Bool
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        let foo_sym = interner.lookup("Foo").unwrap();
        let mod_a = interner.lookup("Mod.A").unwrap();
        let mod_b = interner.lookup("Mod.B").unwrap();
        let id_a = ClassId::new(ModulePath::from_identifier(mod_a), foo_sym);
        let id_b = ClassId::new(ModulePath::from_identifier(mod_b), foo_sym);

        let int = InferType::Con(TypeConstructor::Int);

        // Mod.A.Foo<Int> exists and resolves.
        assert!(
            env.resolve_instance_with_subst_by_id(id_a, &[int.clone()], &interner)
                .is_some(),
            "Mod.A.Foo<Int> should resolve"
        );

        // Mod.B.Foo<Int> does NOT exist — must return None even though
        // Mod.A.Foo<Int> shares the same short class name.
        assert!(
            env.resolve_instance_with_subst_by_id(id_b, &[int], &interner)
                .is_none(),
            "Mod.B.Foo<Int> should NOT resolve to Mod.A's instance"
        );
    }

    /// Proposal 0151, Phase 1b Step 3: declaring `class Foo` twice in the
    /// **same** module is still a duplicate-class error.
    #[test]
    fn duplicate_class_in_same_module_still_errors() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.Same {
    class Dup<a> {
        fn doit(x: a) -> Bool
    }

    class Dup<a> {
        fn doit(x: a) -> Bool
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);

        // First declaration succeeds, second is rejected as a duplicate.
        assert_eq!(env.classes.len(), 1, "only one class should be inserted");
        assert!(
            diags
                .iter()
                .any(|d| d.code.as_deref() == Some("E440")),
            "expected DUPLICATE_CLASS (E440), got: {:?}",
            diags
        );
    }

    /// Proposal 0151, Phase 1b Step 1: nested module declarations propagate
    /// the innermost module's full dotted name as the owning module path.
    #[test]
    fn nested_module_passes_innermost_path() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        // The Flux parser doesn't currently support textually nested
        // `module A { module B { ... } }` blocks, so we exercise the
        // already-dotted form `Outer.Inner` which is what real code uses.
        let source = r#"
module Outer.Inner.Deep {
    class DeeplyNested<a> {
        fn nested_op(x: a) -> Int
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        let class_sym = interner.lookup("DeeplyNested").unwrap();
        let class_def = env.lookup_class(class_sym).unwrap();

        let expected = interner.lookup("Outer.Inner.Deep").unwrap();
        assert_eq!(class_def.module, ModulePath::from_identifier(expected));
    }

    fn env_with_instance(
        interner: &mut Interner,
        class_name: &str,
        type_args: Vec<crate::syntax::type_expr::TypeExpr>,
    ) -> (ClassEnv, crate::syntax::Identifier) {
        let class_sym = interner.intern(class_name);
        let mut env = ClassEnv::new();
        env.instances.push(InstanceDef {
            class_name: class_sym,
            class_id: crate::types::class_id::ClassId::from_local_name(class_sym),
            instance_module: ModulePath::EMPTY,
            type_args,
            context: vec![],
            method_names: vec![],
            span: s(),
        });
        (env, class_sym)
    }

    #[test]
    fn resolve_instance_matches_bare_hkt_constructor_against_applied_list() {
        let mut interner = Interner::new();
        let list = interner.intern("List");
        let (env, functor) = env_with_instance(&mut interner, "Functor", vec![builtin_type(list)]);

        let actual = InferType::App(
            TypeConstructor::List,
            vec![InferType::Con(TypeConstructor::Int)],
        );

        assert!(
            env.resolve_instance_with_subst(functor, &[actual], &interner)
                .is_some()
        );
    }

    #[test]
    fn resolve_instance_matches_bare_hkt_constructor_against_hkt_app() {
        let mut interner = Interner::new();
        let list = interner.intern("List");
        let (env, functor) = env_with_instance(&mut interner, "Functor", vec![builtin_type(list)]);

        let actual = InferType::HktApp(
            Box::new(InferType::Con(TypeConstructor::List)),
            vec![InferType::Con(TypeConstructor::Int)],
        );

        assert!(
            env.resolve_instance_with_subst(functor, &[actual], &interner)
                .is_some()
        );
    }

    #[test]
    fn resolve_instance_matches_multi_arg_constructor_against_applied_either() {
        let mut interner = Interner::new();
        let either = interner.intern("Either");
        let (env, bifunctor) =
            env_with_instance(&mut interner, "Bifunctor", vec![builtin_type(either)]);

        let actual = InferType::App(
            TypeConstructor::Either,
            vec![
                InferType::Con(TypeConstructor::String),
                InferType::Con(TypeConstructor::Int),
            ],
        );

        assert!(
            env.resolve_instance_with_subst(bifunctor, &[actual], &interner)
                .is_some()
        );
    }

    #[test]
    fn resolve_instance_rejects_different_constructor_for_bare_hkt_pattern() {
        let mut interner = Interner::new();
        let list = interner.intern("List");
        let (env, functor) = env_with_instance(&mut interner, "Functor", vec![builtin_type(list)]);

        let actual = InferType::App(
            TypeConstructor::Option,
            vec![InferType::Con(TypeConstructor::Int)],
        );

        assert!(
            env.resolve_instance_with_subst(functor, &[actual], &interner)
                .is_none()
        );
    }

    #[test]
    fn resolve_instance_preserves_structural_matching_for_explicit_args() {
        let mut interner = Interner::new();
        let list = interner.intern("List");
        let int = interner.intern("Int");
        let (env, eq) = env_with_instance(
            &mut interner,
            "Eq",
            vec![crate::syntax::type_expr::TypeExpr::Named {
                name: list,
                args: vec![builtin_type(int)],
                span: s(),
            }],
        );

        let matches = InferType::App(
            TypeConstructor::List,
            vec![InferType::Con(TypeConstructor::Int)],
        );
        let does_not_match = InferType::App(
            TypeConstructor::List,
            vec![InferType::Con(TypeConstructor::String)],
        );

        assert!(
            env.resolve_instance_with_subst(eq, &[matches], &interner)
                .is_some()
        );
        assert!(
            env.resolve_instance_with_subst(eq, &[does_not_match], &interner)
                .is_none()
        );
    }
}
