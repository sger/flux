//! Rendering utilities for diagnostics
//!
//! This module contains all the rendering logic for displaying diagnostics,
//! including colors, formatting, source code snippets, and main orchestration.

pub mod colors;
pub mod formatter;
pub mod source;
pub mod renderer;

pub use colors::Colors;
pub use formatter::{render_diagnostics, render_display_path};
pub use source::{get_source_line, render_source_snippet, render_hint_snippet};
pub use renderer::{
    render_header,
    render_message,
    render_location,
    render_suggestions,
    render_hints,
    render_related,
};
