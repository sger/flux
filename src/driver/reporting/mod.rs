//! Driver-owned reporting surfaces.
//!
//! These modules format backend contracts, runtime analytics, trace banners, and runtime error
//! rendering used by the driver. They do not own compile semantics or backend execution.

pub(crate) mod report;
pub(crate) mod runtime_errors;
