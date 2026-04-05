use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{aether::borrow_infer::BorrowSignature, types::scheme::Scheme};

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
