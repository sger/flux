//! Module interface files (`.flxi`) for separate compilation.
//!
//! The interface stores exported HM schemes and Aether borrow signatures for a
//! compiled module. Consumers can later preload this metadata without
//! recompiling the dependency from source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    aether::borrow_infer::BorrowSignature,
    bytecode::bytecode_cache::hash_bytes,
    cache_paths,
    core::CoreProgram,
    syntax::{Identifier, interner::Interner},
    types::{
        module_interface::{
            DependencyFingerprint, DependencyMissReason, MODULE_INTERFACE_FORMAT_VERSION,
            ModuleInterface,
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
}

/// Build a module interface from post-Aether Core plus cached HM schemes.
#[allow(clippy::too_many_arguments)]
pub fn build_interface(
    module_name: &str,
    module_sym: Identifier,
    source_hash: &[u8; 32],
    semantic_config_hash: &[u8; 32],
    program: &CoreProgram,
    schemes: &HashMap<(Identifier, Identifier), Scheme>,
    visibility: &HashMap<(Identifier, Identifier), bool>,
    dependency_fingerprints: Vec<DependencyFingerprint>,
    interner: &Interner,
) -> ModuleInterface {
    let mut interface = ModuleInterface::new(
        module_name,
        hex::encode(source_hash),
        hex::encode(semantic_config_hash),
    );
    interface.dependency_fingerprints = dependency_fingerprints;

    for def in &program.defs {
        if def.is_anonymous() || visibility.get(&(module_sym, def.name)) != Some(&true) {
            continue;
        }

        let name = interner.resolve(def.name).to_string();
        if let Some(scheme) = schemes.get(&(module_sym, def.name)) {
            interface.schemes.insert(name.clone(), scheme.clone());
        }
        if let Some(signature) = &def.borrow_signature {
            interface.borrow_signatures.insert(name, signature.clone());
        }
    }

    interface.interface_fingerprint = compute_interface_fingerprint(&interface);
    interface
}

pub fn compute_semantic_config_hash(strict_mode: bool, optimize_mode: bool) -> [u8; 32] {
    let marker = format!(
        "strict={}\noptimize={}\n",
        u8::from(strict_mode),
        u8::from(optimize_mode)
    );
    hash_bytes(marker.as_bytes())
}

pub fn compute_interface_fingerprint(interface: &ModuleInterface) -> String {
    let mut members: Vec<&str> = interface
        .schemes
        .keys()
        .map(String::as_str)
        .chain(interface.borrow_signatures.keys().map(String::as_str))
        .collect();
    members.sort_unstable();
    members.dedup();

    let exports: Vec<_> = members
        .into_iter()
        .map(|member| CanonicalExport {
            member,
            scheme: interface.schemes.get(member),
            borrow_signature: interface.borrow_signatures.get(member),
        })
        .collect();
    let bytes = serde_json::to_vec(&exports).expect("canonical interface fingerprint");
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
        return load_interface(&primary_path).ok_or(InterfaceLoadError::InvalidJson);
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

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        aether::borrow_infer::{BorrowMode, BorrowProvenance, BorrowSignature},
        core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreLit},
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
            &visibility,
            vec![DependencyFingerprint {
                module_name: "Flow.Prelude".to_string(),
                source_path: "lib/Flow/Prelude.flx".to_string(),
                interface_fingerprint: "abc".to_string(),
            }],
            &interner,
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

        let interface = ModuleInterface {
            module_name: "TestMod".to_string(),
            source_hash: "abc123".to_string(),
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            cache_format_version: MODULE_INTERFACE_FORMAT_VERSION,
            semantic_config_hash: "config".to_string(),
            interface_fingerprint: "abi".to_string(),
            schemes,
            borrow_signatures,
            dependency_fingerprints: vec![DependencyFingerprint {
                module_name: "Flow.List".to_string(),
                source_path: "lib/Flow/List.flx".to_string(),
                interface_fingerprint: "dep".to_string(),
            }],
        };

        let json = serde_json::to_string_pretty(&interface).unwrap();
        let loaded: ModuleInterface = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.module_name, "TestMod");
        assert_eq!(loaded.schemes.len(), 1);
        assert_eq!(loaded.borrow_signatures.len(), 1);
        assert!(loaded.schemes.contains_key("map"));
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
        let temp = std::env::temp_dir().join(format!(
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
        let temp = std::env::temp_dir().join(format!(
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
        let temp = std::env::temp_dir().join(format!(
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
