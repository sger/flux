//! Builder pattern module for diagnostics
//!
//! This module provides the DiagnosticBuilder trait that implements the fluent API
//! for constructing diagnostics. The trait is implemented by the Diagnostic struct
//! to provide a clean, organized interface for building complex diagnostic messages.

mod diagnostic_builder;

pub use diagnostic_builder::DiagnosticBuilder;
