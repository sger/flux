//! The frontend half of generics: turning an AST statement list into the
//! binding groups the solver and Core lowering consume.
//!
//! This lives in the compiler crate rather than in `flux-generics` because it
//! walks the surface AST. That is the split GHC uses — dependency analysis is
//! the renamer's job, and the constraint solver never sees surface syntax. Only
//! the graph algorithm itself lives in the crate, as
//! [`flux_generics::strongly_connected_components`].
//!
//! See `docs/proposals/0186_generics_foundations.md`.

pub mod plan;

pub use plan::{PlanItem, group_index, plan_block};
