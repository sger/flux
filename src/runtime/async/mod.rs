//! Async runtime support (proposal 0174).
//!
//! Phase 0 lands the scheduler-owned [`context::EffectContext`] type that
//! Phase 0c/0d will migrate the VM and native C runtime onto. Phase 1a layers
//! the worker pool, the `mio` reactor backend, and the `Task<a>` primitive on
//! top; Phase 1b adds fibers and structured concurrency.

pub mod context;
