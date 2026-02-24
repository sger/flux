use crate::syntax::symbol::Symbol;

pub struct AdtDefinition {
    // (constructor_name, arity)
    pub constructors: Vec<(Symbol, usize)>,
}
