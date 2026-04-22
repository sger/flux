//! Builtin effect label registry (Proposal 0161, Phase 0 plumbing).
//!
//! Centralizes the interning of the well-known effect label names so call
//! sites that previously wrote `interner.intern("IO")` or
//! `interner.intern("Time")` all share one source of truth. This is the
//! mechanical prerequisite for later phases of 0161 (moving the labels into
//! `lib/Flow/Effects.flx`, decomposing `IO` into `Console`/`FileSystem`/…,
//! and driving the optimizer classification from the effect row).
//!
//! `Interner::intern` is idempotent — the helpers here do not cache a
//! `Symbol`; they delegate straight to the interner so a single registry
//! instance can be shared across the compiler without extra state.

use crate::syntax::{Identifier, interner::Interner};

/// Label name for the monolithic `IO` effect. Still one effect today; a
/// later 0161 phase replaces this with the decomposed set
/// (`Console`, `FileSystem`, `Stdin`).
pub const IO: &str = "IO";

/// Label name for the `Time` effect.
pub const TIME: &str = "Time";

/// Intern the `IO` effect label.
pub fn io_effect_symbol(interner: &mut Interner) -> Identifier {
    interner.intern(IO)
}

/// Intern the `Time` effect label.
pub fn time_effect_symbol(interner: &mut Interner) -> Identifier {
    interner.intern(TIME)
}

/// Look up the `IO` effect label without mutating the interner. Returns
/// `None` if the program has not yet interned it.
pub fn io_effect_symbol_opt(interner: &Interner) -> Option<Identifier> {
    interner.lookup(IO)
}

/// Look up the `Time` effect label without mutating the interner.
pub fn time_effect_symbol_opt(interner: &Interner) -> Option<Identifier> {
    interner.lookup(TIME)
}
