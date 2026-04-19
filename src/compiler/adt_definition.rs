use crate::{core::FluxRep, syntax::symbol::Symbol};

pub struct AdtDefinition {
    /// (constructor_name, arity, field_reps)
    pub constructors: Vec<(Symbol, usize, Vec<FluxRep>)>,
}
