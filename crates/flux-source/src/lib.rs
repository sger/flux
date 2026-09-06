//! The vocabulary of source text: interned names, and positions in a file.
//!
//! Everything here answers one of two questions — *what was it called* and
//! *where was it written*. `Symbol` and `Interner` are the first; `Position`
//! and `Span` are the second.
//!
//! It is a crate rather than a module because `flux-generics` needs these
//! types and the compiler needs `flux-generics`; bundling them with anything
//! larger closes that loop.
//!
//! Nothing here knows what a type, an expression or a diagnostic is, and the
//! name is deliberately narrow to keep it that way. A dependency added here is
//! inherited by every crate downstream, so the bar for adding one is that
//! every consumer would want it.

pub mod entry;
pub mod interner;
pub mod position;
pub mod symbol;

pub use entry::Entry;
pub use interner::Interner;
pub use position::{Position, Span};
pub use symbol::Symbol;
