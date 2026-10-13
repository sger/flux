//! Backend-neutral debug information (proposal 0188).
//!
//! `--debug` / `-g` selects a [`DebugLevel`]. When it is not `None`, the
//! compiler builds a [`ProgramDebugInfo`] for each module from its Core
//! program: the module's file, its functions and lambdas, their variables with
//! types, its data types and its effects. The VM engine (0188 S2) and the
//! native DWARF emitter (S5) read that model; `--dump-debug-info` prints it.

mod build;
mod model;
mod render;

pub use build::build_program_debug_info;
pub use model::{
    DebugCtor, DebugEffect, DebugField, DebugFile, DebugFnId, DebugFunction, DebugType,
    DebugTypeId, DebugVar, FileId, FnKind, ProgramDebugInfo, VarKind,
};
pub use render::render_program_debug_info;

/// How much debug information a compile produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DebugLevel {
    /// No debug information (the default).
    #[default]
    None,
    /// Locations only: functions, files and lines, without variables.
    Lines,
    /// Locations, variables and types.
    Full,
}

impl DebugLevel {
    /// Parses the value of `--debug=<level>`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "lines" => Some(Self::Lines),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// True when any debug information is produced.
    pub fn is_enabled(self) -> bool {
        self != Self::None
    }

    /// The spelling used on the command line and in cache markers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lines => "lines",
            Self::Full => "full",
        }
    }
}
