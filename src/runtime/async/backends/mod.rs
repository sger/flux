//! Async backend implementations.
//!
//! Phase 1a will put the production `mio` reactor behind the `async-mio`
//! feature. The module boundary exists now so scheduler code depends on the
//! `AsyncBackend` trait rather than on a concrete reactor.

#[cfg(feature = "async-mio")]
pub mod mio;
pub mod thread_timer;

#[cfg(feature = "async-mio")]
pub use mio::MioBackend;
pub use thread_timer::ThreadTimerBackend;
