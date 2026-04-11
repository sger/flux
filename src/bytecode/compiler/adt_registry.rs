use std::collections::HashMap;

use crate::{
    bytecode::compiler::{adt_definition::AdtDefinition, constructor_info::ConstructorInfo},
    core::FluxRep,
    syntax::{data_variant::DataVariant, interner::Interner, symbol::Symbol},
};

pub struct AdtRegistry {
    pub constructors: HashMap<Symbol, ConstructorInfo>,
    pub adts: HashMap<Symbol, AdtDefinition>,
}

impl AdtRegistry {
    pub fn new() -> Self {
        Self {
            constructors: HashMap::new(),
            adts: HashMap::new(),
        }
    }

    pub fn register_adt(&mut self, name: Symbol, variants: &[DataVariant], interner: &Interner) {
        let mut constructor_list = Vec::new();

        for (idx, variant) in variants.iter().enumerate() {
            let arity = variant.fields.len();
            let field_reps: Vec<FluxRep> = variant
                .fields
                .iter()
                .map(|f| FluxRep::from_type_expr(f, interner))
                .collect();
            constructor_list.push((variant.name, arity, field_reps.clone()));
            self.constructors.insert(
                variant.name,
                ConstructorInfo {
                    adt_name: name,
                    tag_idx: idx,
                    arity,
                    field_reps,
                },
            );
        }

        self.adts.insert(
            name,
            AdtDefinition {
                constructors: constructor_list,
            },
        );
    }

    pub fn lookup_constructor(&self, name: Symbol) -> Option<&ConstructorInfo> {
        self.constructors.get(&name)
    }

    pub fn lookup_adt(&self, name: Symbol) -> Option<&AdtDefinition> {
        self.adts.get(&name)
    }
}

impl Default for AdtRegistry {
    fn default() -> Self {
        Self::new()
    }
}
