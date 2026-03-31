//! Parity validation subsystem (Proposal 0138).
//!
//! Runs the same Flux fixture under multiple execution "ways" (vm, llvm, etc.)
//! and compares normalized results to detect backend, cache, and mode drift.

pub mod cli;
pub mod fixture;
pub mod normalize;
pub mod report;
pub mod runner;

use std::fmt;
use std::path::PathBuf;

// ── Exit classification ────────────────────────────────────────────────────

/// How a single way's execution terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    Success,
    CompileError,
    RuntimeError,
    Timeout,
    ToolFailure,
}

impl fmt::Display for ExitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "Success"),
            Self::CompileError => write!(f, "CompileError"),
            Self::RuntimeError => write!(f, "RuntimeError"),
            Self::Timeout => write!(f, "Timeout"),
            Self::ToolFailure => write!(f, "ToolFailure"),
        }
    }
}

// ── Ways ───────────────────────────────────────────────────────────────────

/// A named execution configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Way {
    Vm,
    Llvm,
    VmCached,
    LlvmCached,
    VmStrict,
    LlvmStrict,
}

impl fmt::Display for Way {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vm => write!(f, "vm"),
            Self::Llvm => write!(f, "llvm"),
            Self::VmCached => write!(f, "vm_cached"),
            Self::LlvmCached => write!(f, "llvm_cached"),
            Self::VmStrict => write!(f, "vm_strict"),
            Self::LlvmStrict => write!(f, "llvm_strict"),
        }
    }
}

impl Way {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "vm" => Some(Self::Vm),
            "llvm" => Some(Self::Llvm),
            "vm_cached" => Some(Self::VmCached),
            "llvm_cached" => Some(Self::LlvmCached),
            "vm_strict" => Some(Self::VmStrict),
            "llvm_strict" => Some(Self::LlvmStrict),
            _ => None,
        }
    }

    /// The base (fresh) way for a cached variant.
    pub fn base_way(self) -> Self {
        match self {
            Self::VmCached => Self::Vm,
            Self::LlvmCached => Self::Llvm,
            other => other,
        }
    }

    /// Whether this way uses the bytecode/module cache.
    pub fn is_cached(self) -> bool {
        matches!(self, Self::VmCached | Self::LlvmCached)
    }

    /// Whether this way enables `--strict` mode.
    pub fn is_strict(self) -> bool {
        matches!(self, Self::VmStrict | Self::LlvmStrict)
    }

    /// The non-strict counterpart of a strict way.
    pub fn non_strict(self) -> Self {
        match self {
            Self::VmStrict => Self::Vm,
            Self::LlvmStrict => Self::Llvm,
            other => other,
        }
    }
}

// ── Run result ─────────────────────────────────────────────────────────────

/// Captured output from running a single fixture under a single way.
#[derive(Debug)]
pub struct RunResult {
    pub way: Way,
    pub exit_kind: ExitKind,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub normalized_stdout: String,
    pub normalized_stderr: String,
    /// Cache files observed after execution (for cached ways).
    pub cache_observations: Vec<CacheObservation>,
}

/// A cache file that was created or found during execution.
#[derive(Debug, Clone)]
pub struct CacheObservation {
    pub path: PathBuf,
    pub kind: CacheFileKind,
    pub state: CacheFileState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFileKind {
    /// `.fxc` — top-level bytecode cache
    Bytecode,
    /// `.flxi` — semantic interface cache
    Interface,
}

impl fmt::Display for CacheFileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytecode => write!(f, ".fxc"),
            Self::Interface => write!(f, ".flxi"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFileState {
    /// File was created by this run.
    Created,
    /// File existed before this run (cache hit).
    Existed,
}

/// Optional debug artifacts captured alongside a run.
#[derive(Debug, Default)]
pub struct DebugArtifacts {
    pub dump_core: Option<String>,
    pub normalized_dump_core: Option<String>,
    pub dump_aether: Option<String>,
    pub normalized_dump_aether: Option<String>,
}

// ── Parity verdict ─────────────────────────────────────────────────────────

/// Comparison outcome for a single fixture across all requested ways.
#[derive(Debug)]
pub struct ParityResult {
    pub file: PathBuf,
    pub results: Vec<RunResult>,
    pub artifacts: Vec<(Way, DebugArtifacts)>,
    pub verdict: Verdict,
}

#[derive(Debug)]
pub enum Verdict {
    Pass,
    Mismatch { details: Vec<MismatchDetail> },
    Skip { reason: String },
}

#[derive(Debug)]
pub enum MismatchDetail {
    ExitKind {
        left_way: Way,
        left: ExitKind,
        right_way: Way,
        right: ExitKind,
    },
    Stdout {
        left_way: Way,
        left: String,
        right_way: Way,
        right: String,
    },
    Stderr {
        left_way: Way,
        left: String,
        right_way: Way,
        right: String,
    },
    CoreMismatch {
        left_way: Way,
        left: String,
        right_way: Way,
        right: String,
    },
    AetherMismatch {
        left_way: Way,
        left: String,
        right_way: Way,
        right: String,
    },
    CacheMismatch {
        fresh_way: Way,
        cached_way: Way,
        field: String,
        fresh: String,
        cached: String,
    },
    StrictModeMismatch {
        normal_way: Way,
        strict_way: Way,
        field: String,
        normal: String,
        strict: String,
    },
}
