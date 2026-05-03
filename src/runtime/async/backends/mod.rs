//! Backends for the async runtime (proposal 0174).
//!
//! Phase 0 ships an in-memory deterministic backend used to exercise the
//! `Suspend → completion → resume` round trip without bringing up `mio`.
//! Phase 1a adds [`mio`] as the production backend behind the same trait.

pub mod in_memory;
