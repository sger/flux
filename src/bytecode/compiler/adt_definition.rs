use crate::syntax::symbol::Symbol;

pub struct AdtDefinition {
    pub name: Symbol,
    // (constructor_name, arity)
    pub constructors: Vec<(Symbol, usize)>,
}
