//! ANSI color codes for terminal output
//!
//! This module provides color codes that respect the NO_COLOR environment variable.

use std::env;

/// ANSI color codes for diagnostic rendering
pub struct Colors {
    pub red: &'static str,
    pub blue: &'static str,
    pub cyan: &'static str,
    pub green: &'static str,
    pub yellow: &'static str,
    pub reset: &'static str,
}

impl Colors {
    /// Get colors based on NO_COLOR environment variable
    pub fn new() -> Self {
        if env::var("NO_COLOR").is_ok() {
            Self::no_color()
        } else {
            Self::with_color()
        }
    }

    /// Get colored output (default)
    pub fn with_color() -> Self {
        Self {
            red: "\u{1b}[31m",
            blue: "\u{1b}[34m",
            cyan: "\u{1b}[36m",
            green: "\u{1b}[32m",
            yellow: "\u{1b}[33m",
            reset: "\u{1b}[0m",
        }
    }

    /// Get no-color output (when NO_COLOR is set)
    pub fn no_color() -> Self {
        Self {
            red: "",
            blue: "",
            cyan: "",
            green: "",
            yellow: "",
            reset: "",
        }
    }
}

impl Default for Colors {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if colors should be disabled
pub fn colors_disabled() -> bool {
    env::var("NO_COLOR").is_ok()
}
