//! Parity validation subsystem (Proposal 0138).
//!
//! Runs the same Flux fixture under multiple execution "ways" (vm, llvm, etc.)
//! and compares normalized results to detect backend, cache, and mode drift.
//!
//! # Setup (one-time)
//!
//! ```bash
//! CARGO_TARGET_DIR=target/parity_vm cargo build
//! CARGO_TARGET_DIR=target/parity_native cargo build --features llvm
//! ```
//!
//! # Usage
//!
//! ```bash
//! cargo run -- parity-check tests/parity
//! cargo run -- parity-check examples/guide
//! cargo run -- parity-check examples/guide/fibonacci.flx
//! cargo run -- parity-check tests/parity --ways vm,llvm
//! cargo run -- parity-check tests/parity --ways vm,vm_cached
//! cargo run -- parity-check tests/parity --ways vm,vm_strict
//! cargo run -- parity-check tests/parity --ways llvm,llvm_strict
//! cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,llvm_cached,vm_strict,llvm_strict
//! cargo run -- parity-check tests/parity --capture-core
//! cargo run -- parity-check tests/parity --capture-aether
//! cargo run -- parity-check tests/parity --capture-repr
//! cargo run -- parity-check tests/parity --capture-core --capture-aether --capture-repr
//! cargo run -- parity-check examples/guide --root lib --root examples
//! cargo run -- parity-check tests/parity --ways vm,llvm
//! cargo run -- parity-check examples/guide --ways vm,llvm
//! cargo run -- parity-check examples/effects --ways vm,llvm
//! cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict
//! ```

pub mod cli;
pub mod fixture;
pub mod normalize;
pub mod report;
pub mod runner;

use std::fmt;
use std::hash::{Hash, Hasher};
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

    pub fn backend_id(self) -> BackendId {
        match self {
            Self::Vm | Self::VmCached | Self::VmStrict => BackendId::Vm,
            Self::Llvm | Self::LlvmCached | Self::LlvmStrict => BackendId::Llvm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendId {
    Vm,
    Llvm,
}

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vm => write!(f, "vm"),
            Self::Llvm => write!(f, "llvm"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceKind {
    Core,
    Aether,
    Repr,
    BackendIr(BackendId),
}

impl SurfaceKind {
    pub fn label(self) -> String {
        match self {
            Self::Core => "core".to_string(),
            Self::Aether => "aether".to_string(),
            Self::Repr => "repr".to_string(),
            Self::BackendIr(backend) => {
                let spec = backend_spec(backend);
                format!("{backend}:{}", spec.ir_surface)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackendSpec {
    pub id: BackendId,
    pub display_label: &'static str,
    pub ir_surface: &'static str,
    pub lowering_files: &'static [&'static str],
    pub runtime_files: &'static [&'static str],
}

pub fn backend_spec(id: BackendId) -> BackendSpec {
    match id {
        BackendId::Vm => BackendSpec {
            id,
            display_label: "CFG -> bytecode -> VM",
            ir_surface: "cfg",
            lowering_files: &["src/cfg/", "src/bytecode/compiler/"],
            runtime_files: &[
                "src/bytecode/vm/core_dispatch.rs",
                "src/bytecode/vm/dispatch.rs",
            ],
        },
        BackendId::Llvm => BackendSpec {
            id,
            display_label: "LIR -> LLVM -> native",
            ir_surface: "lir",
            lowering_files: &["src/lir/lower.rs", "src/lir/emit_llvm.rs"],
            runtime_files: &[
                "runtime/c/flux_rt.c",
                "runtime/c/array.c",
                "runtime/c/hamt.c",
            ],
        },
    }
}

#[derive(Debug, Clone, Default)]
pub struct SurfaceArtifact {
    pub raw: Option<String>,
    pub normalized: Option<String>,
    pub fingerprint: Option<String>,
}

impl SurfaceArtifact {
    pub fn from_raw(raw: String, normalized: String) -> Self {
        Self {
            raw: Some(raw),
            fingerprint: Some(stable_fingerprint(&normalized)),
            normalized: Some(normalized),
        }
    }
}

pub fn stable_fingerprint(text: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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
    /// `.fxm` — module bytecode cache
    Module,
    /// Native module object artifact
    NativeObject,
    /// Native metadata sidecar
    NativeMetadata,
    /// Shared native support object
    NativeSupport,
}

impl fmt::Display for CacheFileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytecode => write!(f, ".fxc"),
            Self::Interface => write!(f, ".flxi"),
            Self::Module => write!(f, ".fxm"),
            Self::NativeObject => write!(f, "native-object"),
            Self::NativeMetadata => write!(f, "native-metadata"),
            Self::NativeSupport => write!(f, "native-support"),
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
    pub core: SurfaceArtifact,
    pub aether: SurfaceArtifact,
    pub repr: SurfaceArtifact,
    pub backend_ir: Vec<(BackendId, SurfaceArtifact)>,
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
    ExpectedOutputMismatch { expected: String, actual: String },
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
    RepresentationMismatch {
        left_way: Way,
        left: String,
        right_way: Way,
        right: String,
    },
    BackendIrMismatch {
        baseline_way: Way,
        backend: BackendId,
        surface: String,
        summary: String,
    },
    BackendRuntimeMismatch {
        baseline_way: Way,
        backend: BackendId,
        summary: String,
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
    DiagnosticCodes {
        way: Way,
        expected: Vec<String>,
        actual: Vec<String>,
    },
    ExpectedStderr {
        way: Way,
        expected: String,
        actual: String,
    },
}
