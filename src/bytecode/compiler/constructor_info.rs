use crate::syntax::symbol::Symbol;

pub struct ConstructorInfo {
    pub adt_name: Symbol,
    pub tag_idx: usize,
    pub arity: usize,
}
