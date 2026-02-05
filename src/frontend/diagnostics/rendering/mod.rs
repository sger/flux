//! Rendering utilities for diagnostics
//!
//! This module contains all the rendering logic for displaying diagnostics,
//! including colors, formatting, source code snippets, and main orchestration.

pub mod colors;
pub mod formatter;
pub mod renderer;
pub mod source;

pub use colors::Colors;
pub use formatter::{render_diagnostics, render_display_path};
pub use renderer::{
    render_header, render_hints, render_location, render_message, render_related,
    render_suggestions,
};
pub use source::{get_source_line, render_hint_snippet, render_source_snippet};
