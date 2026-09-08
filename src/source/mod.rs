//! The vocabulary of source text: interned names, and positions in a file.
//!
//! Everything here answers one of two questions — *what was it called* and
//! *where was it written*. `Symbol` and `Interner` are the first; `Position`
//! and `Span` are the second.
//!
//! Nothing here knows what a type, an expression or a diagnostic is, and the
//! name is deliberately narrow to keep it that way. This is the bottom of the
//! module DAG: `diagnostics` and `syntax` both re-export from it, so anything
//! added here is inherited by everything downstream.

pub mod entry;
pub mod interner;
pub mod position;
pub mod symbol;

pub use entry::Entry;
pub use interner::Interner;
pub use position::{Position, Span};
pub use symbol::Symbol;
