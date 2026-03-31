//! Module interface files (`.flxi`) for separate compilation.
//!
//! The interface stores exported HM schemes and Aether borrow signatures for a
//! compiled module. Consumers can later preload this metadata without
//! recompiling the dependency from source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{
    core::CoreProgram,
    syntax::{Identifier, interner::Interner},
    types::{module_interface::ModuleInterface, scheme::Scheme},
};

/// Build a module interface from post-Aether Core plus cached HM schemes.
pub fn build_interface(
    module_name: &str,
    module_sym: Identifier,
    source_hash: &[u8; 32],
    program: &CoreProgram,
    schemes: &HashMap<(Identifier, Identifier), Scheme>,
    visibility: &HashMap<(Identifier, Identifier), bool>,
    interner: &Interner,
) -> ModuleInterface {
    let mut interface = ModuleInterface::new(module_name, hex::encode(source_hash));

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

    interface
}

/// Compute the `.flxi` cache path for a module source file.
pub fn interface_path(source_path: &Path) -> PathBuf {
    source_path.with_extension("flxi")
}

/// Save a module interface to disk as JSON.
pub fn save_interface(path: &Path, interface: &ModuleInterface) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(interface).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Load a module interface from disk.
pub fn load_interface(path: &Path) -> Option<ModuleInterface> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Check whether a cached interface is still valid for the given source.
pub fn load_valid_interface(source_path: &Path, source: &str) -> Option<ModuleInterface> {
    let cache_path = interface_path(source_path);
    let interface = load_interface(&cache_path)?;

    if interface.compiler_version != env!("CARGO_PKG_VERSION") {
        return None;
    }

    let current_hash = hex::encode(&crate::bytecode::bytecode_cache::hash_bytes(
        source.as_bytes(),
    ));
    if interface.source_hash != current_hash {
        return None;
    }

    Some(interface)
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
            infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme,
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
        let interface = build_interface(
            interner.resolve(module),
            module,
            &hash,
            &program,
            &schemes,
            &visibility,
            &interner,
        );

        assert_eq!(interface.module_name, "Base.List");
        assert!(interface.schemes.contains_key("map"));
        assert!(interface.borrow_signatures.contains_key("map"));
        assert!(!interface.schemes.contains_key("helper"));
        assert!(!interface.borrow_signatures.contains_key("helper"));
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
            schemes,
            borrow_signatures,
        };

        let json = serde_json::to_string_pretty(&interface).unwrap();
        let loaded: ModuleInterface = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.module_name, "TestMod");
        assert_eq!(loaded.schemes.len(), 1);
        assert_eq!(loaded.borrow_signatures.len(), 1);
        assert!(loaded.schemes.contains_key("map"));
    }

    #[test]
    fn interface_path_from_source() {
        let path = interface_path(Path::new("examples/aoc/2024/Day07Solver.flx"));
        assert_eq!(path, PathBuf::from("examples/aoc/2024/Day07Solver.flxi"));
    }
}
