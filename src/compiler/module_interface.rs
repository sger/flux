//! Module interface files (`.flxi`) for separate compilation.
//!
//! The interface stores exported HM schemes and Aether borrow signatures for a
//! compiled module. Consumers can later preload this metadata without
//! recompiling the dependency from source.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    aether::borrow_infer::BorrowSignature,
    bytecode::bytecode_cache::hash_bytes,
    core::CoreProgram,
    runtime::function_contract::FunctionContract,
    shared::{cache_paths, hex},
    syntax::{Identifier, interner::Interner, symbol::Symbol},
    types::{
        class_env::ClassEnv,
        module_interface::{
            DependencyFingerprint, DependencyMissReason, MODULE_INTERFACE_FORMAT_VERSION,
            ModuleInterface, PublicClassEntry, PublicClassMethodEntry, PublicCtorTypeEntry,
            PublicInstanceEntry, PublicInstanceMethodEntry, PublicTypeAliasEntry,
        },
        scheme::Scheme,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceLoadError {
    NotFound,
    InvalidJson,
    CompilerVersionMismatch,
    FormatVersionMismatch,
    SourceHashMismatch,
    SemanticConfigMismatch,
    DependencyFingerprintMismatch {
        module_name: String,
        source_path: String,
        reason: DependencyMissReason,
    },
}

impl InterfaceLoadError {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "not found".to_string(),
            Self::InvalidJson => "invalid json".to_string(),
            Self::CompilerVersionMismatch => "compiler version mismatch".to_string(),
            Self::FormatVersionMismatch => "format version mismatch".to_string(),
            Self::SourceHashMismatch => "source hash mismatch".to_string(),
            Self::SemanticConfigMismatch => "semantic config mismatch".to_string(),
            Self::DependencyFingerprintMismatch {
                module_name,
                source_path,
                reason,
            } => {
                format!(
                    "dependency mismatch ({module_name} @ {source_path}): {}",
                    reason.label()
                )
            }
        }
    }
}

#[derive(Serialize)]
struct CanonicalExport<'a> {
    member: &'a str,
    scheme: Option<&'a Scheme>,
    borrow_signature: Option<&'a BorrowSignature>,
    runtime_contract: Option<&'a FunctionContract>,
    is_value: bool,
}

/// Build a module interface from post-Aether Core plus cached HM schemes.
///
/// Proposal 0151, Phase 2: when `class_env` is `Some`, the interface
/// also records `public class` and `public instance` entries owned by
/// `module_sym`. The recorded entries flow into the interface
/// fingerprint, so adding/removing/modifying a `public class` or
/// `public instance` invalidates downstream `.fxc` cache hits.
#[allow(clippy::too_many_arguments)]
pub fn build_interface(
    module_name: &str,
    module_sym: Identifier,
    source_hash: &[u8; 32],
    semantic_config_hash: &[u8; 32],
    program: &CoreProgram,
    schemes: &HashMap<(Identifier, Identifier), Scheme>,
    runtime_contracts: &HashMap<(Identifier, Identifier), FunctionContract>,
    visibility: &HashMap<(Identifier, Identifier), bool>,
    class_env: Option<&ClassEnv>,
    dependency_fingerprints: Vec<DependencyFingerprint>,
    interner: &Interner,
    ast_program: Option<&crate::syntax::program::Program>,
) -> ModuleInterface {
    let mut interface = ModuleInterface::new(
        module_name,
        hex::encode(source_hash),
        hex::encode(semantic_config_hash),
    );
    interface.dependency_fingerprints = dependency_fingerprints;

    // Proposal 0151, Phase 2: collect `public class` and `public instance`
    // entries owned by this module. Done before the symbol-table sweep so
    // any new symbols introduced by class/instance metadata also land in
    // the portable symbol table.
    if let Some(env) = class_env {
        interface.public_classes = collect_public_class_entries(env, module_sym, interner);
        interface.public_instances = collect_public_instance_entries(env, module_sym, interner);
    }

    for def in &program.defs {
        if def.is_anonymous() || visibility.get(&(module_sym, def.name)) != Some(&true) {
            continue;
        }

        let name = interner.resolve(def.name).to_string();
        if let Some(scheme) = schemes.get(&(module_sym, def.name)) {
            interface.schemes.insert(name.clone(), scheme.clone());
        }
        if let Some(signature) = &def.borrow_signature {
            interface
                .borrow_signatures
                .insert(name.clone(), signature.clone());
        }
        if let Some(contract) = runtime_contracts.get(&(module_sym, def.name)) {
            interface
                .runtime_contracts
                .insert(name.clone(), contract.clone());
        }
        interface
            .member_is_value
            .insert(name, !matches!(def.expr, crate::core::CoreExpr::Lam { .. }));
    }

    // Build portable symbol table: collect all Symbol IDs from schemes and
    // resolve them to strings via the interner. This allows consumers loading
    // the interface in a different session to re-intern and remap correctly.
    let mut symbols = HashSet::<Symbol>::new();
    for scheme in interface.schemes.values() {
        scheme.collect_symbols(&mut symbols);
    }
    for runtime_contract in interface.runtime_contracts.values() {
        runtime_contract.collect_symbols(&mut symbols);
    }
    collect_symbols_from_public_classes(&interface.public_classes, &mut symbols);
    collect_symbols_from_public_instances(&interface.public_instances, &mut symbols);
    for &sym in &symbols {
        interface
            .symbol_table
            .insert(sym.as_u32(), interner.resolve(sym).to_string());
    }

    // record declared field order for this module's public
    // record-style constructors so importing modules can desugar named-field
    // syntax. Must run before the fingerprint so a changed field order (which
    // changes the positional lowering) invalidates downstream caches.
    if let Some(program) = ast_program {
        interface.ctor_field_names = collect_public_ctor_field_names(program, interner);
        // Before the fingerprint: a changed field type changes how
        // importers infer the application.
        interface.public_ctor_types = collect_public_ctor_types(program, interner);
        interface.public_type_aliases = collect_public_type_aliases(program, interner);
        // Field types come from the raw AST, so they still name any alias.
        // Schemes are built post-expansion; these must match or an importer
        // sees `Bytes` where inference produced `String`.
        expand_aliases_in_ctor_types(&mut interface.public_ctor_types, program);
        let mut ctor_symbols: HashSet<Symbol> = HashSet::new();
        for entry in interface.public_ctor_types.values() {
            ctor_symbols.insert(entry.adt_name);
            ctor_symbols.extend(entry.type_params.iter().copied());
            if let Some(names) = entry.field_names.as_ref() {
                ctor_symbols.extend(names.iter().copied());
            }
            for field in &entry.fields {
                collect_symbols_from_type_expr(field, &mut ctor_symbols);
            }
        }
        for entry in interface.public_type_aliases.values() {
            ctor_symbols.extend(entry.params.iter().copied());
            collect_symbols_from_type_expr(&entry.body, &mut ctor_symbols);
        }
        for sym in ctor_symbols {
            interface
                .symbol_table
                .insert(sym.as_u32(), interner.resolve(sym).to_string());
        }
    }

    interface.interface_fingerprint = compute_interface_fingerprint(&interface);
    interface
}

/// collect `constructor -> [field names]` for every `public
/// data` declaration in `program`, walking both top-level and module-nested
/// statements.
///
/// Only public declarations are recorded: a private constructor is not
/// nameable from another module, so its field order is not part of the
/// interface. Field order is preserved exactly — it is the positional order
/// that named-field desugaring emits.
/// Collect `constructor -> type metadata` for every `public data` declaration,
/// walking top-level and module-nested statements.
///
/// Private constructors are skipped: they are not nameable from another module.
fn collect_public_ctor_types(
    program: &crate::syntax::program::Program,
    interner: &Interner,
) -> BTreeMap<String, PublicCtorTypeEntry> {
    fn walk(
        statements: &[crate::syntax::statement::Statement],
        interner: &Interner,
        out: &mut BTreeMap<String, PublicCtorTypeEntry>,
    ) {
        use crate::syntax::statement::Statement;
        for statement in statements {
            match statement {
                Statement::Data {
                    name,
                    is_public,
                    type_params,
                    variants,
                    ..
                } => {
                    if !is_public {
                        continue;
                    }
                    for variant in variants {
                        out.insert(
                            interner.resolve(variant.name).to_string(),
                            PublicCtorTypeEntry {
                                adt_name: *name,
                                type_params: type_params.clone(),
                                fields: variant.fields.clone(),
                                field_names: variant.field_names.clone(),
                            },
                        );
                    }
                }
                Statement::Module { body, .. } => walk(&body.statements, interner, out),
                _ => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&program.statements, interner, &mut out);
    out
}

/// Collect `alias -> (params, body)` for every `public alias` declaration,
/// walking top-level and module-nested statements.
/// Expand transparent aliases in exported constructor field types.
fn expand_aliases_in_ctor_types(
    ctor_types: &mut BTreeMap<String, PublicCtorTypeEntry>,
    program: &crate::syntax::program::Program,
) {
    use crate::syntax::statement::Statement;

    fn collect(
        statements: &[Statement],
        out: &mut std::collections::HashMap<
            crate::syntax::Identifier,
            crate::syntax::statement::TypeAliasDecl,
        >,
    ) {
        for statement in statements {
            match statement {
                Statement::TypeAlias(alias) => {
                    out.insert(alias.name, alias.clone());
                }
                Statement::Module { body, .. } => collect(&body.statements, out),
                _ => {}
            }
        }
    }

    let mut aliases = std::collections::HashMap::new();
    collect(&program.statements, &mut aliases);
    if aliases.is_empty() {
        return;
    }
    // Diagnostics are discarded: the declaring module already reported any
    // bad alias when it was compiled.
    let mut discarded = Vec::new();
    for entry in ctor_types.values_mut() {
        for field in &mut entry.fields {
            crate::ast::expand_type_aliases::expand_type(field, &aliases, "", &mut discarded);
        }
    }
}

fn collect_public_type_aliases(
    program: &crate::syntax::program::Program,
    interner: &Interner,
) -> BTreeMap<String, PublicTypeAliasEntry> {
    fn walk(
        statements: &[crate::syntax::statement::Statement],
        interner: &Interner,
        out: &mut BTreeMap<String, PublicTypeAliasEntry>,
    ) {
        use crate::syntax::statement::Statement;
        for statement in statements {
            match statement {
                Statement::TypeAlias(alias) => {
                    if !alias.is_public {
                        continue;
                    }
                    out.insert(
                        interner.resolve(alias.name).to_string(),
                        PublicTypeAliasEntry {
                            params: alias.params.clone(),
                            body: alias.body.clone(),
                        },
                    );
                }
                Statement::Module { body, .. } => walk(&body.statements, interner, out),
                _ => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&program.statements, interner, &mut out);
    out
}

fn collect_public_ctor_field_names(
    program: &crate::syntax::program::Program,
    interner: &Interner,
) -> BTreeMap<String, Vec<String>> {
    fn walk(
        statements: &[crate::syntax::statement::Statement],
        interner: &Interner,
        out: &mut BTreeMap<String, Vec<String>>,
    ) {
        use crate::syntax::statement::Statement;
        for statement in statements {
            match statement {
                Statement::Data {
                    is_public,
                    variants,
                    ..
                } => {
                    if !is_public {
                        continue;
                    }
                    for variant in variants {
                        if let Some(names) = variant.field_names.as_ref() {
                            out.insert(
                                interner.resolve(variant.name).to_string(),
                                names
                                    .iter()
                                    .map(|n| interner.resolve(*n).to_string())
                                    .collect(),
                            );
                        }
                    }
                }
                Statement::Module { body, .. } => walk(&body.statements, interner, out),
                _ => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&program.statements, interner, &mut out);
    out
}

/// Proposal 0151, Phase 2: extract every `public class` declared in
/// `module_sym` from the live `ClassEnv` and render it as a serializable
/// `PublicClassEntry`. Sorted by `(class_module, name)` for deterministic
/// fingerprinting.
fn collect_public_class_entries(
    env: &ClassEnv,
    module_sym: Identifier,
    interner: &Interner,
) -> Vec<PublicClassEntry> {
    let mut entries: Vec<PublicClassEntry> = env
        .classes
        .values()
        .filter(|def| def.is_public)
        .filter(|def| def.module.as_identifier() == Some(module_sym))
        .map(|def| {
            let class_module = def
                .module
                .as_identifier()
                .map(|id| interner.resolve(id).to_string())
                .unwrap_or_default();
            let method_names = def
                .methods
                .iter()
                .map(|m| interner.resolve(m.name).to_string())
                .collect();
            PublicClassEntry {
                class_module,
                name: interner.resolve(def.name).to_string(),
                type_param_arity: def.type_params.len(),
                parameter_kinds: def
                    .type_params
                    .iter()
                    .map(|param| class_parameter_kind(def, *param))
                    .collect(),
                type_params: def.type_params.clone(),
                superclasses: def.superclasses.clone(),
                superclass_class_modules: def
                    .superclass_class_ids
                    .iter()
                    .map(|id| {
                        id.module
                            .as_identifier()
                            .map(|module| interner.resolve(module).to_string())
                            .unwrap_or_default()
                    })
                    .collect(),
                methods: def
                    .methods
                    .iter()
                    .map(|method| PublicClassMethodEntry {
                        name: method.name,
                        type_params: method.type_params.clone(),
                        param_types: method.param_types.clone(),
                        return_type: method.return_type.clone(),
                        effects: method.effects.clone(),
                    })
                    .collect(),
                default_methods: def.default_methods.clone(),
                method_names,
                pinned_row_placeholder: None,
            }
        })
        .collect();
    entries.sort_by(|a, b| (&a.class_module, &a.name).cmp(&(&b.class_module, &b.name)));
    entries
}

fn class_parameter_kind(
    class: &crate::types::class_env::ClassDef,
    parameter: Identifier,
) -> crate::types::kind::Kind {
    let arity = class
        .methods
        .iter()
        .flat_map(|method| {
            method
                .param_types
                .iter()
                .chain(std::iter::once(&method.return_type))
        })
        .map(|ty| type_expr_parameter_arity(ty, parameter))
        .max()
        .unwrap_or(0);
    kind_from_arity(arity)
}

fn type_expr_parameter_arity(
    ty: &crate::syntax::type_expr::TypeExpr,
    parameter: Identifier,
) -> usize {
    match ty {
        crate::syntax::type_expr::TypeExpr::Named { name, args, .. } => {
            let own = if *name == parameter { args.len() } else { 0 };
            own.max(
                args.iter()
                    .map(|arg| type_expr_parameter_arity(arg, parameter))
                    .max()
                    .unwrap_or(0),
            )
        }
        crate::syntax::type_expr::TypeExpr::Tuple { elements, .. } => elements
            .iter()
            .map(|element| type_expr_parameter_arity(element, parameter))
            .max()
            .unwrap_or(0),
        crate::syntax::type_expr::TypeExpr::Function { params, ret, .. } => params
            .iter()
            .chain(std::iter::once(ret.as_ref()))
            .map(|part| type_expr_parameter_arity(part, parameter))
            .max()
            .unwrap_or(0),
    }
}

/// Kinds of an instance head, taken from the class parameters it instantiates.
///
/// An instance head must have the kind its class parameter declares, so the
/// class is the authority here — the head `TypeExpr` alone cannot distinguish
/// a bare value type from an unapplied constructor. Falls back to `Type` for
/// positions beyond the class's declared parameters.
fn instance_head_kinds(
    env: &ClassEnv,
    inst: &crate::types::class_env::InstanceDef,
) -> Vec<crate::types::kind::Kind> {
    let class = env.classes.get(&inst.class_id);
    inst.type_args
        .iter()
        .enumerate()
        .map(|(index, _)| {
            class
                .and_then(|class| {
                    let param = class.type_params.get(index)?;
                    Some(class_parameter_kind(class, *param))
                })
                .unwrap_or(crate::types::kind::Kind::Type)
        })
        .collect()
}

fn kind_from_arity(arity: usize) -> crate::types::kind::Kind {
    (0..arity).fold(crate::types::kind::Kind::Type, |result, _| {
        crate::types::kind::Kind::Arrow(Box::new(crate::types::kind::Kind::Type), Box::new(result))
    })
}

/// Proposal 0151, Phase 2: extract every `public instance` whose
/// `instance_module` matches `module_sym`. Sorted by
/// `(class_module, class_name, head_type_repr)`.
fn collect_public_instance_entries(
    env: &ClassEnv,
    module_sym: Identifier,
    interner: &Interner,
) -> Vec<PublicInstanceEntry> {
    let mut entries: Vec<PublicInstanceEntry> = env
        .instances
        .iter()
        .filter(|inst| inst.is_public)
        .filter(|inst| inst.instance_module.as_identifier() == Some(module_sym))
        .map(|inst| {
            let class_module = inst
                .class_id
                .module
                .as_identifier()
                .map(|id| interner.resolve(id).to_string())
                .unwrap_or_default();
            let instance_module = inst
                .instance_module
                .as_identifier()
                .map(|id| interner.resolve(id).to_string())
                .unwrap_or_default();
            let head_type_repr: Vec<String> = inst
                .type_args
                .iter()
                .map(|t| t.display_with(interner))
                .collect();
            PublicInstanceEntry {
                class_module,
                class_name: interner.resolve(inst.class_name).to_string(),
                instance_module,
                head_type_repr: head_type_repr.join(", "),
                head_kinds: instance_head_kinds(env, inst),
                type_args: inst.type_args.clone(),
                context: inst.context.clone(),
                context_class_modules: inst
                    .context_class_ids
                    .iter()
                    .map(|id| {
                        id.module
                            .as_identifier()
                            .map(|module| interner.resolve(module).to_string())
                            .unwrap_or_default()
                    })
                    .collect(),
                methods: inst
                    .method_effects
                    .iter()
                    .map(|(name, effects)| PublicInstanceMethodEntry {
                        name: *name,
                        effects: effects.clone(),
                    })
                    .collect(),
                pinned_row_placeholder: None,
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        (&a.class_module, &a.class_name, &a.head_type_repr).cmp(&(
            &b.class_module,
            &b.class_name,
            &b.head_type_repr,
        ))
    });
    entries
}

fn collect_symbols_from_public_classes(entries: &[PublicClassEntry], out: &mut HashSet<Symbol>) {
    for entry in entries {
        for &type_param in &entry.type_params {
            out.insert(type_param);
        }
        for superclass in &entry.superclasses {
            collect_symbols_from_class_constraint(superclass, out);
        }
        for &default_method in &entry.default_methods {
            out.insert(default_method);
        }
        for method in &entry.methods {
            out.insert(method.name);
            for &type_param in &method.type_params {
                out.insert(type_param);
            }
            for param in &method.param_types {
                collect_symbols_from_type_expr(param, out);
            }
            collect_symbols_from_type_expr(&method.return_type, out);
            for effect in &method.effects {
                collect_symbols_from_effect_expr(effect, out);
            }
        }
    }
}

fn collect_symbols_from_public_instances(
    entries: &[PublicInstanceEntry],
    out: &mut HashSet<Symbol>,
) {
    for entry in entries {
        for ty in &entry.type_args {
            collect_symbols_from_type_expr(ty, out);
        }
        for constraint in &entry.context {
            collect_symbols_from_class_constraint(constraint, out);
        }
        for method in &entry.methods {
            out.insert(method.name);
            for effect in &method.effects {
                collect_symbols_from_effect_expr(effect, out);
            }
        }
    }
}

fn collect_symbols_from_class_constraint(
    constraint: &crate::syntax::type_class::ClassConstraint,
    out: &mut HashSet<Symbol>,
) {
    out.insert(constraint.class_name);
    for ty in &constraint.type_args {
        collect_symbols_from_type_expr(ty, out);
    }
}

fn collect_symbols_from_type_expr(
    ty: &crate::syntax::type_expr::TypeExpr,
    out: &mut HashSet<Symbol>,
) {
    match ty {
        crate::syntax::type_expr::TypeExpr::Named { name, args, .. } => {
            out.insert(*name);
            for arg in args {
                collect_symbols_from_type_expr(arg, out);
            }
        }
        crate::syntax::type_expr::TypeExpr::Tuple { elements, .. } => {
            for elem in elements {
                collect_symbols_from_type_expr(elem, out);
            }
        }
        crate::syntax::type_expr::TypeExpr::Function {
            params,
            ret,
            effects,
            ..
        } => {
            for param in params {
                collect_symbols_from_type_expr(param, out);
            }
            collect_symbols_from_type_expr(ret, out);
            for effect in effects {
                collect_symbols_from_effect_expr(effect, out);
            }
        }
    }
}

fn collect_symbols_from_effect_expr(
    effect: &crate::syntax::effect_expr::EffectExpr,
    out: &mut HashSet<Symbol>,
) {
    match effect {
        crate::syntax::effect_expr::EffectExpr::Named { name, .. }
        | crate::syntax::effect_expr::EffectExpr::RowVar { name, .. } => {
            out.insert(*name);
        }
        crate::syntax::effect_expr::EffectExpr::Add { left, right, .. }
        | crate::syntax::effect_expr::EffectExpr::Subtract { left, right, .. } => {
            collect_symbols_from_effect_expr(left, out);
            collect_symbols_from_effect_expr(right, out);
        }
    }
}

pub fn compute_semantic_config_hash(strict_mode: bool, optimize_mode: bool) -> [u8; 32] {
    let mut marker = String::new();
    if optimize_mode {
        marker.push_str("optimize: 1\n");
    }
    if strict_mode {
        marker.push_str("strict: 1\n");
    }
    hash_bytes(marker.as_bytes())
}

pub fn compute_interface_fingerprint(interface: &ModuleInterface) -> String {
    let mut members: Vec<&str> = interface
        .schemes
        .keys()
        .map(String::as_str)
        .chain(interface.borrow_signatures.keys().map(String::as_str))
        .chain(interface.runtime_contracts.keys().map(String::as_str))
        .collect();
    members.sort_unstable();
    members.dedup();

    let exports: Vec<_> = members
        .into_iter()
        .map(|member| CanonicalExport {
            member,
            scheme: interface.schemes.get(member),
            borrow_signature: interface.borrow_signatures.get(member),
            runtime_contract: interface.runtime_contracts.get(member),
            is_value: interface
                .member_is_value
                .get(member)
                .copied()
                .unwrap_or(false),
        })
        .collect();

    // Proposal 0151, Phase 2: the fingerprint also covers the public
    // class/instance tables. Both vectors are pre-sorted in
    // `collect_public_*_entries`, so byte-level serde_json output is
    // deterministic. Adding/removing/modifying a public class or
    // public instance changes the fingerprint, which invalidates
    // downstream `.fxc` cache hits.
    #[derive(Serialize)]
    struct CanonicalInterface<'a> {
        exports: &'a [CanonicalExport<'a>],
        public_classes: &'a [PublicClassEntry],
        public_instances: &'a [PublicInstanceEntry],
        /// constructor field order participates in the
        /// fingerprint because it determines the positional form that
        /// named-field syntax desugars to in *importing* modules.
        ctor_field_names: &'a BTreeMap<String, Vec<String>>,
        /// Importers infer constructor applications from these.
        public_ctor_types: &'a BTreeMap<String, PublicCtorTypeEntry>,
        /// Importers expand these, so a changed alias body changes their types.
        public_type_aliases: &'a BTreeMap<String, PublicTypeAliasEntry>,
    }

    let canonical = CanonicalInterface {
        exports: &exports,
        public_classes: &interface.public_classes,
        public_instances: &interface.public_instances,
        ctor_field_names: &interface.ctor_field_names,
        public_ctor_types: &interface.public_ctor_types,
        public_type_aliases: &interface.public_type_aliases,
    };

    let bytes = serde_json::to_vec(&canonical).expect("canonical interface fingerprint");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(&hasher.finalize())
}

pub fn module_interface_changed(old: &ModuleInterface, new: &ModuleInterface) -> bool {
    old.interface_fingerprint != new.interface_fingerprint
}

/// Compute the centralized `.flxi` cache path for a module source file.
pub fn interface_path(cache_root: &Path, source_path: &Path) -> PathBuf {
    cache_paths::interface_cache_path(cache_root, source_path)
}

/// Save a module interface to disk as JSON.
pub fn save_interface(path: &Path, interface: &ModuleInterface) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(interface).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Load a module interface from disk.
pub fn load_interface(path: &Path) -> Option<ModuleInterface> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn load_cached_interface(
    cache_root: &Path,
    source_path: &Path,
) -> Result<ModuleInterface, InterfaceLoadError> {
    let primary_path = interface_path(cache_root, source_path);
    if primary_path.exists() {
        let interface = load_interface(&primary_path).ok_or(InterfaceLoadError::InvalidJson)?;
        if interface.compiler_version != env!("CARGO_PKG_VERSION") {
            return Err(InterfaceLoadError::CompilerVersionMismatch);
        }
        if interface.cache_format_version != MODULE_INTERFACE_FORMAT_VERSION {
            return Err(InterfaceLoadError::FormatVersionMismatch);
        }
        return Ok(interface);
    }

    Err(InterfaceLoadError::NotFound)
}

pub fn dependency_fingerprints_match(interface: &ModuleInterface, cache_root: &Path) -> bool {
    interface.dependency_fingerprints.iter().all(|dependency| {
        let dependency_path = PathBuf::from(&dependency.source_path);
        let Ok(current) = load_cached_interface(cache_root, &dependency_path) else {
            return false;
        };

        current.compiler_version == env!("CARGO_PKG_VERSION")
            && current.cache_format_version == MODULE_INTERFACE_FORMAT_VERSION
            && current.interface_fingerprint == dependency.interface_fingerprint
    })
}

/// Check whether a cached interface is still valid for the given source.
pub fn load_valid_interface(
    cache_root: &Path,
    source_path: &Path,
    source: &str,
    semantic_config_hash: &[u8; 32],
) -> Result<ModuleInterface, InterfaceLoadError> {
    let interface = load_cached_interface(cache_root, source_path)?;

    if interface.compiler_version != env!("CARGO_PKG_VERSION") {
        return Err(InterfaceLoadError::CompilerVersionMismatch);
    }

    if interface.cache_format_version != MODULE_INTERFACE_FORMAT_VERSION {
        return Err(InterfaceLoadError::FormatVersionMismatch);
    }

    let current_hash = hex::encode(&hash_bytes(source.as_bytes()));
    if interface.source_hash != current_hash {
        return Err(InterfaceLoadError::SourceHashMismatch);
    }

    if interface.semantic_config_hash != hex::encode(semantic_config_hash) {
        return Err(InterfaceLoadError::SemanticConfigMismatch);
    }

    for dependency in &interface.dependency_fingerprints {
        let dependency_path = PathBuf::from(&dependency.source_path);
        let Ok(current) = load_cached_interface(cache_root, &dependency_path) else {
            return Err(InterfaceLoadError::DependencyFingerprintMismatch {
                module_name: dependency.module_name.clone(),
                source_path: dependency.source_path.clone(),
                reason: DependencyMissReason::InterfaceMissing,
            });
        };
        if current.compiler_version != env!("CARGO_PKG_VERSION") {
            return Err(InterfaceLoadError::DependencyFingerprintMismatch {
                module_name: dependency.module_name.clone(),
                source_path: dependency.source_path.clone(),
                reason: DependencyMissReason::CompilerVersionChanged,
            });
        }
        if current.cache_format_version != MODULE_INTERFACE_FORMAT_VERSION {
            return Err(InterfaceLoadError::DependencyFingerprintMismatch {
                module_name: dependency.module_name.clone(),
                source_path: dependency.source_path.clone(),
                reason: DependencyMissReason::FormatVersionChanged,
            });
        }
        if current.interface_fingerprint != dependency.interface_fingerprint {
            return Err(InterfaceLoadError::DependencyFingerprintMismatch {
                module_name: dependency.module_name.clone(),
                source_path: dependency.source_path.clone(),
                reason: DependencyMissReason::InterfaceFingerprintChanged,
            });
        }
    }

    Ok(interface)
}

/// Collect `constructor Symbol -> type metadata` for every `public data`
/// declaration in `program`, with transparent aliases expanded in field types.
///
/// The interface path ([`collect_public_ctor_types`]) keys by name because a
/// `.flxi` is serialized across sessions and its `Symbol`s must be remapped on
/// load. A dependency compiled in *this* run has no interface to read back —
/// its AST is already in-session — so importers seed inference straight from
/// the program, keyed by the live `Symbol`. Without this the KI-014 metadata
/// reaches an importer only through a warm cache, and a fresh build infers an
/// imported constructor's payload as an unresolved variable (KI-022).
pub fn collect_dependency_ctor_types(
    program: &crate::syntax::program::Program,
    interner: &Interner,
) -> std::collections::HashMap<Symbol, PublicCtorTypeEntry> {
    let mut by_name = collect_public_ctor_types(program, interner);
    expand_aliases_in_ctor_types(&mut by_name, program);
    by_name
        .into_iter()
        .filter_map(|(name, entry)| interner.lookup(&name).map(|sym| (sym, entry)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        aether::borrow_infer::{BorrowMode, BorrowProvenance, BorrowSignature},
        core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreLit},
        runtime::{
            function_contract::FunctionContract,
            runtime_type::{AdtConstructorContract, RuntimeType},
        },
        types::{
            infer_effect_row::InferEffectRow, infer_type::InferType,
            module_interface::DependencyFingerprint, scheme::Scheme,
            type_constructor::TypeConstructor,
        },
    };

    #[test]
    fn build_interface_exports_public_schemes_and_borrow_signatures() {
        let mut interner = Interner::new();
        let module = interner.intern("Base.List");
        let public_name = interner.intern("map");
        let private_name = interner.intern("helper");

        let mut program = CoreProgram {
            defs: vec![
                CoreDef::new(
                    CoreBinder::new(CoreBinderId(0), public_name),
                    CoreExpr::Lit(CoreLit::Unit, Default::default()),
                    false,
                    Default::default(),
                ),
                CoreDef::new(
                    CoreBinder::new(CoreBinderId(1), private_name),
                    CoreExpr::Lit(CoreLit::Unit, Default::default()),
                    false,
                    Default::default(),
                ),
            ],
            top_level_items: Vec::new(),
        };
        program.defs[0].borrow_signature = Some(BorrowSignature::new(
            vec![BorrowMode::Borrowed, BorrowMode::Borrowed],
            BorrowProvenance::Inferred,
        ));
        program.defs[1].borrow_signature = Some(BorrowSignature::new(
            vec![BorrowMode::Owned],
            BorrowProvenance::Inferred,
        ));

        let mut schemes = HashMap::new();
        schemes.insert(
            (module, public_name),
            Scheme {
                forall: vec![0, 1],
                constraints: vec![],
                infer_type: InferType::Fun(
                    vec![
                        InferType::App(TypeConstructor::List, vec![InferType::Var(0)]),
                        InferType::Fun(
                            vec![InferType::Var(0)],
                            Box::new(InferType::Var(1)),
                            InferEffectRow::closed_empty(),
                        ),
                    ],
                    Box::new(InferType::App(
                        TypeConstructor::List,
                        vec![InferType::Var(1)],
                    )),
                    InferEffectRow::closed_empty(),
                ),
            },
        );
        schemes.insert(
            (module, private_name),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );

        let visibility = HashMap::from([
            ((module, public_name), true),
            ((module, private_name), false),
        ]);
        let hash = crate::bytecode::bytecode_cache::hash_bytes(b"module Base.List {}");
        let semantic_hash = compute_semantic_config_hash(false, false);
        let interface = build_interface(
            interner.resolve(module),
            module,
            &hash,
            &semantic_hash,
            &program,
            &schemes,
            &HashMap::new(),
            &visibility,
            None,
            vec![DependencyFingerprint {
                module_name: "Flow.Prelude".to_string(),
                source_path: "lib/Flow/Prelude.flx".to_string(),
                interface_fingerprint: "abc".to_string(),
            }],
            &interner,
            None,
        );

        assert_eq!(interface.module_name, "Base.List");
        assert_eq!(
            interface.cache_format_version,
            MODULE_INTERFACE_FORMAT_VERSION
        );
        assert!(interface.schemes.contains_key("map"));
        assert!(interface.borrow_signatures.contains_key("map"));
        assert!(!interface.schemes.contains_key("helper"));
        assert!(!interface.borrow_signatures.contains_key("helper"));
        assert_eq!(interface.dependency_fingerprints.len(), 1);
        assert!(!interface.interface_fingerprint.is_empty());
    }

    #[test]
    fn build_interface_populates_symbol_table_for_adt_and_effects() {
        let mut interner = Interner::new();
        let module = interner.intern("Test.Mod");
        let fn_name = interner.intern("make");
        let adt_sym = interner.intern("Color");
        let effect_sym = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);

        let program = CoreProgram {
            defs: vec![CoreDef::new(
                CoreBinder::new(CoreBinderId(0), fn_name),
                CoreExpr::Lit(CoreLit::Unit, Default::default()),
                false,
                Default::default(),
            )],
            top_level_items: Vec::new(),
        };

        let mut schemes = HashMap::new();
        schemes.insert(
            (module, fn_name),
            Scheme {
                forall: vec![],
                constraints: vec![],
                infer_type: InferType::Fun(
                    vec![InferType::Con(TypeConstructor::Adt(adt_sym))],
                    Box::new(InferType::Con(TypeConstructor::Unit)),
                    InferEffectRow::closed_from_symbols([effect_sym]),
                ),
            },
        );

        let visibility = HashMap::from([((module, fn_name), true)]);
        let hash = crate::bytecode::bytecode_cache::hash_bytes(b"test");
        let semantic_hash = compute_semantic_config_hash(false, false);
        let interface = build_interface(
            interner.resolve(module),
            module,
            &hash,
            &semantic_hash,
            &program,
            &schemes,
            &HashMap::new(),
            &visibility,
            None,
            Vec::new(),
            &interner,
            None,
        );

        // Symbol table should contain both the ADT and effect symbols.
        assert!(
            interface.symbol_table.len() >= 2,
            "expected at least 2 entries, got: {:?}",
            interface.symbol_table
        );
        assert_eq!(
            interface.symbol_table.get(&adt_sym.as_u32()),
            Some(&"Color".to_string())
        );
        assert_eq!(
            interface.symbol_table.get(&effect_sym.as_u32()),
            Some(&"IO".to_string())
        );
    }

    #[test]
    fn build_interface_symbol_table_empty_for_builtin_only_schemes() {
        let mut interner = Interner::new();
        let module = interner.intern("Test.Simple");
        let fn_name = interner.intern("id");

        let program = CoreProgram {
            defs: vec![CoreDef::new(
                CoreBinder::new(CoreBinderId(0), fn_name),
                CoreExpr::Lit(CoreLit::Unit, Default::default()),
                false,
                Default::default(),
            )],
            top_level_items: Vec::new(),
        };

        let mut schemes = HashMap::new();
        schemes.insert(
            (module, fn_name),
            Scheme {
                forall: vec![0],
                constraints: vec![],
                infer_type: InferType::Fun(
                    vec![InferType::Var(0)],
                    Box::new(InferType::Var(0)),
                    InferEffectRow::closed_empty(),
                ),
            },
        );

        let visibility = HashMap::from([((module, fn_name), true)]);
        let hash = crate::bytecode::bytecode_cache::hash_bytes(b"test");
        let semantic_hash = compute_semantic_config_hash(false, false);
        let interface = build_interface(
            interner.resolve(module),
            module,
            &hash,
            &semantic_hash,
            &program,
            &schemes,
            &HashMap::new(),
            &visibility,
            None,
            Vec::new(),
            &interner,
            None,
        );

        assert!(
            interface.symbol_table.is_empty(),
            "builtin-only schemes should not populate symbol_table"
        );
    }

    #[test]
    fn interface_roundtrip() {
        let mut schemes = HashMap::new();
        schemes.insert(
            "map".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        let mut borrow_signatures = HashMap::new();
        borrow_signatures.insert(
            "map".to_string(),
            BorrowSignature::new(vec![BorrowMode::Borrowed], BorrowProvenance::Imported),
        );
        let mut runtime_contracts = HashMap::new();
        runtime_contracts.insert(
            "map".to_string(),
            FunctionContract {
                params: vec![Some(RuntimeType::Adt {
                    module_name: None,
                    module_name_text: None,
                    name: crate::syntax::symbol::Symbol::new(7),
                    display_name: "Color".to_string(),
                    type_args: vec![],
                    constructors: vec![AdtConstructorContract {
                        name: crate::syntax::symbol::Symbol::new(8),
                        display_name: "Red".to_string(),
                        fields: vec![],
                    }],
                })],
                ret: Some(RuntimeType::Unit),
                effects: vec![],
            },
        );

        let interface = ModuleInterface {
            module_name: "TestMod".to_string(),
            source_hash: "abc123".to_string(),
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            cache_format_version: MODULE_INTERFACE_FORMAT_VERSION,
            semantic_config_hash: "config".to_string(),
            interface_fingerprint: "abi".to_string(),
            schemes,
            borrow_signatures,
            runtime_contracts,
            member_is_value: HashMap::from([("map".to_string(), false)]),
            dependency_fingerprints: vec![DependencyFingerprint {
                module_name: "Flow.List".to_string(),
                source_path: "lib/Flow/List.flx".to_string(),
                interface_fingerprint: "dep".to_string(),
            }],
            symbol_table: HashMap::new(),
            public_classes: Vec::new(),
            public_instances: Vec::new(),
            ctor_field_names: BTreeMap::new(),
            public_ctor_types: BTreeMap::new(),
            public_type_aliases: BTreeMap::new(),
        };

        let json = serde_json::to_string_pretty(&interface).unwrap();
        let loaded: ModuleInterface = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.module_name, "TestMod");
        assert_eq!(loaded.schemes.len(), 1);
        assert_eq!(loaded.borrow_signatures.len(), 1);
        assert_eq!(loaded.runtime_contracts.len(), 1);
        assert!(loaded.schemes.contains_key("map"));
        assert_eq!(loaded.member_is_value.get("map"), Some(&false));
    }

    #[test]
    fn fingerprint_ignores_private_exports_by_shape() {
        let mut a = ModuleInterface::new("Mod", "src", "cfg");
        let mut b = ModuleInterface::new("Mod", "src2", "cfg");
        a.schemes.insert(
            "pub".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        b.schemes.insert(
            "pub".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        a.borrow_signatures.insert(
            "pub".to_string(),
            BorrowSignature::new(vec![BorrowMode::Borrowed], BorrowProvenance::Imported),
        );
        b.borrow_signatures.insert(
            "pub".to_string(),
            BorrowSignature::new(vec![BorrowMode::Borrowed], BorrowProvenance::Imported),
        );

        a.interface_fingerprint = compute_interface_fingerprint(&a);
        b.interface_fingerprint = compute_interface_fingerprint(&b);

        assert_eq!(a.interface_fingerprint, b.interface_fingerprint);
    }

    /// constructor field *order* determines the positional form
    /// that named-field syntax desugars to in importing modules, so reordering
    /// fields is an interface-breaking change and must move the fingerprint.
    #[test]
    fn ctor_field_order_changes_the_interface_fingerprint() {
        let mut a = ModuleInterface::new("M", "src", "cfg");
        a.ctor_field_names.insert(
            "IoError".to_string(),
            vec!["kind".to_string(), "message".to_string()],
        );
        let fingerprint_a = compute_interface_fingerprint(&a);

        let mut b = ModuleInterface::new("M", "src", "cfg");
        b.ctor_field_names.insert(
            "IoError".to_string(),
            vec!["message".to_string(), "kind".to_string()],
        );
        let fingerprint_b = compute_interface_fingerprint(&b);

        assert_ne!(
            fingerprint_a, fingerprint_b,
            "swapping two constructor field names must change the fingerprint; \
             importers desugar named fields to this order"
        );
    }

    /// Adding a record constructor to a module changes what importers can
    /// desugar, so it must invalidate downstream caches too.
    #[test]
    fn adding_a_record_constructor_changes_the_interface_fingerprint() {
        let bare = ModuleInterface::new("M", "src", "cfg");
        let mut with_ctor = ModuleInterface::new("M", "src", "cfg");
        with_ctor
            .ctor_field_names
            .insert("IoError".to_string(), vec!["kind".to_string()]);

        assert_ne!(
            compute_interface_fingerprint(&bare),
            compute_interface_fingerprint(&with_ctor),
            "adding a record constructor must change the fingerprint"
        );
    }

    /// Proposal 0151, Phase 2: an empty `.flxi` round-trips its
    /// `public_classes` and `public_instances` vectors as JSON.
    #[test]
    fn public_class_and_instance_entries_roundtrip_through_json() {
        use crate::types::module_interface::{PublicClassEntry, PublicInstanceEntry};

        let mut interface = ModuleInterface::new("Mod.A", "src", "cfg");
        interface.public_classes.push(PublicClassEntry {
            class_module: "Mod.A".to_string(),
            name: "MyShow".to_string(),
            type_param_arity: 1,
            parameter_kinds: vec![],
            type_params: vec![],
            superclasses: vec![],
            superclass_class_modules: vec![],
            methods: vec![],
            default_methods: vec![],
            method_names: vec!["my_show".to_string()],
            pinned_row_placeholder: None,
        });
        interface.public_instances.push(PublicInstanceEntry {
            class_module: "Mod.A".to_string(),
            class_name: "MyShow".to_string(),
            instance_module: "Mod.A".to_string(),
            head_type_repr: "Int".to_string(),
            head_kinds: vec![],
            type_args: vec![],
            context: vec![],
            context_class_modules: vec![],
            methods: vec![],
            pinned_row_placeholder: None,
        });

        let json = serde_json::to_string_pretty(&interface).expect("serialize");
        let decoded: ModuleInterface = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.public_classes.len(), 1);
        assert_eq!(decoded.public_classes[0].name, "MyShow");
        assert_eq!(decoded.public_instances.len(), 1);
        assert_eq!(decoded.public_instances[0].head_type_repr, "Int");
    }

    /// Proposal 0151, Phase 2: an old `.flxi` written before the new
    /// fields existed must still load — the `#[serde(default)]` on
    /// `public_classes` and `public_instances` makes them optional.
    #[test]
    fn old_flxi_loads_without_class_fields() {
        let json = r#"{
            "module_name": "Old.Mod",
            "source_hash": "abc",
            "compiler_version": "0.0.1",
            "cache_format_version": 1,
            "semantic_config_hash": "cfg",
            "interface_fingerprint": "fp",
            "schemes": {},
            "borrow_signatures": {},
            "dependency_fingerprints": []
        }"#;
        let decoded: ModuleInterface = serde_json::from_str(json).expect("backward-compat load");
        assert!(decoded.public_classes.is_empty());
        assert!(decoded.public_instances.is_empty());
    }

    /// Proposal 0151, Phase 2: adding a `public class` entry to the
    /// interface changes its fingerprint, so the `.fxc` cache stays
    /// sound when an upstream module gains a new public class.
    #[test]
    fn fingerprint_changes_when_public_class_is_added() {
        use crate::types::module_interface::PublicClassEntry;

        let base = ModuleInterface::new("Mod.A", "src", "cfg");
        let mut with_class = base.clone();
        with_class.public_classes.push(PublicClassEntry {
            class_module: "Mod.A".to_string(),
            name: "MyShow".to_string(),
            type_param_arity: 1,
            parameter_kinds: vec![],
            type_params: vec![],
            superclasses: vec![],
            superclass_class_modules: vec![],
            methods: vec![],
            default_methods: vec![],
            method_names: vec!["my_show".to_string()],
            pinned_row_placeholder: None,
        });

        assert_ne!(
            compute_interface_fingerprint(&base),
            compute_interface_fingerprint(&with_class),
            "adding a public class must change the interface fingerprint"
        );
    }

    /// Symmetric: adding a `public instance` also changes the fingerprint.
    #[test]
    fn fingerprint_changes_when_public_instance_is_added() {
        use crate::types::module_interface::PublicInstanceEntry;

        let base = ModuleInterface::new("Mod.A", "src", "cfg");
        let mut with_inst = base.clone();
        with_inst.public_instances.push(PublicInstanceEntry {
            class_module: "Mod.A".to_string(),
            class_name: "MyShow".to_string(),
            instance_module: "Mod.A".to_string(),
            head_type_repr: "Int".to_string(),
            head_kinds: vec![],
            type_args: vec![],
            context: vec![],
            context_class_modules: vec![],
            methods: vec![],
            pinned_row_placeholder: None,
        });

        assert_ne!(
            compute_interface_fingerprint(&base),
            compute_interface_fingerprint(&with_inst),
            "adding a public instance must change the interface fingerprint"
        );
    }

    /// Negative: adding a *private* (non-public) class to the live
    /// `ClassEnv` does NOT show up in the interface, so the fingerprint
    /// stays unchanged. We exercise this through `build_interface` to
    /// confirm the public-only filter is correct.
    #[test]
    fn fingerprint_unchanged_when_only_private_class_added() {
        use crate::types::class_env::{ClassDef, ClassEnv};
        use crate::types::class_id::{ClassId, ModulePath};

        let mut interner = Interner::new();
        let module = interner.intern("Mod.A");
        let priv_name = interner.intern("PrivShow");

        // Empty env baseline.
        let env_empty = ClassEnv::new();
        let hash = hash_bytes(b"src");
        let cfg = compute_semantic_config_hash(false, false);
        let program = CoreProgram {
            defs: Vec::new(),
            top_level_items: Vec::new(),
        };
        let schemes = HashMap::new();
        let visibility = HashMap::new();

        let iface_empty = build_interface(
            "Mod.A",
            module,
            &hash,
            &cfg,
            &program,
            &schemes,
            &HashMap::new(),
            &visibility,
            Some(&env_empty),
            Vec::new(),
            &interner,
            None,
        );

        // Same module, but with one PRIVATE class registered.
        let mut env_priv = ClassEnv::new();
        let class_id = ClassId::new(ModulePath::from_identifier(module), priv_name);
        env_priv.classes.insert(
            class_id,
            ClassDef {
                name: priv_name,
                module: ModulePath::from_identifier(module),
                is_public: false,
                is_builtin: false,
                type_params: vec![interner.intern("a")],
                superclasses: vec![],
                superclass_class_ids: vec![],
                associated_types: vec![],
                methods: vec![],
                default_methods: vec![],
                span: Default::default(),
            },
        );

        let iface_priv = build_interface(
            "Mod.A",
            module,
            &hash,
            &cfg,
            &program,
            &schemes,
            &HashMap::new(),
            &visibility,
            Some(&env_priv),
            Vec::new(),
            &interner,
            None,
        );

        assert_eq!(
            iface_empty.interface_fingerprint, iface_priv.interface_fingerprint,
            "private classes must not affect the interface fingerprint"
        );
        assert!(
            iface_priv.public_classes.is_empty(),
            "private classes must not appear in public_classes"
        );
    }

    #[test]
    fn fingerprint_changes_when_borrow_signature_changes() {
        let mut borrowed = ModuleInterface::new("Mod", "src", "cfg");
        let mut owned = ModuleInterface::new("Mod", "src", "cfg");
        borrowed.schemes.insert(
            "pub".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        owned.schemes.insert(
            "pub".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        borrowed.borrow_signatures.insert(
            "pub".to_string(),
            BorrowSignature::new(vec![BorrowMode::Borrowed], BorrowProvenance::Imported),
        );
        owned.borrow_signatures.insert(
            "pub".to_string(),
            BorrowSignature::new(vec![BorrowMode::Owned], BorrowProvenance::Imported),
        );

        assert_ne!(
            compute_interface_fingerprint(&borrowed),
            compute_interface_fingerprint(&owned)
        );
    }

    #[test]
    fn interface_path_uses_centralized_cache_root() {
        let path = interface_path(
            Path::new("target/flux"),
            Path::new("examples/aoc/2024/Day07Solver.flx"),
        );
        assert_eq!(path.parent().unwrap(), Path::new("target/flux/interfaces"));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Day07Solver-")
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".flxi")
        );
    }

    #[test]
    fn dependency_fingerprint_mismatch_invalidates_interface() {
        let temp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!(
                "flux_interface_test_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(temp.join("interfaces")).unwrap();

        let dep_source = temp.join("Dep.flx");
        let dep_hash = hash_bytes(b"dep");
        let dep_cfg = compute_semantic_config_hash(false, false);
        let mut dep_interface =
            ModuleInterface::new("Dep", hex::encode(&dep_hash), hex::encode(&dep_cfg));
        dep_interface.schemes.insert(
            "value".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        dep_interface.interface_fingerprint = compute_interface_fingerprint(&dep_interface);
        save_interface(&interface_path(&temp, &dep_source), &dep_interface).unwrap();

        let source = "fn main() { 1 }";
        let source_hash = hash_bytes(source.as_bytes());
        let mut interface =
            ModuleInterface::new("Main", hex::encode(&source_hash), hex::encode(&dep_cfg));
        interface.schemes.insert(
            "main".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        interface
            .dependency_fingerprints
            .push(DependencyFingerprint {
                module_name: "Dep".to_string(),
                source_path: dep_source.to_string_lossy().to_string(),
                interface_fingerprint: "stale".to_string(),
            });
        interface.interface_fingerprint = compute_interface_fingerprint(&interface);
        save_interface(
            &interface_path(&temp, temp.join("Main.flx").as_path()),
            &interface,
        )
        .unwrap();

        let result = load_valid_interface(&temp, temp.join("Main.flx").as_path(), source, &dep_cfg);
        assert!(matches!(
            result,
            Err(InterfaceLoadError::DependencyFingerprintMismatch { .. })
        ));

        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn semantic_config_mismatch_invalidates_interface() {
        let temp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!(
                "flux_interface_cfg_test_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(temp.join("interfaces")).unwrap();

        let source_path = temp.join("Main.flx");
        let source = "fn main() { 1 }";
        let source_hash = hash_bytes(source.as_bytes());
        let current_cfg = compute_semantic_config_hash(false, false);
        let stale_cfg = compute_semantic_config_hash(true, false);
        let mut interface =
            ModuleInterface::new("Main", hex::encode(&source_hash), hex::encode(&stale_cfg));
        interface.schemes.insert(
            "main".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        interface.interface_fingerprint = compute_interface_fingerprint(&interface);
        save_interface(&interface_path(&temp, &source_path), &interface).unwrap();

        let result = load_valid_interface(&temp, &source_path, source, &current_cfg);
        assert!(matches!(
            result,
            Err(InterfaceLoadError::SemanticConfigMismatch)
        ));

        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn format_version_mismatch_invalidates_interface() {
        let temp = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!(
                "flux_interface_fmt_test_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(temp.join("interfaces")).unwrap();

        let source_path = temp.join("Main.flx");
        let source = "fn main() { 1 }";
        let source_hash = hash_bytes(source.as_bytes());
        let cfg = compute_semantic_config_hash(false, false);
        let mut interface =
            ModuleInterface::new("Main", hex::encode(&source_hash), hex::encode(&cfg));
        interface.cache_format_version = MODULE_INTERFACE_FORMAT_VERSION + 1;
        interface.schemes.insert(
            "main".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        );
        interface.interface_fingerprint = compute_interface_fingerprint(&interface);
        save_interface(&interface_path(&temp, &source_path), &interface).unwrap();

        let result = load_valid_interface(&temp, &source_path, source, &cfg);
        assert!(matches!(
            result,
            Err(InterfaceLoadError::FormatVersionMismatch)
        ));

        std::fs::remove_dir_all(temp).ok();
    }
}
