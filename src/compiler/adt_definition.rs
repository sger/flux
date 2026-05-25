use crate::{core::FluxRep, syntax::symbol::Symbol};

#[derive(Clone)]
pub struct AdtDefinition {
    /// (constructor_name, arity, field_reps). Populated by
    /// `AdtRegistry::register_adt` but not yet read by backend lowering —
    /// tracked alongside `ConstructorInfo::field_reps` for a future
    /// representation-directed optimization pass.
    #[allow(dead_code)]
    pub constructors: Vec<(Symbol, usize, Vec<FluxRep>)>,
}
