use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{aether::borrow_infer::BorrowSignature, types::scheme::Scheme};

/// Serializable compiled interface for a Flux module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInterface {
    /// Module name (for example `Base.List`).
    pub module_name: String,
    /// SHA-256 of the source file used to build this interface.
    pub source_hash: String,
    /// Compiler version used to produce this interface.
    pub compiler_version: String,
    /// Exported type schemes keyed by unqualified function name.
    pub schemes: HashMap<String, Scheme>,
    /// Exported borrow signatures keyed by unqualified function name.
    pub borrow_signatures: HashMap<String, BorrowSignature>,
}

impl ModuleInterface {
    pub fn new(module_name: impl Into<String>, source_hash: impl Into<String>) -> Self {
        Self {
            module_name: module_name.into(),
            source_hash: source_hash.into(),
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            schemes: HashMap::new(),
            borrow_signatures: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ModuleInterface;
    use crate::{
        aether::borrow_infer::{BorrowMode, BorrowProvenance, BorrowSignature},
        types::{
            infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme,
            type_constructor::TypeConstructor,
        },
    };

    #[test]
    fn module_interface_roundtrips_with_scheme_and_borrow_metadata() {
        let mut interface = ModuleInterface::new("Base.List", "deadbeef");
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

        let json = serde_json::to_string(&interface).expect("module interface should serialize");
        let decoded: ModuleInterface =
            serde_json::from_str(&json).expect("module interface should deserialize");

        assert_eq!(decoded, interface);
    }
}
