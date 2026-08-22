//! Per-test filesystem and cache isolation for tests that drive the `flux` CLI.
//!
//! ```rust
//! #[path = "../support/scratch.rs"]
//! mod scratch;
//! use scratch::Scratch;
//!
//! let s = Scratch::new("my_case");
//! let file = s.write("main.flx", SOURCE);
//! let out = Command::new(env!("CARGO_BIN_EXE_flux"))
//!     .args(["run", file.to_str().unwrap()])
//!     .args(s.cache_args())     // <- keeps this test off the shared cache
//!     .output();
//! ```
//!
//! # Why this exists
//!
//! Tests used to share `target/test-scratch/` *and*, more damagingly, the one
//! compilation cache at `target/flux`. `resolve_cache_root` walks up to the
//! nearest `Cargo.toml`, so every fixture written anywhere under the repo
//! resolves to the same cache root — concurrent test binaries then read and
//! wrote each other's `.flxi` interfaces and bytecode.
//!
//! That produced failures that moved between runs and looked unrelated: a
//! `missing global mapping for local index N` escaping from the module linker,
//! or a native fixture emitting nothing so the harness could not parse a
//! summary. Both are recorded as KI-010 in `docs/known_issues.md`.
//!
//! `Scratch` gives each test its own directory and its own cache root, so two
//! tests cannot collide however they are scheduled.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes scratch dirs created within one process. The pid separates
/// test binaries from each other; this separates tests inside a binary, which
/// `cargo test` runs on multiple threads by default.
static COUNTER: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// An isolated scratch directory plus a private compilation cache.
///
/// The directory is removed on drop. A test that fails leaves nothing behind
/// to confuse the next run — and because the name is unique per run, a leaked
/// directory from a killed process never collides either.
#[allow(dead_code)]
pub struct Scratch {
    dir: PathBuf,
}

#[allow(dead_code)]
impl Scratch {
    /// Create an isolated scratch directory. `label` only aids debugging; the
    /// uniqueness comes from the pid and counter.
    pub fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = workspace_root()
            .join("target")
            .join("test-scratch")
            .join(format!("{label}-{}-{n}", std::process::id()));
        // A previous run with this exact pid could in principle have leaked
        // this path; start from a known-empty state either way.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self { dir }
    }

    /// The scratch directory itself.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Write `contents` to `name` inside the scratch dir, returning its path.
    /// Parent directories are created, so `name` may contain separators.
    pub fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create scratch subdir");
        }
        std::fs::write(&path, contents).expect("write scratch file");
        path
    }

    /// Path to `name` inside the scratch dir, without creating anything.
    pub fn join(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// This test's private cache directory.
    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join(".flux-cache")
    }

    /// CLI arguments pinning `flux` to this test's private cache.
    ///
    /// Use these instead of `--no-cache` when the test *exercises* caching
    /// (a cold run then a warm one). `--no-cache` is still right when caching
    /// is irrelevant to what is being tested — it is cheaper, and both keep a
    /// test off the shared cache.
    pub fn cache_args(&self) -> Vec<String> {
        vec![
            "--cache-dir".to_string(),
            self.cache_dir().to_string_lossy().into_owned(),
        ]
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
