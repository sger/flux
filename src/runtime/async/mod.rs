//! Async runtime support (proposal 0174).
//!
//! Phase 0 lands the scheduler-owned [`context::EffectContext`] type
//! (slices 0b/0c/0d migrated the VM and native C runtime onto it) and the
//! [`backend::AsyncBackend`] trait + [`request_registry::RequestRegistry`]
//! that drive `Suspend → completion → resume` cycles (slice 0e). Phase 1a
//! layers the worker pool, the `mio` reactor backend, and the `Task<a>`
//! primitive on top; Phase 1b adds fibers and structured concurrency.

pub mod backend;
pub mod backends;
pub mod context;
pub mod request_registry;
pub mod runtime_target;
pub mod task_manager;

#[cfg(test)]
mod phase0_integration_tests;
