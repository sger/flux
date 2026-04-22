use crate::{core::FluxRep, syntax::symbol::Symbol};

pub struct ConstructorInfo {
    /// Owning ADT name. Populated by `AdtRegistry::register_adt` but not
    /// yet read; retained for the same representation-directed work that
    /// will eventually consume `field_reps`.
    #[allow(dead_code)]
    pub adt_name: Symbol,
    pub tag_idx: usize,
    pub arity: usize,
    /// Runtime representations for each field (Proposal 0123 Phase 7g).
    /// Enables type-directed optimizations like unboxed primitive fields.
    /// Currently populated but not yet consumed by backend lowering.
    #[allow(dead_code)]
    pub field_reps: Vec<FluxRep>,
}
