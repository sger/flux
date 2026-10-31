//! Per-test filesystem and cache isolation for tests that drive the `flux` CLI.
//!
//! ```rust
//! use crate::support::scratch;
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

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
        self.dir.join("flux-cache")
    }

    /// CLI arguments pinning `flux` to this test's private cache, empty.
    ///
    /// Use these when the test is about the cache itself: a cold run then a
    /// warm one, or what a cache miss rebuilds. Other tests want
    /// [`Scratch::seeded_cache_args`], which skips compiling the stdlib.
    pub fn cache_args(&self) -> Vec<String> {
        cache_args_for(&self.cache_dir())
    }

    /// [`Scratch::cache_args`], with the private cache first filled from a
    /// stdlib cache built once per `flux` build. Without it every run
    /// compiles all of `lib/Flow` before the program, which made that the
    /// larger part of most native tests. The cache stays private: the test
    /// gets a copy, and the template is never written to.
    pub fn seeded_cache_args(&self) -> Vec<String> {
        self.seed(&["interfaces", "vm", "native"])
    }

    /// [`Scratch::seeded_cache_args`] for a VM run, which reads none of the
    /// native objects that make up most of the template.
    pub fn seeded_vm_cache_args(&self) -> Vec<String> {
        self.seed(&["interfaces", "vm"])
    }

    fn seed(&self, parts: &[&str]) -> Vec<String> {
        let cache = self.cache_dir();
        if !cache.exists()
            && let Some(template) = stdlib_template()
        {
            for part in parts {
                // A partial copy only means the test compiles more itself.
                let _ = copy_tree(&template.join(part), &cache.join(part));
            }
        }
        self.cache_args()
    }
}

/// The stdlib cache [`Scratch::seeded_cache_args`] copies from, or `None`
/// when it could not be built. nextest runs every test in its own process,
/// so the first process to need it builds it under a lock file and the
/// others wait for the finished directory.
fn stdlib_template() -> Option<&'static Path> {
    static TEMPLATE: OnceLock<Option<PathBuf>> = OnceLock::new();
    TEMPLATE.get_or_init(build_or_wait_for_template).as_deref()
}

fn build_or_wait_for_template() -> Option<PathBuf> {
    let root = workspace_root().join("target").join("test-stdlib-cache");
    std::fs::create_dir_all(&root).ok()?;
    let key = template_key();
    let dir = root.join(&key);
    let lock = root.join(format!("{key}.lock"));
    loop {
        if dir.is_dir() {
            return Some(dir);
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut file) => {
                let _ = writeln!(file, "pid={}", std::process::id());
                let _lock = RemoveOnDrop(lock);
                if !dir.is_dir() {
                    build_template(&root, &key);
                }
                return dir.is_dir().then_some(dir);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // The same recovery as the runtime's build lock: a killed
                // holder must not stall every later test.
                let stale = std::fs::metadata(&lock)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(5 * 60));
                if stale {
                    let _ = std::fs::remove_file(&lock);
                    continue;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

/// Names the template after the `flux` binary and the stdlib sources. Cache
/// entries are keyed by build already, but `flux_support_O0.o` is not, so a
/// template from an older build could hand a test a stale object.
fn template_key() -> String {
    let mut hasher = std::hash::DefaultHasher::new();
    let mut stamp = |path: &Path| {
        if let Ok(metadata) = std::fs::metadata(path) {
            path.hash(&mut hasher);
            metadata.len().hash(&mut hasher);
            metadata.modified().ok().hash(&mut hasher);
        }
    };
    stamp(Path::new(env!("CARGO_BIN_EXE_flux")));
    let mut sources = Vec::new();
    files_under(&workspace_root().join("lib"), &mut sources);
    sources.sort();
    for source in &sources {
        stamp(source);
    }
    format!("{:016x}", hasher.finish())
}

fn files_under(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// Fills `<root>/<key>` by running a program that imports every `Flow`
/// module, on the VM and in each native build the tests use. A run that
/// fails leaves out only what it would have added.
fn build_template(root: &Path, key: &str) {
    let work = root.join(format!("{key}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    if std::fs::create_dir_all(&work).is_err() {
        return;
    }
    let program = work.join("warm.flx");
    if std::fs::write(&program, warm_program()).is_err() {
        return;
    }
    let cache = work.join("cache");
    // `--debug-heap` and `-g` compile the stdlib apart from a plain native
    // build; the debug tests use both.
    let variants: &[&[&str]] = if cfg!(feature = "llvm") {
        &[&[], &["--native"], &["--native", "--debug-heap"], &["--native", "-g"]]
    } else {
        &[&[]]
    };
    for variant in variants {
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .arg(&program)
            .args(*variant)
            .args(cache_args_for(&cache))
            .env("NO_COLOR", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    remove_warm_program_entries(&cache);
    if cache.is_dir() && std::fs::rename(&cache, root.join(key)).is_ok() {
        prune_other_templates(root, key);
    }
    let _ = std::fs::remove_dir_all(&work);
}

/// Removes what the warm program left about itself, which no test reads,
/// and the native build's scratch space.
fn remove_warm_program_entries(cache: &Path) {
    let native = cache.join("native");
    let _ = std::fs::remove_dir_all(native.join("scratch"));
    let _ = std::fs::remove_dir_all(native.join("program_hash"));
    for part in ["interfaces", "vm", "native"] {
        for entry in std::fs::read_dir(cache.join(part)).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with("warm") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// `import`s for every module in `lib/Flow` except `Primops`, which programs
/// may not import, and `Effects`, which is documentation.
fn warm_program() -> String {
    let mut modules: Vec<String> = std::fs::read_dir(workspace_root().join("lib").join("Flow"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.path().file_stem()?.to_str().map(str::to_owned))
        .filter(|module| module != "Primops" && module != "Effects")
        .collect();
    modules.sort();
    let mut program: String =
        modules.iter().map(|module| format!("import Flow.{module} as M{module}\n")).collect();
    program.push_str("\nfn main() with IO {\n    print(1)\n}\n");
    program
}

/// Removes the templates of earlier builds. Best effort: one still being
/// copied from only leaves its test to compile more itself.
fn prune_other_templates(root: &Path, key: &str) {
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(key) && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Copies `from` into `to`, keeping each file's mtime: the stdlib archive is
/// trusted only while it is no older than its members.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
            let modified = entry.metadata()?.modified()?;
            std::fs::File::options().write(true).open(&target)?.set_modified(modified)?;
        }
    }
    Ok(())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Build cache arguments for a caller that already has an isolated cache
/// path. Keeping the argument construction here prevents integration tests
/// from independently inventing legacy `.flux-cache` locations.
#[allow(dead_code)]
pub fn cache_args_for(cache_dir: &Path) -> Vec<String> {
    vec!["--cache-dir".to_string(), cache_dir.to_string_lossy().into_owned()]
}

/// The ports `free_port` picks from. They lie below every OS's ephemeral
/// range (Linux from 32768, Windows and macOS from 49152), the range outgoing
/// connections take their local port from. A port the OS chose with port 0
/// came from that range, and another test's connection could take it before
/// the program bound it (KI-141).
const TEST_PORTS: std::ops::Range<u16> = 20000..30000;

static NEXT_PORT: AtomicU64 = AtomicU64::new(0);

/// A loopback port nothing listens on right now, from `TEST_PORTS`. Each
/// process starts at its own offset: nextest runs each test in its own
/// process, so a counter from a fixed base gave every test the same port.
/// Ports that do not bind (in use, or excluded, as Windows does for Hyper-V)
/// are skipped.
#[allow(dead_code)]
pub fn free_port() -> u16 {
    let span = u64::from(TEST_PORTS.end - TEST_PORTS.start);
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let start = u64::from(std::process::id()).wrapping_mul(7919) ^ u64::from(clock);
    for _ in 0..span {
        let step = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        let port = TEST_PORTS.start + ((start.wrapping_add(step)) % span) as u16;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free loopback port in {TEST_PORTS:?}");
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
