//! Runtime-owned async/concurrency substrate.
//!
//! This module is the Rust-side target for Proposal 0174's scheduler and
//! backend work. Phase 0 only defines the context and record shapes that let
//! VM and native/LLVM paths meet the same runtime later; it deliberately does
//! not start worker threads, depend on `mio`, or change effect-handler
//! execution.

pub mod backend;
pub mod backends;
pub mod blocking;
pub mod bridge;
pub mod context;
pub mod driver;
pub mod scheduler;
pub mod worker;
