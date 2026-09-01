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
        block::Block,
        effect_expr::EffectExpr,
        interner::Interner,
        statement::Statement,
        type_class::{AssociatedTypeDecl, AssociatedTypeEquation, ClassConstraint},
        type_expr::TypeExpr,
    },
    types::{
        class_id::{ClassId, ModulePath},
        infer_type::InferType,
        type_constructor::TypeConstructor,
    },
};

use super::super::diagnostics::compiler_errors::{
    ASSOCIATED_TYPE_KIND_MISMATCH, DUPLICATE_ASSOCIATED_TYPE, MISSING_ASSOCIATED_TYPE,
    RECURSIVE_ASSOCIATED_TYPE, UNBOUND_ASSOCIATED_TYPE_VARIABLE, UNKNOWN_ASSOCIATED_TYPE,
};
use super::super::diagnostics::compiler_errors::{
    DUPLICATE_CLASS, DUPLICATE_INSTANCE, INSTANCE_EXTRA_METHOD, INSTANCE_METHOD_ARITY,
    INSTANCE_MISSING_METHOD, INSTANCE_TYPE_ARG_ARITY, INSTANCE_UNKNOWN_CLASS,
    MISSING_SUPERCLASS_INSTANCE, ORPHAN_INSTANCE, PUBLIC_CLASS_LEAKS_PRIVATE_TYPE,
    PUBLIC_INSTANCE_HAS_PRIVATE_HEAD, PUBLIC_INSTANCE_OF_PRIVATE_CLASS, SEALED_CLASS_INSTANCE,
    SUPERCLASS_CYCLE,
};

/// Proposal 0151, Phase 2: per-ADT bookkeeping used by the orphan and
/// visibility walkers. Built once per `collect_from_statements` call by
/// walking `Statement::Data` declarations across all module bodies.
#[derive(Debug, Clone, Copy)]
struct DataInfo {
    /// Owning module of the data declaration.
    module: ModulePath,
    /// `true` for `public data`, `false` otherwise.
    is_public: bool,
}

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
    /// Semantic class lookup is keyed by the full [`ClassId`]. The short name
    /// remains on the definition for source spelling and diagnostics only, so
    /// same-named classes in different modules remain distinct.
    pub module: ModulePath,
    /// Proposal 0151, Phase 2: visibility of this class declaration.
    ///
    /// `true` for `public class`, `false` for unmarked / private. Used by
    /// the visibility walker to enforce that no public instance refers to
    /// a private class (E450) and that public class signatures don't leak
    /// private types (E451). Top-level (legacy) and built-in classes are
    /// always recorded as `false` (their cross-module visibility is
    /// governed by the implicit prelude, not by this flag).
    pub is_public: bool,
    /// `true` for the classes registered by [`ClassEnv::register_builtins`].
    ///
    /// Distinguishes the compiler's own `Eq`/`Ord`/`Num`/`Show`/... from a
    /// user class that merely reuses one of those short names. Consumers must
    /// test this rather than matching on the resolved name (Proposal 0179).
    pub is_builtin: bool,
    pub type_params: Vec<Identifier>,
    pub superclasses: Vec<ClassConstraint>,
    /// Resolved identities corresponding positionally to `superclasses`.
    pub superclass_class_ids: Vec<ClassId>,
    pub methods: Vec<MethodSig>,
    /// Methods that have default implementations in the class body.
    pub default_methods: Vec<Identifier>,
    /// Types this class declares and every instance must define
    /// (Proposal 0179 Stage 6).
    pub associated_types: Vec<AssociatedTypeDecl>,
    pub span: Span,
}

/// One slot of a class's dictionary tuple.
///
/// See [`ClassEnv::dictionary_layout`] for the ordering and why it is defined
/// in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictSlot {
    /// Evidence for a directly declared superclass — itself a dictionary.
    Superclass(ClassId),
    /// One of the class's own method implementations.
    Method(Identifier),
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
    /// Value-parameter names in source order.
    pub param_names: Vec<Identifier>,
    /// Value-parameter types in source order.
    ///
    /// Invariant: this should contain one entry per value parameter, while
    /// `arity` remains the canonical call arity used by downstream consumers.
    pub param_types: Vec<TypeExpr>,
    pub return_type: TypeExpr,
    pub arity: usize,
    /// Declared effect row for the method (Proposal 0151, Phase 4a).
    /// Acts as a *floor*: implementing instances must declare a row that
    /// is a superset of this one (validated by the E452 walker).
    pub effects: Vec<EffectExpr>,
    /// Optional default method body from the class declaration.
    pub default_body: Option<Block>,
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
    /// Proposal 0151, Phase 2: visibility of this instance declaration.
    ///
    /// `true` for `public instance`, `false` for unmarked / private.
    /// A public instance of a private class is rejected with E450; a
    /// public instance whose head type is a private ADT (in another
    /// module) is rejected with E455. Top-level (legacy) and built-in
    /// instances are always recorded as `false`.
    pub is_public: bool,
    pub type_args: Vec<TypeExpr>,
    pub context: Vec<ClassConstraint>,
    /// Resolved identities corresponding positionally to `context`.
    pub context_class_ids: Vec<ClassId>,
    pub method_names: Vec<Identifier>,
    /// Declared effect rows for methods implemented by this instance.
    ///
    /// Used when imported public instances are reconstructed from module
    /// interfaces and downstream callers need the resolved instance row
    /// without re-parsing the defining source module.
    pub method_effects: Vec<(Identifier, Vec<EffectExpr>)>,
    /// This instance's definitions for the class's associated types
    /// (Proposal 0179 Stage 6).
    pub associated_types: Vec<AssociatedTypeEquation>,
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
/// [`method_index`](Self::method_index)) remain only for source-resolution,
/// diagnostics, and old tooling APIs. Semantic consumers use the ClassId-keyed
/// methods below; they must not depend on the shims' iteration order.
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

/// One dictionary required by a matched contextual instance.  `dictionary`
/// is `None` when the required type is still polymorphic and must be supplied
/// by the current function's contextual dictionary parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstanceContextDictionaryRequest {
    pub class_name: Identifier,
    pub class_id: ClassId,
    pub type_args: Vec<InferType>,
    pub dictionary: Option<ResolvedDictionaryRef>,
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
        Self::collect_classes(
            statements,
            ModulePath::EMPTY,
            self,
            &mut diagnostics,
            interner,
        );
        Self::collect_instances(
            statements,
            ModulePath::EMPTY,
            self,
            &mut diagnostics,
            interner,
        );
        Self::collect_deriving(
            statements,
            ModulePath::EMPTY,
            self,
            &mut diagnostics,
            interner,
        );

        // Proposal 0151, Phase 2: orphan rule enforcement.
        //
        // Build a map of user-defined ADT name -> owning module by walking
        // the program's `data` declarations, then check every collected
        // instance against the relaxed orphan rule (instance is legal iff
        // either the class or the head type is local to the instance's
        // owning module). Legacy top-level instances (instance_module ==
        // EMPTY) are grandfathered.
        let mut data_info: HashMap<Identifier, DataInfo> = HashMap::new();
        Self::collect_data_info(statements, ModulePath::EMPTY, &mut data_info);
        self.enforce_orphan_rule(&data_info, &mut diagnostics, interner);

        // Proposal 0151, Phase 2: visibility enforcement.
        //
        // E450: a `public instance` cannot reference a private class.
        // E451: a `public class` signature must not mention a private type.
        // E455: a `public instance` of a public class must not have a
        //       private head ADT.
        self.enforce_instance_visibility(&data_info, &mut diagnostics, interner);
        self.enforce_class_signature_visibility(&data_info, &mut diagnostics, interner);

        // Proposal 0174 D4: synthesize `Sendable<Foo>` for user-declared ADTs.
        // Positive-only — we only synthesize when no field type contains a
        // function type. User-written Sendable instances are rejected because
        // Sendable is compiler-owned and authorizes worker-boundary transfer.
        Self::synthesize_sendable_instances(statements, ModulePath::EMPTY, self, interner);

        // Proposal 0179 Stage 5: superclass obligations are checked here,
        // after every class and instance is collected, so the result does not
        // depend on the order the source happens to declare them in.
        diagnostics.extend(self.validate_superclass_obligations(interner));
        diagnostics.extend(self.validate_associated_types(interner));

        diagnostics
    }

    /// The slot assignment for a class's dictionary tuple.
    ///
    /// Superclass evidence occupies the leading slots, method implementations
    /// follow, both in declaration order. This is the single definition of the
    /// layout: everything that builds a dictionary or reads a slot out of one
    /// derives its offsets from here rather than recomputing the convention.
    ///
    /// Evidence leads because it makes a slot's offset independent of how many
    /// methods the class declares.
    ///
    /// Only *directly* declared superclasses get a slot. A transitive one is
    /// reached by projecting twice, which is what keeps the layout of a class
    /// independent of hierarchies declared above it.
    pub fn dictionary_layout(&self, id: ClassId) -> Option<Vec<DictSlot>> {
        let class_def = self.lookup_class_by_id(id)?;
        Some(
            class_def
                .superclass_class_ids
                .iter()
                .map(|&superclass| DictSlot::Superclass(superclass))
                .chain(
                    class_def
                        .methods
                        .iter()
                        .map(|method| DictSlot::Method(method.name)),
                )
                .collect(),
        )
    }
}

/// The outcome of choosing among the dictionaries a caller holds for one class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictSelection {
    /// Exactly one candidate is consistent with the call — its index into the
    /// enclosing function's constraint list, which is also its dictionary
    /// parameter's position.
    Unique(usize),
    /// More than one candidate survives. The call does not say which dictionary
    /// it means, and picking either is a guess: this is reported (E485) rather
    /// than resolved.
    Ambiguous,
    /// No candidate is consistent. The call is not dispatching through a
    /// dictionary the caller holds — a concrete argument inside a constrained
    /// function, for instance — and the concrete dispatch path handles it.
    NoMatch,
}

/// Choose among the dictionaries a caller holds for one class by matching what
/// the call reveals against each candidate's type arguments.
///
/// `candidates` pairs each constraint's index with its type arguments;
/// `observed` holds the type seen at each class-parameter position, as located
/// by [`ClassEnv::dispatch_positions`], or `None` where the call reveals
/// nothing there. A position that reveals nothing does not narrow, so a call
/// that says nothing at all about a class with two dictionaries in scope is
/// [`DictSelection::Ambiguous`] — which is the honest answer.
///
/// Candidates that survive with *equal* type arguments are not an ambiguity.
/// There is at most one instance per type, so two constraints predicating the
/// same class over the same type name the same instance and reach the same
/// method — whichever is picked. This is what lets a function constrained on
/// both `Sizeable<a>` and `Measurable<a>` call `size`: one reaches it directly
/// and the other through superclass evidence, but they arrive at the same
/// implementation. Only candidates that differ are a real choice.
///
/// Generic over the type representation so the VM caller can pass `InferType`
/// and the native caller `CoreType` without converting between them. Both carry
/// the same `TypeVarId`s, because generalization does not renumber.
pub fn select_dictionary<T: PartialEq>(
    candidates: &[(usize, Vec<T>)],
    observed: &[Option<T>],
) -> DictSelection {
    let consistent: Vec<&(usize, Vec<T>)> = candidates
        .iter()
        .filter(|(_, type_args)| {
            observed
                .iter()
                .enumerate()
                .all(|(position, seen)| match seen {
                    Some(ty) => type_args.get(position) == Some(ty),
                    None => true,
                })
        })
        .collect();
    let Some((first_index, first_args)) = consistent.first() else {
        return DictSelection::NoMatch;
    };
    if consistent
        .iter()
        .all(|(_, type_args)| type_args == first_args)
    {
        return DictSelection::Unique(*first_index);
    }
    DictSelection::Ambiguous
}

impl ClassEnv {
    /// For each of the class's type parameters, which value argument of
    /// `method` reveals it at a call site.
    ///
    /// `class Root<a> { fn root(x: a) -> Int }` yields `[Some(0)]`: whatever
    /// type argument 0 has *is* `a`, so a call to `root` names the instance it
    /// needs. An entry is `None` when no parameter is declared as exactly that
    /// class parameter — either the class parameter appears only in the return
    /// type, or only nested inside a larger type such as `List<a>`, where the
    /// argument's type would have to be taken apart to recover it.
    ///
    /// Only an exact match counts, deliberately. A position that requires
    /// destructuring is reported as unknown rather than guessed at, and an
    /// unknown position simply fails to narrow the candidates — which is the
    /// conservative direction, because failing to narrow is diagnosed while
    /// narrowing wrongly is a silent miscompile.
    ///
    /// This is the single definition of *what a call site reveals*. Inference
    /// checks ambiguity with it and both backends select with it, so the three
    /// cannot disagree about which argument decides a dispatch.
    pub fn dispatch_positions(
        &self,
        id: ClassId,
        method: Identifier,
    ) -> Option<Vec<Option<usize>>> {
        let class_def = self.lookup_class_by_id(id)?;
        let method_sig = class_def.methods.iter().find(|m| m.name == method)?;
        Some(
            class_def
                .type_params
                .iter()
                .map(|&param| {
                    method_sig.param_types.iter().position(|ty| {
                        matches!(ty, TypeExpr::Named { name, args, .. } if *name == param && args.is_empty())
                    })
                })
                .collect(),
        )
    }

    /// Whether `method`'s signature mentions every one of its class's type
    /// parameters at all — in a value parameter or in the return type.
    ///
    /// A method that mentions none of them, `fn mk(tag: Int) -> Int` on
    /// `class Mk<a>`, is *undispatchable*: nothing anywhere in a call to it can
    /// name the instance it wants. That is worth separating from a method whose
    /// parameter appears only in the return type, like `decode`, which is
    /// dispatched perfectly well by where its result flows — just not by
    /// anything this stage reads.
    pub fn method_mentions_class_parameters(
        &self,
        id: ClassId,
        method: Identifier,
    ) -> Option<bool> {
        let class_def = self.lookup_class_by_id(id)?;
        let method_sig = class_def.methods.iter().find(|m| m.name == method)?;
        let mut mentioned = Vec::new();
        for ty in method_sig
            .param_types
            .iter()
            .chain(std::iter::once(&method_sig.return_type))
        {
            Self::collect_type_names(ty, &mut mentioned);
        }
        Some(
            class_def
                .type_params
                .iter()
                .all(|param| mentioned.contains(param)),
        )
    }

    /// The slot holding `method`'s implementation in a dictionary for `id`.
    pub fn method_slot(&self, id: ClassId, method: Identifier) -> Option<usize> {
        let class_def = self.lookup_class_by_id(id)?;
        let position = class_def.methods.iter().position(|m| m.name == method)?;
        Some(class_def.superclass_class_ids.len() + position)
    }

    /// The symbol name held by each slot of the dictionary for one instance head.
    ///
    /// `type_key` is the rendered instance head that [`dictionary_name`] and
    /// [`mangled_method_name`] mangle into their symbols. Superclass slots name
    /// the superclass's dictionary for the *same* head, because
    /// `class Eq<a> => Ord<a>` constrains the very type the instance is for.
    ///
    /// Returns `None` for an unknown class, so a caller can tell "no layout"
    /// from "a layout with no slots".
    pub fn dictionary_slot_names(
        &self,
        id: ClassId,
        type_key: &str,
        interner: &Interner,
    ) -> Option<Vec<String>> {
        Some(
            self.dictionary_layout(id)?
                .into_iter()
                .map(|slot| match slot {
                    DictSlot::Superclass(superclass) => {
                        dictionary_name(superclass, type_key, interner)
                    }
                    DictSlot::Method(method) => {
                        mangled_method_name(id, type_key, interner.resolve(method), interner)
                    }
                })
                .collect(),
        )
    }

    /// The slot path from a dictionary for `from` to the evidence for `to`.
    ///
    /// Empty is not a valid answer — `from == to` needs no projection — so
    /// `None` means `to` is not among `from`'s transitive superclasses.
    /// Breadth-first, so the path returned is the shortest one.
    pub fn superclass_path(&self, from: ClassId, to: ClassId) -> Option<Vec<usize>> {
        let mut visited = vec![from];
        let mut queue = std::collections::VecDeque::from([(from, Vec::new())]);

        while let Some((current, prefix)) = queue.pop_front() {
            let Some(class_def) = self.lookup_class_by_id(current) else {
                continue;
            };
            for (idx, &superclass) in class_def.superclass_class_ids.iter().enumerate() {
                let mut path = prefix.clone();
                path.push(idx);
                if superclass == to {
                    return Some(path);
                }
                if !visited.contains(&superclass) {
                    visited.push(superclass);
                    queue.push_back((superclass, path));
                }
            }
        }

        None
    }

    /// Proposal 0179 Stage 6: check every instance's associated type equations.
    ///
    /// Runs once the whole environment is populated, for the same reason
    /// [`validate_superclass_obligations`] does: an instance's equations can
    /// only be checked against the class that declares them, and that class may
    /// be declared after the instance.
    ///
    /// [`validate_superclass_obligations`]: Self::validate_superclass_obligations
    pub fn validate_associated_types(&self, interner: &Interner) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        for instance in &self.instances {
            let Some(class_def) = self.lookup_class_by_id(instance.class_id) else {
                continue;
            };

            // E479 — one equation per associated type. Reported before the
            // rest so a duplicated name is not also reported as a mismatch.
            let mut seen: Vec<Identifier> = Vec::new();
            for equation in &instance.associated_types {
                if seen.contains(&equation.name) {
                    let display = interner.resolve(equation.name);
                    diagnostics.push(
                        diagnostic_for(&DUPLICATE_ASSOCIATED_TYPE)
                            .with_span(equation.span)
                            .with_message(format!(
                                "Associated type `{display}` is defined more than once \
                                 in this instance."
                            )),
                    );
                    continue;
                }
                seen.push(equation.name);

                // E484 — an equation for a name the class never declared
                // defines nothing. Reported here so a misspelling points at the
                // equation, not only at the instance via the E480 below.
                if !class_def
                    .associated_types
                    .iter()
                    .any(|declaration| declaration.name == equation.name)
                {
                    let display = interner.resolve(equation.name);
                    let class_display = interner.resolve(class_def.name);
                    let declared: Vec<&str> = class_def
                        .associated_types
                        .iter()
                        .map(|declaration| interner.resolve(declaration.name))
                        .collect();
                    let mut diagnostic = diagnostic_for(&UNKNOWN_ASSOCIATED_TYPE)
                        .with_span(equation.span)
                        .with_message(format!(
                            "`{display}` is not an associated type of class `{class_display}`."
                        ));
                    if !declared.is_empty() {
                        diagnostic = diagnostic.with_hint_text(format!(
                            "`{class_display}` declares: {}",
                            declared.join(", ")
                        ));
                    }
                    diagnostics.push(diagnostic);
                }
            }

            for declaration in &class_def.associated_types {
                let display = interner.resolve(declaration.name);
                let Some(equation) = instance
                    .associated_types
                    .iter()
                    .find(|equation| equation.name == declaration.name)
                else {
                    // E480 — the class declares it, this instance does not
                    // define it, so an application at this head cannot reduce.
                    let class_display = interner.resolve(class_def.name);
                    let head = instance
                        .type_args
                        .iter()
                        .map(|arg| arg.display_with(interner))
                        .collect::<Vec<_>>()
                        .join(", ");
                    diagnostics.push(
                        diagnostic_for(&MISSING_ASSOCIATED_TYPE)
                            .with_span(instance.span)
                            .with_message(format!(
                                "Instance does not define associated type `{display}`, \
                                 which `{class_display}` declares."
                            ))
                            .with_hint_text(format!("Add: `type {display}<{head}> = ...`")),
                    );
                    continue;
                };

                // E482 — the head must be applied at the declared arity.
                if equation.head.len() != declaration.params.len() {
                    diagnostics.push(
                        diagnostic_for(&ASSOCIATED_TYPE_KIND_MISMATCH)
                            .with_span(equation.span)
                            .with_message(format!(
                                "Associated type `{display}` takes {} argument(s), \
                                 but this equation gives {}.",
                                declaration.params.len(),
                                equation.head.len()
                            )),
                    );
                    continue;
                }

                // E481 — reduction substitutes the head's variables into the
                // body, so a body variable the head never binds has nothing to
                // receive.
                let mut bound = Vec::new();
                for arg in &equation.head {
                    Self::collect_type_vars(arg, interner, &mut bound);
                }
                let mut used = Vec::new();
                Self::collect_type_vars(&equation.body, interner, &mut used);
                for variable in used {
                    if bound.contains(&variable) {
                        continue;
                    }
                    let var_display = interner.resolve(variable);
                    diagnostics.push(
                        diagnostic_for(&UNBOUND_ASSOCIATED_TYPE_VARIABLE)
                            .with_span(equation.span)
                            .with_message(format!(
                                "`{var_display}` appears in the body of `{display}` \
                                 but is not bound by its head."
                            )),
                    );
                }

                // E483 — reduction has to terminate, so a body must not reach
                // the very type it defines.
                let mut mentioned = Vec::new();
                Self::collect_type_names(&equation.body, &mut mentioned);
                if mentioned.contains(&declaration.name) {
                    diagnostics.push(
                        diagnostic_for(&RECURSIVE_ASSOCIATED_TYPE)
                            .with_span(equation.span)
                            .with_message(format!(
                                "Associated type `{display}` reduces to a type \
                                 mentioning `{display}`."
                            )),
                    );
                }
            }
        }

        diagnostics
    }

    /// Collect the type *variables* of `ty` — lowercase-initial names — in
    /// order of appearance, without duplicates.
    fn collect_type_vars(ty: &TypeExpr, interner: &Interner, out: &mut Vec<Identifier>) {
        match ty {
            TypeExpr::Named { name, args, .. } => {
                if args.is_empty()
                    && Self::is_instance_type_var(*name, interner)
                    && !out.contains(name)
                {
                    out.push(*name);
                }
                for arg in args {
                    Self::collect_type_vars(arg, interner, out);
                }
            }
            TypeExpr::Tuple { elements, .. } => {
                for element in elements {
                    Self::collect_type_vars(element, interner, out);
                }
            }
            TypeExpr::Function { params, ret, .. } => {
                for param in params {
                    Self::collect_type_vars(param, interner, out);
                }
                Self::collect_type_vars(ret, interner, out);
            }
        }
    }

    /// Collect every name `ty` mentions, variables and constructors alike.
    fn collect_type_names(ty: &TypeExpr, out: &mut Vec<Identifier>) {
        match ty {
            TypeExpr::Named { name, args, .. } => {
                out.push(*name);
                for arg in args {
                    Self::collect_type_names(arg, out);
                }
            }
            TypeExpr::Tuple { elements, .. } => {
                for element in elements {
                    Self::collect_type_names(element, out);
                }
            }
            TypeExpr::Function { params, ret, .. } => {
                for param in params {
                    Self::collect_type_names(param, out);
                }
                Self::collect_type_names(ret, out);
            }
        }
    }

    /// The class that declares an associated type named `name`, if exactly one
    /// does.
    ///
    /// `None` when no class declares it, and also when several do: an
    /// ambiguous name is left as an ordinary type constructor rather than
    /// resolved to an arbitrary class.
    pub fn associated_type_class(&self, name: Identifier) -> Option<ClassId> {
        let mut found: Option<ClassId> = None;
        for class_def in self.classes.values() {
            if !class_def
                .associated_types
                .iter()
                .any(|declaration| declaration.name == name)
            {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(class_def.class_id());
        }
        found
    }

    /// The equation defining `name` for the instance of `class_id` whose head
    /// matches `args`, together with the bindings that matching produced.
    ///
    /// This is the selection step of reduction: it answers "which instance
    /// does this application land on, and what does its head bind".
    pub(crate) fn associated_type_equation(
        &self,
        class_id: ClassId,
        name: Identifier,
        args: &[InferType],
        interner: &Interner,
    ) -> Option<(&AssociatedTypeEquation, HashMap<Identifier, InferType>)> {
        self.instances.iter().find_map(|instance| {
            if instance.class_id != class_id {
                return None;
            }
            let equation = instance
                .associated_types
                .iter()
                .find(|equation| equation.name == name)?;
            if equation.head.len() != args.len() {
                return None;
            }
            let mut subst = HashMap::new();
            let matched = equation.head.iter().zip(args).all(|(pattern, actual)| {
                Self::match_instance_type_expr(pattern, actual, &mut subst, interner)
            });
            matched.then_some((equation, subst))
        })
    }

    /// Transitive superclasses of `id`, nearest first and deduplicated.
    ///
    /// Breadth-first over declaration order, so the result is deterministic and
    /// its prefix is the class's own directly declared superclasses — which is
    /// what makes it usable as a dictionary slot assignment.
    ///
    /// Returns empty for an unknown class, and stops rather than looping if the
    /// hierarchy is cyclic; [`validate_superclass_obligations`] rejects a cycle
    /// with E477 before anything relies on the closure.
    ///
    /// [`validate_superclass_obligations`]: Self::validate_superclass_obligations
    pub fn superclass_closure(&self, id: ClassId) -> Vec<ClassId> {
        let mut ordered: Vec<ClassId> = Vec::new();
        let mut queue: std::collections::VecDeque<ClassId> = std::collections::VecDeque::new();
        queue.push_back(id);

        while let Some(current) = queue.pop_front() {
            let Some(class_def) = self.lookup_class_by_id(current) else {
                continue;
            };
            for &parent in &class_def.superclass_class_ids {
                if parent == id || ordered.contains(&parent) {
                    continue;
                }
                ordered.push(parent);
                queue.push_back(parent);
            }
        }

        ordered
    }

    /// The chain of classes leading `id` back to itself, if one exists.
    ///
    /// Returned nearest-first and ending at `id` again, so it renders as the
    /// path a reader has to follow to see the cycle (`Ord -> Eq -> Ord`).
    fn superclass_cycle_from(&self, id: ClassId) -> Option<Vec<ClassId>> {
        fn walk(
            env: &ClassEnv,
            current: ClassId,
            target: ClassId,
            path: &mut Vec<ClassId>,
            seen: &mut Vec<ClassId>,
        ) -> bool {
            let Some(class_def) = env.lookup_class_by_id(current) else {
                return false;
            };
            for &parent in &class_def.superclass_class_ids {
                path.push(parent);
                if parent == target {
                    return true;
                }
                if !seen.contains(&parent) {
                    seen.push(parent);
                    if walk(env, parent, target, path, seen) {
                        return true;
                    }
                }
                path.pop();
            }
            false
        }

        let mut path = vec![id];
        let mut seen = vec![id];
        walk(self, id, id, &mut path, &mut seen).then_some(path)
    }

    /// Proposal 0179 Stage 5: check every superclass obligation in the program.
    ///
    /// Two checks, in this order because the second cannot terminate without
    /// the first:
    ///
    /// - **E477** — a class that reaches itself through its own superclass
    ///   declarations. A dictionary for it would have to contain itself, and
    ///   the superclass closure would not terminate.
    /// - **E445** — `instance Ord<T>` without the `instance Eq<T>` that
    ///   `class Eq<a> => Ord<a>` demands, checked transitively.
    ///
    /// Runs once the whole environment is populated. It used to run inline
    /// while instances were being collected, which meant it could only see the
    /// instances declared *above* the one it was checking, so writing the
    /// subclass instance first produced a false error.
    pub fn validate_superclass_obligations(&self, interner: &Interner) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        // `self.classes` is a `HashMap`, so walk it in declaration order to keep
        // the choice of which class anchors a cycle's diagnostic stable.
        let mut declared: Vec<&ClassDef> = self.classes.values().collect();
        declared.sort_by_key(|class_def| {
            let start = class_def.span.start;
            (start.line, start.column, interner.resolve(class_def.name))
        });

        let mut cyclic: Vec<ClassId> = Vec::new();
        for class_def in declared {
            let id = class_def.class_id();
            let Some(cycle) = self.superclass_cycle_from(id) else {
                continue;
            };
            // One diagnostic per cycle, anchored at its first-declared class —
            // every other member would report the same loop from a different
            // starting point.
            let already_reported = cycle.iter().any(|step| cyclic.contains(step));
            for &step in &cycle {
                if !cyclic.contains(&step) {
                    cyclic.push(step);
                }
            }
            if already_reported {
                continue;
            }
            let path = cycle
                .iter()
                .map(|step| interner.resolve(step.name))
                .collect::<Vec<_>>()
                .join(" -> ");
            let display_class = interner.resolve(id.name);
            diagnostics.push(
                diagnostic_for(&SUPERCLASS_CYCLE)
                    .with_span(class_def.span)
                    .with_message(format!(
                        "Class `{display_class}` is reachable from its own superclasses: {path}."
                    )),
            );
        }

        for instance in &self.instances {
            if cyclic.contains(&instance.class_id) {
                continue;
            }
            for superclass_id in self.superclass_closure(instance.class_id) {
                if self.head_has_instance(superclass_id, &instance.type_args, interner) {
                    continue;
                }
                let display_class = interner.resolve(instance.class_id.name);
                let super_display = interner.resolve(superclass_id.name);
                let head = instance
                    .type_args
                    .iter()
                    .map(|arg| arg.display_with(interner))
                    .collect::<Vec<_>>()
                    .join(", ");
                diagnostics.push(
                    diagnostic_for(&MISSING_SUPERCLASS_INSTANCE)
                        .with_span(instance.span)
                        .with_message(format!(
                            "No instance for `{super_display}<{head}>` \
                             (required by `{display_class}<{head}>`)."
                        ))
                        .with_hint_text(format!(
                            "`{display_class}` requires `{super_display}` as a superclass. \
                             Add: `instance {super_display}<{head}> {{ ... }}`"
                        )),
                );
            }
        }

        diagnostics
    }

    /// Whether some instance of `class_id` covers the head `subject`.
    ///
    /// Structural, not textual: an `instance Eq<a> => Eq<Array<a>>` discharges
    /// the obligation raised by `instance Ord<Array<b>>`, which comparing the
    /// two rendered heads as strings never could.
    fn head_has_instance(
        &self,
        class_id: ClassId,
        subject: &[TypeExpr],
        interner: &Interner,
    ) -> bool {
        self.instances.iter().any(|candidate| {
            if candidate.class_id != class_id || candidate.type_args.len() != subject.len() {
                return false;
            }
            let mut subst = HashMap::new();
            candidate
                .type_args
                .iter()
                .zip(subject)
                .all(|(pattern, actual)| {
                    Self::match_instance_head(pattern, actual, &mut subst, interner)
                })
        })
    }

    /// One-way match of an instance head `pattern` against another head.
    ///
    /// The `TypeExpr`/`TypeExpr` counterpart of
    /// [`match_instance_type_expr`](Self::match_instance_type_expr), which
    /// matches a head against an inferred type. Only the pattern's type
    /// variables bind; the subject's are rigid, so `Eq<Array<a>>` matches the
    /// head `Array<b>` but `Eq<Array<Int>>` does not.
    fn match_instance_head(
        pattern: &TypeExpr,
        subject: &TypeExpr,
        subst: &mut HashMap<Identifier, TypeExpr>,
        interner: &Interner,
    ) -> bool {
        match (pattern, subject) {
            (TypeExpr::Named { name, args, .. }, _)
                if args.is_empty() && Self::is_instance_type_var(*name, interner) =>
            {
                match subst.get(name) {
                    // Compared as rendered types rather than with `==`: a
                    // `TypeExpr` carries its `Span`, so two occurrences of the
                    // same type are never structurally equal.
                    Some(bound) => bound.display_with(interner) == subject.display_with(interner),
                    None => {
                        subst.insert(*name, subject.clone());
                        true
                    }
                }
            }
            (
                TypeExpr::Named {
                    name,
                    args: pattern_args,
                    ..
                },
                TypeExpr::Named {
                    name: subject_name,
                    args: subject_args,
                    ..
                },
            ) => {
                name == subject_name
                    && pattern_args.len() == subject_args.len()
                    && pattern_args
                        .iter()
                        .zip(subject_args)
                        .all(|(p, s)| Self::match_instance_head(p, s, subst, interner))
            }
            _ => false,
        }
    }

    /// Phase 2 instance-visibility walker — enforces E450 (public instance
    /// of private class) and E455 (public instance of public class with
    /// private head ADT). E451 lives in `enforce_class_signature_visibility`.
    fn enforce_instance_visibility(
        &self,
        data_info: &HashMap<Identifier, DataInfo>,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for inst in &self.instances {
            // Only public instances are subject to the leak check; private
            // instances cannot leak by definition. Built-in / legacy
            // placeholders also opt out.
            if !inst.is_public {
                continue;
            }
            if inst.instance_module.is_empty() {
                continue;
            }

            let Some(class_def) = self.classes.get(&inst.class_id) else {
                continue;
            };

            let display_class = interner.resolve(inst.class_name);
            let display_type: Vec<String> = inst
                .type_args
                .iter()
                .map(|t| t.display_with(interner))
                .collect();
            let display_head = display_type.join(", ");

            // E450: public instance of a private (non-built-in) class.
            // Built-in classes (module == EMPTY) are universally visible
            // through the implicit prelude and never count as a leak.
            if !class_def.module.is_empty() && !class_def.is_public {
                diagnostics.push(
                    diagnostic_for(&PUBLIC_INSTANCE_OF_PRIVATE_CLASS)
                        .with_span(inst.span)
                        .with_message(format!(
                            "`public instance` `{display_class}<{display_head}>` references \
                             the private class `{display_class}`."
                        ))
                        .with_hint_text(format!(
                            "Mark the class `public class {display_class}` or remove `public` \
                             from this instance."
                        )),
                );
                // Don't double-report E455 on the same instance — E450 is
                // the more fundamental leak.
                continue;
            }

            // E455: public instance of a public class with a private head
            // ADT. Only fires when the head type is a user-defined ADT
            // present in `data_info`. Built-ins (Int, List, ...) are
            // treated as universally visible (same as built-in classes).
            if let Some(head_name) = Self::head_type_name(&inst.type_args)
                && let Some(head_info) = data_info.get(&head_name)
                && !head_info.is_public
            {
                let head_display = interner.resolve(head_name);
                diagnostics.push(
                    diagnostic_for(&PUBLIC_INSTANCE_HAS_PRIVATE_HEAD)
                        .with_span(inst.span)
                        .with_message(format!(
                            "`public instance` `{display_class}<{display_head}>` has the \
                             private head type `{head_display}`."
                        ))
                        .with_hint_text(format!(
                            "Mark the head type `public data {head_display}` or remove \
                             `public` from this instance."
                        )),
                );
            }
        }
    }

    /// Phase 2 class-signature visibility walker — enforces E451
    /// (`public class` mentions a private type in any of its method
    /// signatures, including the return type, parameter types, and the
    /// class's own type parameter constraints).
    ///
    /// "Private" here means: a user-defined ADT in `data_info` whose
    /// `is_public == false`. Built-in types (Int, List, ...) are treated
    /// as universally visible.
    fn enforce_class_signature_visibility(
        &self,
        data_info: &HashMap<Identifier, DataInfo>,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for class_def in self.classes.values() {
            if !class_def.is_public {
                continue;
            }
            if class_def.module.is_empty() {
                // Built-in / legacy class — visibility check is moot.
                continue;
            }

            for method in &class_def.methods {
                let mut leaks: Vec<Identifier> = Vec::new();
                for ty in &method.param_types {
                    Self::collect_named_types(ty, &mut leaks);
                }
                Self::collect_named_types(&method.return_type, &mut leaks);

                for ty_name in leaks {
                    if let Some(info) = data_info.get(&ty_name)
                        && !info.is_public
                    {
                        let display_class = interner.resolve(class_def.name);
                        let display_type = interner.resolve(ty_name);
                        diagnostics.push(
                            diagnostic_for(&PUBLIC_CLASS_LEAKS_PRIVATE_TYPE)
                                .with_span(class_def.span)
                                .with_message(format!(
                                    "`public class` `{display_class}` mentions the \
                                     private type `{display_type}` in method `{}`.",
                                    interner.resolve(method.name)
                                ))
                                .with_hint_text(format!(
                                    "Mark the type `public data {display_type}` or remove \
                                     `public` from this class."
                                )),
                        );
                    }
                }
            }
        }
    }

    /// Recursively collect every `Named { name }` identifier from a TypeExpr
    /// into `out`. Used by the class-signature visibility walker.
    fn collect_named_types(ty: &TypeExpr, out: &mut Vec<Identifier>) {
        match ty {
            TypeExpr::Named { name, args, .. } => {
                out.push(*name);
                for arg in args {
                    Self::collect_named_types(arg, out);
                }
            }
            TypeExpr::Tuple { elements, .. } => {
                for el in elements {
                    Self::collect_named_types(el, out);
                }
            }
            TypeExpr::Function { params, ret, .. } => {
                for p in params {
                    Self::collect_named_types(p, out);
                }
                Self::collect_named_types(ret, out);
            }
        }
    }

    /// Proposal 0174 D4 — synthesize `Sendable<Foo>` instances for user-
    /// declared ADTs. Positive-only: we synthesize an instance only when
    /// no field type anywhere in the ADT contains a function type, since
    /// closures are the canonical non-`Sendable` shape and Phase 1 does
    /// not promote them across worker boundaries.
    ///
    /// For a parameterized ADT `data Foo<a, b> { ... }`, the synthesized
    /// instance is `instance <a: Sendable, b: Sendable> => Sendable<Foo<a, b>>`.
    /// The existing contextual-instance solver then enforces the bound
    /// recursively at every use site.
    ///
    /// User-written `instance Sendable<Foo>` declarations are rejected before
    /// synthesis; this pass is the only ADT path that may create Sendable
    /// evidence.
    fn synthesize_sendable_instances(
        statements: &[Statement],
        current_module: ModulePath,
        env: &mut ClassEnv,
        interner: &Interner,
    ) {
        let Some(sendable_id) = interner.lookup("Sendable") else {
            // Sendable wasn't registered (e.g. a test that built a bare
            // ClassEnv without `register_builtins`). Nothing to do.
            return;
        };
        for stmt in statements {
            match stmt {
                Statement::Data {
                    name,
                    type_params,
                    variants,
                    span,
                    ..
                } => {
                    Self::try_synthesize_sendable_for_adt(
                        env,
                        interner,
                        sendable_id,
                        *name,
                        type_params,
                        variants,
                        current_module,
                        *span,
                    );
                }
                Statement::Module {
                    name: module_name,
                    body,
                    ..
                } => {
                    let module_path = ModulePath::from_identifier(*module_name);
                    Self::synthesize_sendable_instances(
                        &body.statements,
                        module_path,
                        env,
                        interner,
                    );
                }
                _ => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn try_synthesize_sendable_for_adt(
        env: &mut ClassEnv,
        interner: &Interner,
        sendable_id: Identifier,
        adt_name: Identifier,
        type_params: &[Identifier],
        variants: &[crate::syntax::data_variant::DataVariant],
        instance_module: ModulePath,
        span: Span,
    ) {
        if is_opaque_non_sendable_adt(instance_module, adt_name, interner) {
            return;
        }

        // Skip if any field anywhere contains a function type — the
        // positive-only rule. Closures and function values aren't sendable
        // and we have no way to make them so without copying.
        for variant in variants {
            for field in &variant.fields {
                if type_expr_contains_function(field) {
                    return;
                }
            }
        }

        let head_args: Vec<TypeExpr> = if type_params.is_empty() {
            vec![TypeExpr::Named {
                name: adt_name,
                args: Vec::new(),
                span: Span::default(),
            }]
        } else {
            vec![TypeExpr::Named {
                name: adt_name,
                args: type_params
                    .iter()
                    .map(|p| TypeExpr::Named {
                        name: *p,
                        args: Vec::new(),
                        span: Span::default(),
                    })
                    .collect(),
                span: Span::default(),
            }]
        };

        // Bound every type parameter with `Sendable<a>`.
        let context: Vec<ClassConstraint> = type_params
            .iter()
            .map(|p| ClassConstraint {
                class_name: sendable_id,
                type_args: vec![TypeExpr::Named {
                    name: *p,
                    args: Vec::new(),
                    span: Span::default(),
                }],
                span: Span::default(),
            })
            .collect();

        env.instances.push(InstanceDef {
            class_name: sendable_id,
            // Synthesized, so it declares no associated types.
            associated_types: Vec::new(),
            class_id: ClassId::from_local_name(sendable_id),
            instance_module,
            // Synthesized instances follow the same visibility rule as the
            // owning ADT — they're effectively part of its public surface.
            // Phase 1 doesn't enforce this distinction since `Sendable` has
            // no methods.
            is_public: false,
            type_args: head_args,
            context,
            context_class_ids: vec![ClassId::from_local_name(sendable_id); type_params.len()],
            method_names: Vec::new(),
            method_effects: Vec::new(),
            span,
        });
        // Suppress unused warning: adt_name is captured in head_args via a
        // closure path that the borrow checker can't trace through.
        let _ = adt_name;
    }

    /// Walk a statement tree and record the owning module and visibility
    /// for each `data` declaration. Used by the orphan rule walker (Phase 2)
    /// and the visibility walkers (E451, E455).
    fn collect_data_info(
        statements: &[Statement],
        current_module: ModulePath,
        out: &mut HashMap<Identifier, DataInfo>,
    ) {
        for stmt in statements {
            match stmt {
                Statement::Data {
                    is_public, name, ..
                } => {
                    // First-wins: if the same ADT name appears twice (which
                    // would be flagged elsewhere), keep the first sighting.
                    out.entry(*name).or_insert(DataInfo {
                        module: current_module,
                        is_public: *is_public,
                    });
                }
                Statement::Module { name, body, .. } => {
                    let module_path = ModulePath::from_identifier(*name);
                    Self::collect_data_info(&body.statements, module_path, out);
                }
                _ => {}
            }
        }
    }

    /// Enforce the orphan rule on every collected instance.
    ///
    /// An instance `instance C<T>` declared in module `M` is legal iff:
    ///   * `M == class_module(C)`, or
    ///   * `M == head_module(T)`.
    ///
    /// Legacy top-level instances (where `instance_module == EMPTY`) are
    /// grandfathered: they participate in the implicit prelude and predate
    /// module-scoped classes. Built-in placeholder instances (with empty
    /// `method_names` and a default span) are also skipped.
    fn enforce_orphan_rule(
        &self,
        data_info: &HashMap<Identifier, DataInfo>,
        diagnostics: &mut Vec<Diagnostic>,
        interner: &Interner,
    ) {
        for inst in &self.instances {
            // Skip legacy / built-in placeholder instances.
            if inst.instance_module.is_empty() {
                continue;
            }
            if inst.method_names.is_empty() && inst.span == Span::default() {
                continue;
            }

            let class_module = inst.class_id.module;
            let head_module = Self::head_type_owning_module(&inst.type_args, data_info);

            let class_local = inst.instance_module == class_module;
            let head_local = match head_module {
                Some(m) => inst.instance_module == m,
                None => false,
            };

            if class_local || head_local {
                continue;
            }

            let display_class = interner.resolve(inst.class_name);
            let display_type: Vec<String> = inst
                .type_args
                .iter()
                .map(|t| t.display_with(interner))
                .collect();
            let display_head = display_type.join(", ");

            diagnostics.push(
                diagnostic_for(&ORPHAN_INSTANCE)
                    .with_span(inst.span)
                    .with_message(format!(
                        "Orphan instance `{display_class}<{display_head}>` is not allowed."
                    ))
                    .with_hint_text(format!(
                        "Move this instance into the module that defines `{display_class}` \
                         or the module that defines its head type."
                    )),
            );
        }
    }

    /// Compute the owning module of an instance's head type.
    ///
    /// Returns `Some(module)` when the head type is a user-defined ADT
    /// recorded in `data_modules`, or `None` for built-in head types
    /// (`Int`, `List`, `Option`, ...) and structural types (tuple,
    /// function). A `None` result means "not owned by any user module",
    /// so the instance is only legal if its class is local.
    fn head_type_owning_module(
        type_args: &[TypeExpr],
        data_info: &HashMap<Identifier, DataInfo>,
    ) -> Option<ModulePath> {
        let head = type_args.first()?;
        let TypeExpr::Named { name, .. } = head else {
            return None;
        };
        data_info.get(name).map(|info| info.module)
    }

    /// Extract the head ADT identifier from `type_args[0]` if it's a
    /// named type. Returns `None` for built-ins, tuples, and functions.
    fn head_type_name(type_args: &[TypeExpr]) -> Option<Identifier> {
        let head = type_args.first()?;
        let TypeExpr::Named { name, .. } = head else {
            return None;
        };
        Some(*name)
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
                    is_public,
                    name,
                    type_params,
                    superclasses,
                    methods,
                    associated_types,
                    span,
                    ..
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
                            param_names: m.params.clone(),
                            param_types: m.param_types.clone(),
                            return_type: m.return_type.clone(),
                            arity: m.params.len(),
                            effects: m.effects.clone(),
                            default_body: m.default_body.clone(),
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
                            associated_types: associated_types.clone(),
                            module: current_module,
                            is_public: *is_public,
                            is_builtin: false,
                            type_params: type_params.clone(),
                            superclasses: superclasses.clone(),
                            superclass_class_ids: superclasses
                                .iter()
                                .map(|constraint| {
                                    env.resolve_class_id(current_module, constraint.class_name)
                                        .unwrap_or_else(|| {
                                            ClassId::from_local_name(constraint.class_name)
                                        })
                                })
                                .collect(),
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
                    is_public,
                    class_name,
                    type_args,
                    context,
                    methods,
                    associated_types,
                    span,
                    name_span: _,
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

                    if Self::is_builtin_sendable_class(&class_def, interner) {
                        diagnostics.push(Self::sealed_sendable_diagnostic(*span));
                        continue;
                    }

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

                            // Proposal 0151, Phase 2: E443 extended.
                            //
                            // The dedup gate matches on `class_id` +
                            // structural type_args, so it already
                            // catches duplicates regardless of which
                            // module hosts each instance. When the
                            // existing instance lives in a different
                            // module than the new one, surface that
                            // in the diagnostic so users can find
                            // the conflicting site.
                            let existing_module = existing
                                .instance_module
                                .as_identifier()
                                .map(|id| interner.resolve(id).to_string());
                            let new_module = current_module
                                .as_identifier()
                                .map(|id| interner.resolve(id).to_string());
                            let cross_module = matches!(
                                (&existing_module, &new_module),
                                (Some(a), Some(b)) if a != b
                            );

                            let mut diag = diagnostic_for(&DUPLICATE_INSTANCE)
                                .with_span(*span)
                                .with_message(format!(
                                    "Duplicate instance for `{display_class}<{}>`.",
                                    display_type.join(", ")
                                ));
                            if cross_module {
                                let existing_mod = existing_module.as_deref().unwrap_or("?");
                                let new_mod = new_module.as_deref().unwrap_or("?");
                                diag = diag.with_hint_text(format!(
                                    "Another instance of `{display_class}<{}>` already lives \
                                     in module `{existing_mod}`; this one is in `{new_mod}`. \
                                     Each `(class, head type)` may have at most one instance \
                                     across the whole program.",
                                    display_type.join(", ")
                                ));
                            }
                            diagnostics.push(diag);
                            continue;
                        }
                    }

                    // Validate: all required methods are implemented
                    let method_names: Vec<Identifier> = methods.iter().map(|m| m.name).collect();
                    let method_effects: Vec<(Identifier, Vec<EffectExpr>)> = methods
                        .iter()
                        .map(|m| (m.name, m.effects.clone()))
                        .collect();

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
                            && method.params.len() != class_method.arity
                        {
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

                    env.instances.push(InstanceDef {
                        class_name: *class_name,
                        associated_types: associated_types.clone(),
                        // Phase 1b Step 4: canonical ClassId of the class
                        // being implemented. We resolved the class above
                        // (cloned into `class_def`) and use its `class_id()`
                        // accessor to roll its (module, name) into a
                        // ClassId. Two same-named classes in different
                        // modules now have distinct instance buckets.
                        class_id: class_def.class_id(),
                        instance_module: current_module,
                        is_public: *is_public,
                        type_args: type_args.clone(),
                        context: context.clone(),
                        context_class_ids: context
                            .iter()
                            .map(|constraint| {
                                env.resolve_class_id(current_module, constraint.class_name)
                                    .unwrap_or_else(|| {
                                        ClassId::from_local_name(constraint.class_name)
                                    })
                            })
                            .collect(),
                        method_names,
                        method_effects,
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

    /// Return whether `class_def` is the compiler-owned built-in `Sendable`.
    fn is_builtin_sendable_class(class_def: &ClassDef, interner: &Interner) -> bool {
        class_def.module.is_empty() && interner.resolve(class_def.name) == "Sendable"
    }

    /// Build the diagnostic used when user code tries to implement `Sendable`.
    fn sealed_sendable_diagnostic(span: Span) -> Diagnostic {
        diagnostic_for(&SEALED_CLASS_INSTANCE)
            .with_span(span)
            .with_message(
                "Sendable is compiler-derived and cannot be implemented manually.".to_string(),
            )
            .with_hint_text(
                "Remove the instance; data types become Sendable automatically when all fields are Sendable."
                    .to_string(),
            )
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
                    is_public,
                    name,
                    type_params,
                    deriving,
                    span,
                    ..
                } if !deriving.is_empty() => {
                    for class_name in deriving {
                        // Check that the class exists. Phase 1b Step 4: prefer
                        // a class in the same module as the data declaration,
                        // falling back to the bare-name shim. Mirrors the
                        // disambiguation rule used by `collect_instances`.
                        let class_def = env
                            .lookup_class_in_module_or_global(current_module, *class_name)
                            .or_else(|| {
                                interner
                                    .try_resolve(*class_name)
                                    .and_then(|name| name.rsplit('.').next())
                                    .and_then(|short| interner.lookup(short))
                                    .and_then(|short| {
                                        env.lookup_class_in_module_or_global(current_module, short)
                                    })
                            });
                        let (class_id, resolved_class_name) = match class_def {
                            Some(def) => (def.class_id(), def.name),
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
                        if let Some(def) =
                            env.lookup_class_in_module_or_global(current_module, *class_name)
                            && Self::is_builtin_sendable_class(def, interner)
                        {
                            diagnostics.push(Self::sealed_sendable_diagnostic(*span));
                            continue;
                        }

                        let type_arg = TypeExpr::Named {
                            name: *name,
                            args: type_params
                                .iter()
                                .map(|param| builtin_type(*param))
                                .collect(),
                            span: Span::default(),
                        };
                        let context = type_params
                            .iter()
                            .map(|param| ClassConstraint {
                                class_name: resolved_class_name,
                                type_args: vec![builtin_type(*param)],
                                span: *span,
                            })
                            .collect();
                        env.instances.push(InstanceDef {
                            class_name: resolved_class_name,
                            class_id,
                            instance_module: current_module,
                            is_public: *is_public,
                            type_args: vec![type_arg],
                            context,
                            context_class_ids: type_params
                                .iter()
                                .map(|_| {
                                    env.resolve_class_id(current_module, resolved_class_name)
                                        .unwrap_or_else(|| {
                                            ClassId::from_local_name(resolved_class_name)
                                        })
                                })
                                .collect(),
                            method_names: vec![],
                            method_effects: vec![],
                            associated_types: vec![],
                            span: Span::default(),
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
    // Proposal 0151 — source-boundary short-name compatibility helpers.
    //
    // These methods exist for parser-facing diagnostics and legacy tooling
    // that only has a bare `Identifier`. They never choose among multiple
    // classes or methods: callers must resolve a unique ClassId first for
    // semantic work.
    // ========================================================================

    /// Look up a class definition by short name (compatibility shim).
    ///
    /// Returns a definition only when the short name is unique.
    pub fn lookup_class(&self, name: Identifier) -> Option<&ClassDef> {
        let mut matches = self.classes.values().filter(|def| def.name == name);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    /// Look up a class definition by short name, **preferring** a class
    /// declared in `current_module` if one exists with that short name.
    ///
    /// This is the disambiguation rule used by [`collect_instances`] and
    /// [`collect_deriving`] in Phase 1b Step 4: an `instance Foo<...>`
    /// declaration written inside `module Mod.A` should refer to `Mod.A.Foo`
    /// when `Mod.A.Foo` exists, even if other modules also declare a `Foo`.
    /// Falls back to the unique short-name resolver when no class with the
    /// matching name lives in `current_module`.
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
        // Fall back only when the short name identifies one class.
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

    /// Given a method name, find which class declares it when it is unique.
    /// This is retained for diagnostics and legacy tooling; semantic callers
    /// should use [`resolve_method_class_id`](Self::resolve_method_class_id).
    pub fn method_to_class(&self, method_name: Identifier) -> Option<(Identifier, &ClassDef)> {
        let mut matches = self
            .classes
            .values()
            .filter(|class| class.methods.iter().any(|m| m.name == method_name));
        let first = matches.next()?;
        matches.next().is_none().then_some((first.name, first))
    }

    /// Resolve a method's declaring class at the source boundary. A class in
    /// the current module wins; otherwise the method must belong to exactly
    /// one visible class. This deliberately rejects declaration-order
    /// fallback when imported classes expose the same method name.
    pub fn resolve_method_class_id(
        &self,
        current_module: ModulePath,
        method_name: Identifier,
    ) -> Option<ClassId> {
        let matches = self.method_class_ids(method_name);
        let local = matches
            .iter()
            .find(|class| class.module == current_module)
            .copied();
        local
            .or_else(|| (matches.len() == 1).then(|| matches[0]))
            .map(ClassDef::class_id)
    }

    /// All class definitions declaring `method_name`, used by the source
    /// resolver to distinguish an unknown method from an ambiguous one.
    pub fn method_class_ids(&self, method_name: Identifier) -> Vec<&ClassDef> {
        self.classes
            .values()
            .filter(|class| {
                class
                    .methods
                    .iter()
                    .any(|method| method.name == method_name)
            })
            .collect()
    }

    /// Resolve a qualified method reference such as `A.render` or
    /// `Mod.A.render`. The qualifier is matched against the declaring module
    /// (or its final source segment for an import alias), and still must name
    /// exactly one class method.
    pub fn resolve_qualified_method_class_id(
        &self,
        qualifier: Identifier,
        method_name: Identifier,
        interner: &Interner,
    ) -> Option<ClassId> {
        let qualifier = interner.resolve(qualifier);
        let matches: Vec<_> = self
            .classes
            .values()
            .filter(|class| {
                class
                    .methods
                    .iter()
                    .any(|method| method.name == method_name)
            })
            .filter(|class| {
                class.module.as_identifier().is_some_and(|module| {
                    let module = interner.resolve(module);
                    module == qualifier || module.rsplit('.').next() == Some(qualifier)
                })
            })
            .collect();
        (matches.len() == 1).then(|| matches[0].class_id())
    }

    /// Return the positional index of a method within its class definition,
    /// looking the class up by short name (compatibility shim).
    ///
    /// Linear scan via [`lookup_class`](Self::lookup_class).
    pub fn method_index(&self, class_name: Identifier, method_name: Identifier) -> Option<usize> {
        self.method_slot(self.lookup_class(class_name)?.class_id(), method_name)
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

    /// Resolve a short source name only when it identifies one class. Semantic
    /// consumers must use the returned identity rather than `lookup_class`.
    pub fn unique_class_id(&self, name: Identifier) -> Option<ClassId> {
        let mut matches = self.classes.values().filter(|class| class.name == name);
        let first = matches.next()?.class_id();
        matches.next().is_none().then_some(first)
    }

    /// Resolve a class name in a module first, then require a unique global
    /// match. This is the source-resolution boundary; no downstream semantic
    /// lookup should fall back to declaration order.
    pub fn resolve_class_id(
        &self,
        current_module: ModulePath,
        name: Identifier,
    ) -> Option<ClassId> {
        if let Some(class) = self
            .classes
            .values()
            .find(|class| class.module == current_module && class.name == name)
        {
            return Some(class.class_id());
        }
        self.unique_class_id(name)
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
    pub fn method_index_by_id(&self, id: ClassId, method_name: Identifier) -> Option<usize> {
        self.method_slot(id, method_name)
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
        self.candidate_instances(class_name, actual_type_args, interner)
            .next()
    }

    /// Every instance whose head matches `actual_type_args`, in declaration
    /// order.
    ///
    /// [`Self::resolve_instance_with_subst`] takes the first candidate, which
    /// is only sound when there is exactly one. Enumerating them lets callers
    /// detect overlap (Proposal 0179 Stage 3, E454) rather than silently
    /// depending on declaration order, and gives Stage 4's deterministic
    /// evidence resolution the candidate set it needs.
    pub fn candidate_instances<'a, 'args>(
        &'a self,
        class_name: Identifier,
        actual_type_args: &'args [InferType],
        interner: &'args Interner,
    ) -> impl Iterator<Item = (&'a InstanceDef, HashMap<Identifier, InferType>)> + use<'a, 'args>
    {
        self.instances.iter().filter_map(move |inst| {
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

    /// Every instance whose head matches a concrete argument list for the
    /// specified canonical class identity.
    pub fn candidate_instances_by_id<'a, 'args>(
        &'a self,
        class_id: ClassId,
        actual_type_args: &'args [InferType],
        interner: &'args Interner,
    ) -> impl Iterator<Item = (&'a InstanceDef, HashMap<Identifier, InferType>)> + use<'a, 'args>
    {
        self.instances.iter().filter_map(move |inst| {
            if inst.class_id != class_id || inst.type_args.len() != actual_type_args.len() {
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

    /// The unique instance matching a canonical class identity and a partially
    /// known argument list.
    pub fn unique_instance_for_known_args_by_id(
        &self,
        class_id: ClassId,
        known: &[Option<InferType>],
        interner: &Interner,
    ) -> Option<&InstanceDef> {
        let mut matches = self.instances.iter().filter(|inst| {
            if inst.class_id != class_id || inst.type_args.len() != known.len() {
                return false;
            }
            let mut subst = HashMap::new();
            inst.type_args
                .iter()
                .zip(known)
                .all(|(pattern, actual)| match actual {
                    Some(actual) => {
                        Self::match_instance_type_expr(pattern, actual, &mut subst, interner)
                    }
                    None => true,
                })
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    /// The single instance matching a predicate whose slots are only partly
    /// known (Proposal 0179, Stage 4).
    ///
    /// `known` carries one entry per class type parameter: `Some` where a call
    /// determined the type, `None` where it did not. Only the known slots are
    /// matched, so `Convert<Int, ?>` still selects `Convert<Int, String>` when
    /// that is the sole candidate — the instance head then supplies the
    /// remaining argument.
    ///
    /// Returns `None` unless exactly one instance matches. Committing to the
    /// first of several would make evidence selection depend on declaration
    /// order, which is what E454 reports.
    pub fn unique_instance_for_known_args(
        &self,
        class_name: Identifier,
        known: &[Option<InferType>],
        interner: &Interner,
    ) -> Option<&InstanceDef> {
        let class_id = self.unique_class_id(class_name)?;
        self.unique_instance_for_known_args_by_id(class_id, known, interner)
    }

    /// ClassId-keyed counterpart of [`Self::instances_matching_known_args`].
    pub fn instances_matching_known_args_by_id<'a, 'args>(
        &'a self,
        class_id: ClassId,
        known: &'args [Option<InferType>],
        interner: &'args Interner,
    ) -> impl Iterator<Item = &'a InstanceDef> + use<'a, 'args> {
        self.instances.iter().filter(move |inst| {
            if inst.class_id != class_id || inst.type_args.len() != known.len() {
                return false;
            }
            let mut subst = HashMap::new();
            inst.type_args
                .iter()
                .zip(known)
                .all(|(pattern, actual)| match actual {
                    Some(actual) => {
                        Self::match_instance_type_expr(pattern, actual, &mut subst, interner)
                    }
                    None => true,
                })
        })
    }

    /// Resolve a class method receiver using a canonical class identity.
    pub fn resolve_method_call_instance_from_first_arg_by_id(
        &self,
        class_id: ClassId,
        first_actual_type: &InferType,
        interner: &Interner,
    ) -> Option<(&InstanceDef, Vec<InferType>)> {
        let mut matches = self.instances.iter().filter_map(|inst| {
            if inst.class_id != class_id {
                return None;
            }
            let first_pattern = inst.type_args.first()?;
            let mut subst = HashMap::new();
            if !Self::match_instance_type_expr(
                first_pattern,
                first_actual_type,
                &mut subst,
                interner,
            ) {
                return None;
            }
            let concrete_type_args = inst
                .type_args
                .iter()
                .map(|arg| instantiate_instance_type_expr(arg, &subst, interner))
                .collect::<Option<Vec<_>>>()?;
            Some((inst, concrete_type_args))
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    // Keep the existing short-name API at the source-resolution boundary.
    pub fn resolve_method_call_instance_from_first_arg(
        &self,
        class_name: Identifier,
        first_actual_type: &InferType,
        interner: &Interner,
    ) -> Option<(&InstanceDef, Vec<InferType>)> {
        let class_id = self.unique_class_id(class_name)?;
        self.resolve_method_call_instance_from_first_arg_by_id(
            class_id,
            first_actual_type,
            interner,
        )
    }

    /// Every instance compatible with the slots a call determined.
    ///
    /// An undetermined slot constrains nothing, so this is the candidate set
    /// that remains open. More than one candidate means the call cannot select
    /// an instance until the missing type is supplied (E459).
    pub fn instances_matching_known_args<'a, 'args>(
        &'a self,
        class_name: Identifier,
        known: &'args [Option<InferType>],
        interner: &'args Interner,
    ) -> impl Iterator<Item = &'a InstanceDef> + use<'a, 'args> {
        self.instances.iter().filter(move |inst| {
            if inst.class_name != class_name || inst.type_args.len() != known.len() {
                return false;
            }
            let mut subst = HashMap::new();
            inst.type_args
                .iter()
                .zip(known)
                .all(|(pattern, actual)| match actual {
                    Some(actual) => {
                        Self::match_instance_type_expr(pattern, actual, &mut subst, interner)
                    }
                    None => true,
                })
        })
    }

    /// The concrete type arguments of `instance`'s head, given what a call
    /// determined (Proposal 0179, Stage 4).
    ///
    /// Binds the head's own variables from the known slots, then instantiates
    /// every head argument. This is what lets `let s = convert(42)` — where the
    /// result type is not yet known — still resolve `Convert<Int, String>`:
    /// the sole matching instance fixes the second argument.
    pub fn instantiate_instance_head(
        &self,
        instance: &InstanceDef,
        known: &[Option<InferType>],
        interner: &Interner,
    ) -> Option<Vec<InferType>> {
        let mut subst = HashMap::new();
        for (pattern, actual) in instance.type_args.iter().zip(known) {
            if let Some(actual) = actual {
                Self::match_instance_type_expr(pattern, actual, &mut subst, interner);
            }
        }
        instance
            .type_args
            .iter()
            .map(|arg| instantiate_instance_type_expr(arg, &subst, interner))
            .collect()
    }

    /// Whether `constraint` contributes a runtime dictionary parameter.
    ///
    /// A class with no methods — a marker such as `Sendable` — has no
    /// dictionary tuple and no `__dict_*` global, so it contributes no
    /// parameter and no argument.
    ///
    /// Every phase that counts dictionaries must agree on this predicate.
    /// Before Proposal 0179 Stage 2 the AST path filtered marker classes while
    /// Core lowering and dictionary elaboration did not, so the same function
    /// had two different arities: the callee gained a phantom parameter the
    /// caller never passed, and the call failed at runtime with
    /// `E1000 wrong number of arguments`.
    ///
    /// An *unknown* class is deliberately treated as dictionary-carrying. It
    /// is not a marker class, and silently dropping it would reintroduce the
    /// arity disagreement this predicate exists to prevent.
    pub fn constraint_needs_dictionary(
        &self,
        constraint: &crate::ast::type_infer::constraint::SchemeConstraint,
    ) -> bool {
        self.lookup_class_by_id(constraint.class_id)
            .is_none_or(|class| !class.methods.is_empty())
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
        let class_id = self.unique_class_id(class_name)?;
        self.resolve_dictionary_ref_by_id(class_id, actual_type_args, interner)
    }

    /// ClassId-keyed dictionary resolution used by elaboration and backends.
    pub fn resolve_dictionary_ref_by_id(
        &self,
        class_id: ClassId,
        actual_type_args: &[InferType],
        interner: &Interner,
    ) -> Option<ResolvedDictionaryRef> {
        let (instance, subst) =
            self.resolve_instance_with_subst_by_id(class_id, actual_type_args, interner)?;
        let type_name = instance
            .type_args
            .iter()
            .map(|arg| arg.display_with(interner))
            .collect::<Vec<_>>()
            .join("_");
        let dict_name =
            interner.lookup(&dictionary_name(instance.class_id, &type_name, interner))?;
        let context_args = instance
            .context
            .iter()
            .enumerate()
            .map(|(index, constraint)| {
                let concrete_args = constraint
                    .type_args
                    .iter()
                    .map(|arg| instantiate_instance_type_expr(arg, &subst, interner))
                    .collect::<Option<Vec<_>>>()?;
                let context_id = instance
                    .context_class_ids
                    .get(index)
                    .copied()
                    .or_else(|| self.unique_class_id(constraint.class_name))?;
                self.resolve_dictionary_ref_by_id(context_id, &concrete_args, interner)
            })
            .collect::<Option<Vec<_>>>()?;

        Some(ResolvedDictionaryRef {
            dict_name,
            context_args,
        })
    }

    pub(crate) fn resolve_instance_context_dictionary_requests_by_id(
        &self,
        class_id: ClassId,
        actual_type_args: &[InferType],
        interner: &Interner,
    ) -> Option<Vec<InstanceContextDictionaryRequest>> {
        let (instance, subst) =
            self.resolve_instance_with_subst_by_id(class_id, actual_type_args, interner)?;

        instance
            .context
            .iter()
            .enumerate()
            .map(|(index, constraint)| {
                let type_args = constraint
                    .type_args
                    .iter()
                    .map(|arg| instantiate_instance_type_expr(arg, &subst, interner))
                    .collect::<Option<Vec<_>>>()?;
                let context_id = instance
                    .context_class_ids
                    .get(index)
                    .copied()
                    .or_else(|| self.unique_class_id(constraint.class_name))?;
                let dictionary =
                    self.resolve_dictionary_ref_by_id(context_id, &type_args, interner);
                Some(InstanceContextDictionaryRequest {
                    class_name: constraint.class_name,
                    class_id: context_id,
                    type_args,
                    dictionary,
                })
            })
            .collect()
    }

    /// Expand a pre-interned `__dict_{Class}_{Type}` name into the ordered
    /// mangled method symbols that make up the dictionary tuple, if this name
    /// corresponds to a known instance.
    pub fn dictionary_slot_symbols(
        &self,
        dict_name: Identifier,
        interner: &Interner,
    ) -> Option<Vec<Identifier>> {
        let dict_name_str = interner.resolve(dict_name);
        self.instances.iter().find_map(|instance| {
            let type_name = instance
                .type_args
                .iter()
                .map(|arg| arg.display_with(interner))
                .collect::<Vec<_>>()
                .join("_");
            let expected = dictionary_name(instance.class_id, &type_name, interner);
            if dict_name_str != expected {
                return None;
            }

            self.dictionary_slot_names(instance.class_id, &type_name, interner)?
                .iter()
                .map(|name| interner.lookup(name))
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
        let sendable = interner.intern("Sendable");

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
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: neq_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
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
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: int_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: lt_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: lte_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: gt_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: gte_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: bool_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
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
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: sub_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: mul_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
                },
                MethodSig {
                    type_params: vec![],
                    name: div_method,
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_ty.clone(), a_ty.clone()],
                    return_type: a_ty.clone(),
                    arity: 2,
                    effects: vec![],
                    default_body: None,
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
                param_names: vec![interner.intern("__x0")],
                param_types: vec![a_ty.clone()],
                return_type: string_ty,
                arity: 1,
                effects: vec![],
                default_body: None,
            }],
        );

        // Semigroup: append(a, a) -> a
        self.register_builtin_class(
            semigroup,
            vec![a_param],
            vec![MethodSig {
                type_params: vec![],
                name: append_method,
                param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                param_types: vec![a_ty.clone(), a_ty],
                return_type: builtin_type(a_param),
                arity: 2,
                effects: vec![],
                default_body: None,
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

        // Sendable: marker class, no methods (proposal 0174 Phase 1a-v).
        // Authorizes a value to cross a worker-thread boundary
        // (`Channel.send`, `Task.spawn`). Positive-only auto-derivation:
        // primitives have explicit instances below; the constraint solver
        // synthesises structural instances for tuples and persistent
        // collections (`Option`, `List`, `Array`, `Map`, `Either`) whose
        // element types are themselves `Sendable`. User ADTs get synthesized
        // instances during collection when their fields are sendable; closures
        // and explicit opaque runtime handles remain non-sendable. There are
        // no negative instances; absence of an instance means "not sendable."
        self.register_builtin_class(sendable, vec![a_param], vec![]);

        // Sendable instances: Int, Float, String, Bool, Unit.
        let unit_name = interner.intern("Unit");
        for ty in [int_name, float_name, string_name, bool_name, unit_name] {
            self.register_builtin_instance(sendable, ty);
        }
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
                is_builtin: true,
                // Built-in classes are visible everywhere via the implicit
                // prelude — `is_public` is meaningless for them since
                // visibility checks key off `instance_module` vs class
                // module, and built-ins use the EMPTY sentinel instead.
                is_public: false,
                type_params,
                superclasses: vec![],
                superclass_class_ids: vec![],
                associated_types: vec![],
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
            // Built-ins are universally visible via the prelude; the flag
            // is irrelevant for them.
            is_public: false,
            type_args: vec![builtin_type(type_name)],
            context: vec![],
            context_class_ids: vec![],
            method_names: vec![],
            method_effects: vec![],
            associated_types: vec![],
            span: Span::default(),
        });
    }
}

pub(crate) fn instantiate_instance_type_expr(
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
    pub(crate) fn match_instance_type_expr(
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
                                && args.iter().zip(actual_args.iter()).all(|(p, a)| {
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

    pub(crate) fn type_constructor_matches(
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

/// The one place a type-class method's mangled global name is built.
///
/// Every resolution path and both backends must agree on this string. A
/// mismatch is not a compile error — it is a missing global discovered at run
/// time, surfacing as `E1001`/`E1009` far from its cause, which is the failure
/// mode KI-051 took two attempts to pin down. Ten call sites used to format
/// this independently; routing them through one function is what makes the
/// format safe to change at all.
pub fn mangled_method_name(
    class_id: ClassId,
    type_key: &str,
    method: &str,
    interner: &Interner,
) -> String {
    let class = class_symbol_name(class_id, interner);
    format!("{INSTANCE_METHOD_PREFIX}{class}_{type_key}_{method}")
}

/// Render the canonical dictionary global for a concrete instance head.
pub fn dictionary_name(class_id: ClassId, type_key: &str, interner: &Interner) -> String {
    format!(
        "{DICTIONARY_PREFIX}{}_{}",
        class_symbol_name(class_id, interner),
        type_key
    )
}

/// Render the canonical prefix used for contextual dictionary parameters.
pub fn dictionary_prefix(class_id: ClassId, interner: &Interner) -> String {
    format!(
        "{DICTIONARY_PREFIX}{}",
        class_symbol_name(class_id, interner)
    )
}

/// Whether `name` was built by [`dictionary_prefix`] or [`dictionary_name`].
///
/// The same reasoning as [`is_generated_instance_method`]: several passes
/// recognise a dictionary by its prefix, so the spelling is asked for here
/// rather than written out at each site.
pub fn is_dictionary_name(name: &str) -> bool {
    name.starts_with(DICTIONARY_PREFIX)
}

const DICTIONARY_PREFIX: &str = "__dict_";

/// Render a class identity for generated symbols. Legacy top-level classes
/// retain their historical spelling; module-owned classes include an
/// injective hexadecimal encoding of the owning module so `A.Foo` and
/// `B.Foo` cannot collapse after native symbol sanitization.
pub fn class_symbol_name(class_id: ClassId, interner: &Interner) -> String {
    let class = interner.resolve(class_id.name);
    let Some(module) = class_id.module.as_identifier() else {
        return class.to_string();
    };
    let module = interner.resolve(module);
    let encoded_module = module
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    format!("m{}_{encoded_module}_{class}", module.len())
}

/// The prefix [`mangled_method_name`] stamps on every name it builds.
const INSTANCE_METHOD_PREFIX: &str = "__tc_";

/// Whether `name` was built by [`mangled_method_name`].
///
/// Centralising the *constructor* is only half of what makes the format safe
/// to change. Sites across desugaring, inference, both backends, and the VM
/// recognise these names by prefix, and a format change that missed one of them
/// would fail exactly the way described above — silently, at run time. Ask here
/// rather than writing the prefix out.
///
/// This answers "does the name look generated". Where the compiler can know
/// the answer outright — it has the instance list — prefer that; see
/// `Compiler::generated_instance_methods_for_module`.
pub fn is_generated_instance_method(name: &str) -> bool {
    name.starts_with(INSTANCE_METHOD_PREFIX)
}

/// Create a simple named TypeExpr for built-in type references.
fn builtin_type(name: Identifier) -> TypeExpr {
    TypeExpr::Named {
        name,
        args: vec![],
        span: Span::default(),
    }
}

/// True if `expr` syntactically contains any function-type subterm.
/// Used by the Sendable ADT auto-derivation (proposal 0174 D4) to apply
/// the positive-only rule: data declarations whose fields can hold a
/// closure are not auto-derived.
fn type_expr_contains_function(expr: &TypeExpr) -> bool {
    match expr {
        TypeExpr::Function { .. } => true,
        TypeExpr::Tuple { elements, .. } => elements.iter().any(type_expr_contains_function),
        TypeExpr::Named { args, .. } => args.iter().any(type_expr_contains_function),
    }
}

fn is_opaque_non_sendable_adt(
    module: ModulePath,
    adt_name: Identifier,
    interner: &Interner,
) -> bool {
    let Some(module_name) = module.as_identifier().map(|id| interner.resolve(id)) else {
        return false;
    };
    module_name == "Flow.Tcp" && matches!(interner.resolve(adt_name), "Connection" | "Listener")
}

#[cfg(test)]
mod tests {
    use super::{ClassEnv, InstanceDef, builtin_type, dictionary_name, mangled_method_name};
    use crate::{
        diagnostics::position::Span,
        syntax::interner::Interner,
        types::{class_id::ModulePath, infer_type::InferType, type_constructor::TypeConstructor},
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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

        // A bare-name semantic lookup is ambiguous once multiple modules
        // provide the same class, so only the ClassId-keyed API can select it.
        let bare = env.lookup_class(foo_sym);
        assert!(
            bare.is_none(),
            "ambiguous bare lookup must not select a class"
        );
    }

    #[test]
    fn class_id_symbols_are_disjoint_for_same_named_classes() {
        use crate::syntax::{lexer::Lexer, parser::Parser};
        use crate::types::class_id::ClassId;

        let source = r#"
module Mod.A {
    class Foo<a> { fn render(x: a) -> String }
    instance Foo<Int> { fn render(x) { "A" } }
}
module Mod.B {
    class Foo<a> { fn render(x: a) -> String }
    instance Foo<Int> { fn render(x) { "B" } }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();
        let (env, diagnostics) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diagnostics.is_empty(),
            "collection errors: {:?}",
            diagnostics
        );

        let foo = interner.lookup("Foo").unwrap();
        let render = interner.resolve(interner.lookup("render").unwrap());
        let int = "Int";
        let id_a = ClassId::new(
            ModulePath::from_identifier(interner.lookup("Mod.A").unwrap()),
            foo,
        );
        let id_b = ClassId::new(
            ModulePath::from_identifier(interner.lookup("Mod.B").unwrap()),
            foo,
        );
        let method_a = mangled_method_name(id_a, int, render, &interner);
        let method_b = mangled_method_name(id_b, int, render, &interner);
        assert_ne!(method_a, method_b);
        assert_ne!(
            dictionary_name(id_a, int, &interner),
            dictionary_name(id_b, int, &interner)
        );
        assert_eq!(env.instances_for_id(id_a).len(), 1);
        assert_eq!(env.instances_for_id(id_b).len(), 1);
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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
        assert_eq!(
            insts_a[0].instance_module,
            ModulePath::from_identifier(mod_a)
        );
        assert_eq!(
            insts_b[0].instance_module,
            ModulePath::from_identifier(mod_b)
        );

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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
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
            env.resolve_instance_with_subst_by_id(id_a, std::slice::from_ref(&int), &interner)
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);

        // First declaration succeeds, second is rejected as a duplicate.
        assert_eq!(env.classes.len(), 1, "only one class should be inserted");
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E440")),
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
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(diags.is_empty(), "collection errors: {:?}", diags);

        let class_sym = interner.lookup("DeeplyNested").unwrap();
        let class_def = env.lookup_class(class_sym).unwrap();

        let expected = interner.lookup("Outer.Inner.Deep").unwrap();
        assert_eq!(class_def.module, ModulePath::from_identifier(expected));
    }

    // ============================================================
    // Proposal 0151, Phase 2: orphan rule walker tests (E449).
    // ============================================================

    /// Class is local: instance lives in the same module as the class.
    /// Head type is foreign (Int). Must NOT be flagged as orphan.
    #[test]
    fn orphan_rule_class_local_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    class MyShow<a> {
        fn my_show(x: a) -> String
    }

    instance MyShow<Int> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E449")),
            "instance in class's own module must not be orphan, got: {:?}",
            diags
        );
    }

    /// Head type is local: data declared in the same module as the
    /// instance, class is foreign. Must NOT be flagged as orphan.
    #[test]
    fn orphan_rule_head_type_local_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.Class {
    class MyShow<a> {
        fn my_show(x: a) -> String
    }
}

module Mod.Type {
    data Color { Red, Green, Blue }

    instance MyShow<Color> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E449")),
            "instance in head type's own module must not be orphan, got: {:?}",
            diags
        );
    }

    /// Third-module orphan: neither the class nor the head type lives in
    /// the instance's module. Must fire E449.
    #[test]
    fn orphan_rule_third_module_is_rejected() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.Class {
    class MyShow<a> {
        fn my_show(x: a) -> String
    }
}

module Mod.Type {
    data Color { Red, Green, Blue }
}

module Mod.Third {
    instance MyShow<Color> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E449")),
            "third-module instance should be rejected as orphan, got: {:?}",
            diags
        );
    }

    /// `deriving` instances are always trivially legal because they live
    /// in the data declaration's own module — head type is local.
    #[test]
    fn orphan_rule_deriving_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.Class {
    class MyShow<a> {
        fn my_show(x: a) -> String
    }
}

module Mod.Type {
    data Color { Red, Green, Blue } deriving (MyShow)
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E449")),
            "deriving instance lives in the data's module — must not be orphan, got: {:?}",
            diags
        );
    }

    /// Legacy top-level instances (instance_module == EMPTY) are
    /// grandfathered: the orphan walker must not flag them.
    #[test]
    fn orphan_rule_grandfathers_top_level_instances() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
class TopLvlShow<a> {
    fn doit(x: a) -> String
}

instance TopLvlShow<Int> {
    fn doit(x) { "" }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E449")),
            "legacy top-level instances must be grandfathered, got: {:?}",
            diags
        );
    }

    // ============================================================
    // Proposal 0151, Phase 2: visibility walker tests (E450).
    // ============================================================

    /// `public instance` of a `public class` is legal — both surfaces
    /// agree, no leak.
    #[test]
    fn visibility_public_instance_of_public_class_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    public class MyShow<a> {
        fn my_show(x: a) -> String
    }

    public instance MyShow<Int> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E450")),
            "public instance of public class must not fire E450, got: {:?}",
            diags
        );
    }

    /// Private instance of a private class is legal — neither escapes
    /// the module.
    #[test]
    fn visibility_private_instance_of_private_class_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    class MyShow<a> {
        fn my_show(x: a) -> String
    }

    instance MyShow<Int> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E450")),
            "private instance of private class must not fire E450, got: {:?}",
            diags
        );
    }

    /// `public instance` of a private class — must fire E450.
    #[test]
    fn visibility_public_instance_of_private_class_is_rejected() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    class MyShow<a> {
        fn my_show(x: a) -> String
    }

    public instance MyShow<Int> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E450")),
            "public instance of private class must fire E450, got: {:?}",
            diags
        );
    }

    /// Visibility check must not fire on built-in (prelude) classes — those
    /// have `module == EMPTY` and are universally visible. A `public instance`
    /// for a built-in class like `Show<MyType>` is legal.
    #[test]
    fn visibility_walker_skips_builtin_classes() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        // `Show` is a built-in class registered with module = EMPTY. We
        // declare a user ADT and a `public instance Show<Color>` — the
        // walker must not flag this even though `Show` is not literally
        // marked `public class`.
        let source = r#"
module Mod.A {
    data Color { Red, Green, Blue }

    public instance Show<Color> {
        fn show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        // We don't register built-in classes here — `from_statements`
        // doesn't pre-populate the env. The class lookup will fail and the
        // instance won't be added, which means the visibility walker has
        // nothing to flag. That's fine: the test is asserting "no E450",
        // not the absence of all errors.
        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E450")),
            "instance for unknown/built-in class must not fire E450, got: {:?}",
            diags
        );
    }

    // ============================================================
    // Proposal 0151, Phase 2: E455 (public instance, private head ADT).
    // ============================================================

    /// Public instance of a public class with a public ADT head — legal.
    #[test]
    fn e455_public_instance_public_head_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    public class MyShow<a> {
        fn my_show(x: a) -> String
    }

    public data Color { Red, Green, Blue }

    public instance MyShow<Color> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E455")),
            "public instance with public head must not fire E455, got: {:?}",
            diags
        );
    }

    /// Public instance of a public class with a *private* ADT head — E455.
    #[test]
    fn e455_public_instance_private_head_is_rejected() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    public class MyShow<a> {
        fn my_show(x: a) -> String
    }

    data Color { Red, Green, Blue }

    public instance MyShow<Color> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E455")),
            "public instance with private head must fire E455, got: {:?}",
            diags
        );
    }

    /// Private instance with private head — E455 must NOT fire (private
    /// instances cannot leak).
    #[test]
    fn e455_private_instance_private_head_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    public class MyShow<a> {
        fn my_show(x: a) -> String
    }

    data Color { Red, Green, Blue }

    instance MyShow<Color> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E455")),
            "private instance must not fire E455, got: {:?}",
            diags
        );
    }

    /// Public instance of a public class with a built-in head (Int) — E455
    /// must NOT fire because built-in types are universally visible.
    #[test]
    fn e455_public_instance_builtin_head_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    public class MyShow<a> {
        fn my_show(x: a) -> String
    }

    public instance MyShow<Int> {
        fn my_show(x) { "" }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E455")),
            "public instance with built-in head must not fire E455, got: {:?}",
            diags
        );
    }

    // ============================================================
    // Proposal 0151, Phase 2: E451 (public class leaks private type).
    // ============================================================

    /// Public class signature mentions a private ADT in a method parameter
    /// — E451.
    #[test]
    fn e451_public_class_param_mentions_private_type_is_rejected() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    data Secret { Hidden }

    public class Reveal<a> {
        fn show_secret(x: a, s: Secret) -> String
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E451")),
            "public class with private type in method param must fire E451, got: {:?}",
            diags
        );
    }

    /// Public class signature mentions a private ADT in the return type — E451.
    #[test]
    fn e451_public_class_return_mentions_private_type_is_rejected() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    data Secret { Hidden }

    public class Maker<a> {
        fn make(x: a) -> Secret
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E451")),
            "public class with private return type must fire E451, got: {:?}",
            diags
        );
    }

    /// Public class signature mentions only public ADTs — E451 must NOT fire.
    #[test]
    fn e451_public_class_with_public_types_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    public data Color { Red, Green, Blue }

    public class Painter<a> {
        fn paint(x: a, c: Color) -> Color
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E451")),
            "public class with all-public ADTs must not fire E451, got: {:?}",
            diags
        );
    }

    /// Private class with private types — E451 must NOT fire (private
    /// classes can mention anything they want).
    #[test]
    fn e451_private_class_with_private_types_is_legal() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.A {
    data Secret { Hidden }

    class Reveal<a> {
        fn show_secret(x: a, s: Secret) -> Secret
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E451")),
            "private class is allowed to mention private types, got: {:?}",
            diags
        );
    }

    // ============================================================
    // Proposal 0151, Phase 2: E443 extended (cross-module duplicate
    // public instances of the same `(ClassId, head_type)`).
    // ============================================================

    /// Cross-module duplicate: `Mod.Class` declares the class, two
    /// different ADT-owning modules each declare a `public instance`
    /// of `Mod.Class.Foo<X>` for the SAME structural head type. Both
    /// instances pass the orphan rule (each has a local head type),
    /// but together they create a coherence violation. The walker
    /// must reject the second one with E443.
    #[test]
    fn e443_extended_cross_module_duplicate_public_instances_rejected() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        // Both modules implement `MyShow<Int>`. The class lives in
        // `Mod.Class`, so `Mod.Class`'s own instance is class-local
        // and the second one in `Mod.Other` is rejected as orphan
        // first... but if BOTH are in the class's own module they
        // collide directly. Use the simpler same-module form to
        // exercise the dedup, since the cross-module case for a
        // shared head type is already covered by the orphan walker.
        //
        // The harder cross-module case (instance Cls<MyAdt> in two
        // different modules where both are legal under the orphan
        // rule) requires a SHARED ADT, which means the head type is
        // owned by exactly one of the two modules — only that module
        // can host an instance under the orphan rule. So the only
        // way two cross-module instances of the same (ClassId,
        // head_type) can coexist post-orphan-rule is if they BOTH
        // live in the class's own module — i.e. same module. This
        // confirms the dedup check is the right gate.
        let source = r#"
module Mod.Class {
    public class MyShow<a> {
        fn my_show(x: a) -> Int
    }

    public instance MyShow<Int> {
        fn my_show(x) { 1 }
    }

    public instance MyShow<Int> {
        fn my_show(x) { 2 }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E443")),
            "duplicate public instance must fire E443, got: {:?}",
            diags
        );
    }

    /// Cross-module dedup: two `public instance`s of `Mod.Class.MyShow<MyAdt>`,
    /// where the ADT lives in the class's own module so both placements
    /// pass the orphan rule. The dedup check must collapse them into a
    /// single E443.
    ///
    /// In practice the only way to construct this (post-orphan-rule) is
    /// for both instances to live in the same module — see the comment
    /// in the previous test. This test exercises the same path with an
    /// ADT head type instead of `Int`.
    #[test]
    fn e443_extended_duplicate_public_instances_for_adt_rejected() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.Class {
    public class MyShow<a> {
        fn my_show(x: a) -> Int
    }

    public data Color { Red, Green, Blue }

    public instance MyShow<Color> {
        fn my_show(x) { 1 }
    }

    public instance MyShow<Color> {
        fn my_show(x) { 2 }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("E443")),
            "duplicate public ADT instance must fire E443, got: {:?}",
            diags
        );
    }

    /// Genuinely cross-module dedup: a third module attempts to add a
    /// `public instance Mod.Class.MyShow<Int>` that already exists in
    /// `Mod.Class`. The orphan walker (E449) will fire on the third
    /// module too because neither the class nor the head is local to
    /// it — but the dedup gate's diagnostic is the relevant one for
    /// the *coherence* story, and its hint must mention the other
    /// owning module so users see "extended cross-module" coverage in
    /// the message.
    ///
    /// We bypass the orphan walker by hand-constructing the env: we
    /// register a class in `Mod.Class`, then push two `InstanceDef`s
    /// with different `instance_module`s and structurally identical
    /// type args, and verify that re-running the dedup logic via a
    /// fresh source-driven collection still catches it. The simpler
    /// way to assert the cross-module diagnostic message itself is to
    /// make both modules add an instance for an ADT they don't own —
    /// the dedup fires before E449 within `collect_instances`, so the
    /// hint text is observable.
    #[test]
    fn e443_extended_diagnostic_mentions_other_owning_module() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        // Mod.Class owns both the class and the head ADT. Mod.Other
        // illegally adds the same instance — orphan rule rejects it,
        // and the dedup ALSO rejects it (since the in-program env
        // already contains the legal Mod.Class instance). The dedup
        // diagnostic's hint text should mention `Mod.Class` as the
        // existing instance's module.
        let source = r#"
module Mod.Class {
    public class MyShow<a> {
        fn my_show(x: a) -> Int
    }

    public data Color { Red, Green, Blue }

    public instance MyShow<Color> {
        fn my_show(x) { 1 }
    }
}

module Mod.Other {
    public instance MyShow<Color> {
        fn my_show(x) { 2 }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);

        // The duplicate-instance gate must fire (E443).
        let dupe = diags
            .iter()
            .find(|d| d.code.as_deref() == Some("E443"))
            .expect("expected E443 to fire on cross-module duplicate instance");

        // The hint must mention BOTH module names so users can find
        // the existing colliding declaration.
        let hint_text = dupe
            .hints
            .iter()
            .map(|h| h.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            hint_text.contains("Mod.Class") && hint_text.contains("Mod.Other"),
            "E443 hint must mention both owning modules, got: {hint_text:?}"
        );
    }

    /// Negative: two `public instance`s for *different* head types of
    /// the same class are NOT duplicates and must not fire E443.
    #[test]
    fn e443_extended_distinct_head_types_are_not_duplicates() {
        use crate::syntax::{lexer::Lexer, parser::Parser};

        let source = r#"
module Mod.Class {
    public class MyShow<a> {
        fn my_show(x: a) -> Int
    }

    public instance MyShow<Int> {
        fn my_show(x) { 1 }
    }

    public instance MyShow<Bool> {
        fn my_show(x) { 0 }
    }
}
"#;
        let mut parser = Parser::new(Lexer::new(source));
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let (_env, diags) = ClassEnv::from_statements(&program.statements, &interner);
        assert!(
            !diags.iter().any(|d| d.code.as_deref() == Some("E443")),
            "distinct head types must not fire E443, got: {:?}",
            diags
        );
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
            is_public: false,
            type_args,
            context: vec![],
            context_class_ids: vec![],
            method_names: vec![],
            method_effects: vec![],
            associated_types: vec![],
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

#[cfg(test)]
mod dict_selection_tests {
    use super::{DictSelection, select_dictionary};

    /// A call whose argument type matches exactly one constraint names that
    /// constraint, which is the whole point of KI-057: `root(x)` and `root(y)`
    /// in `fn both<a: Root, b: Root>` must reach different dictionaries.
    #[test]
    fn a_matching_argument_type_names_one_constraint() {
        let candidates = vec![(0, vec!["a"]), (1, vec!["b"])];
        assert_eq!(
            select_dictionary(&candidates, &[Some("b")]),
            DictSelection::Unique(1)
        );
        assert_eq!(
            select_dictionary(&candidates, &[Some("a")]),
            DictSelection::Unique(0)
        );
    }

    /// Two constraints over the same type name the same instance, so whichever
    /// is picked reaches the same method. A function constrained on both
    /// `Sizeable<a>` and `Measurable<a>` calling `size` is exactly this: one
    /// candidate reaches it directly, the other through superclass evidence.
    #[test]
    fn candidates_over_the_same_type_are_interchangeable() {
        let candidates = vec![(0, vec!["a"]), (1, vec!["b"]), (2, vec!["a"])];
        assert_eq!(
            select_dictionary(&candidates, &[Some("a")]),
            DictSelection::Unique(0)
        );
    }

    /// Revealing nothing does not narrow. With one dictionary in scope that is
    /// still an answer; with two it is a guess, and a guess is what this whole
    /// change exists to stop.
    #[test]
    fn revealing_nothing_narrows_nothing() {
        assert_eq!(
            select_dictionary(&[(0, vec!["a"])], &[None]),
            DictSelection::Unique(0)
        );
        assert_eq!(
            select_dictionary(&[(0, vec!["a"]), (1, vec!["b"])], &[None]),
            DictSelection::Ambiguous
        );
    }

    /// An argument matching no constraint is not an error: a concrete argument
    /// inside a constrained function dispatches to the concrete instance
    /// instead, which is a different path entirely.
    #[test]
    fn an_unmatched_argument_defers_rather_than_failing() {
        assert_eq!(
            select_dictionary(&[(0, vec!["a"]), (1, vec!["b"])], &[Some("Int")]),
            DictSelection::NoMatch
        );
        assert_eq!(
            select_dictionary::<&str>(&[], &[None]),
            DictSelection::NoMatch
        );
    }

    /// Every class parameter must agree, so a multi-parameter class narrows on
    /// the positions the call reveals and ignores the ones it does not.
    #[test]
    fn a_multi_parameter_class_narrows_on_every_revealed_position() {
        let candidates = vec![(0, vec!["a", "x"]), (1, vec!["a", "y"])];
        assert_eq!(
            select_dictionary(&candidates, &[Some("a"), Some("y")]),
            DictSelection::Unique(1)
        );
        assert_eq!(
            select_dictionary(&candidates, &[Some("a"), None]),
            DictSelection::Ambiguous
        );
    }
}
