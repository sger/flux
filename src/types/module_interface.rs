use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{aether::borrow_infer::BorrowSignature, syntax::symbol::Symbol, types::scheme::Scheme};

pub const MODULE_INTERFACE_FORMAT_VERSION: u16 = crate::cache_paths::CACHE_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyFingerprint {
    pub module_name: String,
    pub source_path: String,
    pub interface_fingerprint: String,
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
