use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    aether::borrow_infer::BorrowSignature,
    syntax::{Identifier, effect_expr::EffectExpr, symbol::Symbol, type_class::ClassConstraint, type_expr::TypeExpr},
    types::scheme::Scheme,
};

pub const MODULE_INTERFACE_FORMAT_VERSION: u16 = crate::cache_paths::CACHE_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyFingerprint {
    pub module_name: String,
    pub source_path: String,
    pub interface_fingerprint: String,
}

/// Proposal 0151, Phase 2: a `public class` entry recorded in a module
/// interface so downstream modules can resolve constraints against it.
///
/// `class_module` is the dotted path of the module that declared the
/// class (i.e. the `ModulePath` half of the canonical `ClassId`). The
/// pair `(class_module, name)` is globally unique. `superclasses` lists
/// the short names of declared superclass constraints; full ClassId
/// resolution for superclasses lands in a later phase.
///
/// `pinned_row_placeholder` is reserved for Phase 4 (effects on instance
/// methods) and is currently always `None`. The field exists in the
/// schema now so that pre-Phase-4 interfaces can be reloaded post-Phase-4
/// without a cache format bump for this single field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicClassMethodEntry {
    pub name: Identifier,
    pub type_params: Vec<Identifier>,
    pub param_types: Vec<TypeExpr>,
    pub return_type: TypeExpr,
    #[serde(default)]
    pub effects: Vec<EffectExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicInstanceMethodEntry {
    pub name: Identifier,
    #[serde(default)]
    pub effects: Vec<EffectExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicClassEntry {
    pub class_module: String,
    pub name: String,
    pub type_param_arity: usize,
    #[serde(default)]
    pub type_params: Vec<Identifier>,
    #[serde(default)]
    pub superclasses: Vec<ClassConstraint>,
    #[serde(default)]
    pub methods: Vec<PublicClassMethodEntry>,
    #[serde(default)]
    pub default_methods: Vec<Identifier>,
    #[serde(default)]
    pub method_names: Vec<String>,
    #[serde(default)]
    pub pinned_row_placeholder: Option<String>,
}

/// Proposal 0151, Phase 2: a `public instance` entry recorded in a
/// module interface. Like `PublicClassEntry`, this is a denormalized
/// snapshot of the relevant fields from the `ClassEnv` `InstanceDef`.
///
/// `class_module` and `class_name` together identify the implemented
/// class. `instance_module` is the module where the instance block
/// itself lives — possibly different from `class_module` if the
/// instance is allowed by the orphan rule via the head type.
/// `head_type_repr` is a textual rendering of the head type (sufficient
/// for cache invalidation; full structural matching uses the in-memory
/// `ClassEnv`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicInstanceEntry {
    pub class_module: String,
    pub class_name: String,
    pub instance_module: String,
    pub head_type_repr: String,
    #[serde(default)]
    pub type_args: Vec<TypeExpr>,
    #[serde(default)]
    pub context: Vec<ClassConstraint>,
    #[serde(default)]
    pub methods: Vec<PublicInstanceMethodEntry>,
    #[serde(default)]
    pub pinned_row_placeholder: Option<String>,
}

/// Serializable compiled interface for a Flux module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInterface {
    /// Module name (for example `Base.List`).
    pub module_name: String,
    /// SHA-256 of the source file used to build this interface.
    pub source_hash: String,
    /// Compiler version used to produce this interface.
    pub compiler_version: String,
    /// Version of the on-disk module interface format.
    pub cache_format_version: u16,
    /// Hash of compiler settings that affect semantic output.
    pub semantic_config_hash: String,
    /// Hash of the exported semantic interface.
    pub interface_fingerprint: String,
    /// Exported type schemes keyed by unqualified function name.
    pub schemes: HashMap<String, Scheme>,
    /// Exported borrow signatures keyed by unqualified function name.
    pub borrow_signatures: HashMap<String, BorrowSignature>,
    /// Fingerprints of direct imported module interfaces used to compile this module.
    pub dependency_fingerprints: Vec<DependencyFingerprint>,
    /// Portable symbol table: maps serialized Symbol u32 IDs to their string names.
    ///
    /// Symbols are interner indices that are session-specific. This table records
    /// the mapping so that consumers can re-intern the strings and remap Symbol IDs
    /// when loading the interface in a different compilation session.
    #[serde(default)]
    pub symbol_table: HashMap<u32, String>,
    /// Proposal 0151, Phase 2: `public class` entries owned by this module.
    ///
    /// Each entry corresponds to a `public class` declaration whose
    /// owning module matches this interface's `module_name`. Sorted by
    /// `(class_module, name)` for deterministic fingerprinting.
    #[serde(default)]
    pub public_classes: Vec<PublicClassEntry>,
    /// Proposal 0151, Phase 2: `public instance` entries owned by this module.
    ///
    /// Each entry corresponds to a `public instance` declaration whose
    /// `instance_module` matches this interface's `module_name`. Sorted
    /// by `(class_module, class_name, head_type_repr)`.
    #[serde(default)]
    pub public_instances: Vec<PublicInstanceEntry>,
}

/// Sub-reason for a dependency fingerprint cache miss.
///
/// When a cached module is invalidated because one of its dependencies
/// changed, this enum tells you *which field* of that dependency was
/// the mismatch trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyMissReason {
    /// The dependency's cached interface could not be loaded at all.
    InterfaceMissing,
    /// The dependency was compiled with a different compiler version.
    CompilerVersionChanged,
    /// The dependency's cache format version doesn't match the current one.
    FormatVersionChanged,
    /// The dependency's exported interface fingerprint changed (i.e. its
    /// public API or borrow signatures differ from what was recorded).
    InterfaceFingerprintChanged,
}

impl DependencyMissReason {
    pub fn label(&self) -> &'static str {
        match self {
            Self::InterfaceMissing => "interface missing",
            Self::CompilerVersionChanged => "compiler version changed",
            Self::FormatVersionChanged => "format version changed",
            Self::InterfaceFingerprintChanged => "interface fingerprint changed",
        }
    }
}

impl ModuleInterface {
    /// Build a remapping from serialized Symbol IDs to freshly interned ones.
    ///
    /// Call this after loading an interface from disk. The returned map translates
    /// old (session-specific) Symbol u32 values to new ones valid in `interner`.
    pub fn build_symbol_remap(
        &self,
        interner: &mut crate::syntax::interner::Interner,
    ) -> HashMap<Symbol, Symbol> {
        let mut remap = HashMap::new();
        for (&old_id, name) in &self.symbol_table {
            let old_sym = Symbol::new(old_id);
            let new_sym = interner.intern(name);
            if old_sym != new_sym {
                remap.insert(old_sym, new_sym);
            }
        }
        remap
    }

    pub fn new(
        module_name: impl Into<String>,
        source_hash: impl Into<String>,
        semantic_config_hash: impl Into<String>,
    ) -> Self {
        Self {
            module_name: module_name.into(),
            source_hash: source_hash.into(),
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            cache_format_version: MODULE_INTERFACE_FORMAT_VERSION,
            semantic_config_hash: semantic_config_hash.into(),
            interface_fingerprint: String::new(),
            schemes: HashMap::new(),
            borrow_signatures: HashMap::new(),
            dependency_fingerprints: Vec::new(),
            symbol_table: HashMap::new(),
            public_classes: Vec::new(),
            public_instances: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DependencyFingerprint, MODULE_INTERFACE_FORMAT_VERSION, ModuleInterface};
    use crate::{
        aether::borrow_infer::{BorrowMode, BorrowProvenance, BorrowSignature},
        types::{
            infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme,
            type_constructor::TypeConstructor,
        },
    };

    #[test]
    fn build_symbol_remap_translates_stale_ids() {
        use crate::syntax::{interner::Interner, symbol::Symbol};

        let mut interface = ModuleInterface::new("Test", "hash", "cfg");
        // Simulate an interface written with Symbol(5) = "IO"
        interface.symbol_table.insert(5, "IO".to_string());
        interface.symbol_table.insert(10, "MyAdt".to_string());

        let mut interner = Interner::new();
        // In the new session, "IO" gets a different index
        let io_sym = interner.intern("IO");
        let adt_sym = interner.intern("MyAdt");

        let remap = interface.build_symbol_remap(&mut interner);

        // Old Symbol(5) should map to the new IO symbol
        if io_sym != Symbol::new(5) {
            assert_eq!(remap.get(&Symbol::new(5)), Some(&io_sym));
        }
        if adt_sym != Symbol::new(10) {
            assert_eq!(remap.get(&Symbol::new(10)), Some(&adt_sym));
        }
    }

    #[test]
    fn build_symbol_remap_empty_when_ids_match() {
        use crate::syntax::interner::Interner;

        let mut interner = Interner::new();
        let sym = interner.intern("IO");

        let mut interface = ModuleInterface::new("Test", "hash", "cfg");
        interface
            .symbol_table
            .insert(sym.as_u32(), "IO".to_string());

        let remap = interface.build_symbol_remap(&mut interner);
        assert!(remap.is_empty());
    }

    #[test]
    fn symbol_table_roundtrips_through_json() {
        use crate::syntax::symbol::Symbol;

        let mut interface = ModuleInterface::new("Test", "hash", "cfg");
        interface.symbol_table.insert(5, "IO".to_string());
        interface.symbol_table.insert(10, "MyAdt".to_string());
        interface.schemes.insert(
            "run".to_string(),
            Scheme::mono(InferType::Fun(
                vec![InferType::Con(TypeConstructor::Adt(Symbol::new(10)))],
                Box::new(InferType::Con(TypeConstructor::Unit)),
                InferEffectRow::closed_from_symbols([Symbol::new(5)]),
            )),
        );

        let json = serde_json::to_string(&interface).expect("serialize");
        let decoded: ModuleInterface = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.symbol_table.len(), 2);
        assert_eq!(decoded.symbol_table.get(&5), Some(&"IO".to_string()));
        assert_eq!(decoded.symbol_table.get(&10), Some(&"MyAdt".to_string()));
        assert_eq!(decoded.schemes, interface.schemes);
    }

    #[test]
    fn symbol_table_defaults_empty_for_old_format() {
        // Simulates loading an old .flxi without the symbol_table field
        let json = r#"{
            "module_name": "Old",
            "source_hash": "abc",
            "compiler_version": "0.0.1",
            "cache_format_version": 1,
            "semantic_config_hash": "cfg",
            "interface_fingerprint": "fp",
            "schemes": {},
            "borrow_signatures": {},
            "dependency_fingerprints": []
        }"#;
        let decoded: ModuleInterface = serde_json::from_str(json).expect("deserialize old format");
        assert!(decoded.symbol_table.is_empty());
    }

    #[test]
    fn module_interface_roundtrips_with_scheme_and_borrow_metadata() {
        let mut interface = ModuleInterface::new("Base.List", "deadbeef", "config");
        interface.schemes.insert(
            "map".to_string(),
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
        interface.borrow_signatures.insert(
            "map".to_string(),
            BorrowSignature::new(
                vec![BorrowMode::Borrowed, BorrowMode::Borrowed],
                BorrowProvenance::Imported,
            ),
        );
        interface.interface_fingerprint = "f00d".to_string();
        interface
            .dependency_fingerprints
            .push(DependencyFingerprint {
                module_name: "Flow.List".to_string(),
                source_path: "lib/Flow/List.flx".to_string(),
                interface_fingerprint: "beef".to_string(),
            });

        let json = serde_json::to_string(&interface).expect("module interface should serialize");
        let decoded: ModuleInterface =
            serde_json::from_str(&json).expect("module interface should deserialize");

        assert_eq!(decoded, interface);
        assert_eq!(
            decoded.cache_format_version,
            MODULE_INTERFACE_FORMAT_VERSION
        );
    }
}
