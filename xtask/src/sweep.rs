//! Compile every Flux file in the corpus, without running any of them, and
//! record how each one ended. Two sweeps of the same corpus, say `main` and a
//! branch, are compared with `compare`, which is how "no file newly
//! rejected" is measured: a test or parity fixture only covers a file someone
//! wrote one for.
//!
//! Modes (each type-checks the file and lowers it, then exits):
//! - `cfg`: `--dump-cfg`, the VM path
//! - `core`: `--dump-core`, the Core path, as dumps and LLVM see it
//! - `core-opt`: `--optimize --dump-core`, the same with AST optimizations
//! - `aether`: `--dump-aether`, ownership: where values are duplicated,
//!   dropped and reused
//! - `lir`: `--native --dump-lir`, the native backend's input; needs a flux
//!   built with the `llvm` feature, as verify's is
//!
//! Every run uses `--no-cache`, so no cached module hides a change.
//!
//! Writes to the out dir:
//! - `runs/<file>.txt`: each file's output, with the dump's last 40 lines
//! - `results.tsv`: file, status, exit code, diagnostic codes, milliseconds
//! - `hashes.tsv`: the mode, then each file's whole dump as an FNV-1a hash,
//!   its bytes and its lines. Dumps themselves are not kept: a CFG dump is
//!   about 2.7 MB, most of it the prelude. `compare --content` reruns both
//!   sweeps' compilers on a file whose hash changed to show the difference.
//! - `flux`: the compiler the sweep ran, kept for that
//! - `summary.txt`: how many files ended each way
//! - `errors.txt`: the files that did not end `ok`
//!
//! Statuses: ok, error (diagnostics, none internal), ice (an internal compiler
//! error), panic (the compiler panicked), timeout, crash (any other failure).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::text::{diagnostic_codes, strip_ansi};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Cfg,
    Core,
    CoreOpt,
    Aether,
    Lir,
}

impl Mode {
    pub fn parse(name: &str) -> Option<Mode> {
        match name {
            "cfg" => Some(Mode::Cfg),
            "core" => Some(Mode::Core),
            "core-opt" => Some(Mode::CoreOpt),
            "aether" => Some(Mode::Aether),
            "lir" => Some(Mode::Lir),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Cfg => "cfg",
            Mode::Core => "core",
            Mode::CoreOpt => "core-opt",
            Mode::Aether => "aether",
            Mode::Lir => "lir",
        }
    }

    pub(crate) fn flags(self) -> &'static [&'static str] {
        match self {
            Mode::Cfg => &["--dump-cfg"],
            Mode::Core => &["--dump-core"],
            Mode::CoreOpt => &["--optimize", "--dump-core"],
            Mode::Aether => &["--dump-aether"],
            Mode::Lir => &["--native", "--dump-lir"],
        }
    }
}

#[derive(Clone)]
pub struct Options {
    pub out_dir: PathBuf,
    pub mode: Mode,
    pub files: Option<PathBuf>,
    pub jobs: usize,
    pub timeout: Duration,
    pub flux: PathBuf,
}

pub fn default_jobs() -> usize {
    thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2))
        .unwrap_or(1)
        .max(1)
}

#[derive(Debug, Clone)]
pub struct FileResult {
    pub file: String,
    pub status: &'static str,
    pub exit: i32,
    pub codes: String,
    pub ms: u128,
    pub dump: DumpHash,
}

/// A whole dump, summarized: equal hashes mean equal dumps for any purpose
/// a comparison has, and the sizes say how big a difference is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DumpHash {
    pub fnv: u64,
    pub bytes: usize,
    pub lines: usize,
    /// The hash of the dump with its numbering taken out ([`normalize`]):
    /// equal when two dumps differ only in how they number or order the
    /// same program. `None` in a sweep made before 0190 S6.
    pub normalized: Option<u64>,
}

impl DumpHash {
    pub fn of(text: &str) -> Self {
        Self {
            fnv: fnv64(text.as_bytes()),
            bytes: text.len(),
            lines: text.lines().count(),
            normalized: Some(fnv64(normalize(text).as_bytes())),
        }
    }

    /// Whether the two dumps are the same text. The normalized hash is left
    /// out, so a sweep made before it existed still compares.
    pub fn same_dump(&self, other: &Self) -> bool {
        (self.fnv, self.bytes, self.lines) == (other.fnv, other.bytes, other.lines)
    }
}

/// `text` with every compiler-chosen number replaced by `_`: the digits after
/// `#`, `%`, `%t` or `<sym:`, and after `v`, `b`, `bb` or `fn` when that
/// prefix starts a word. These name values, blocks, functions and binders in
/// the dumps (`v12`, `b3`, `bb0`, `fn36`, `#243`, `%t530`, `%2`, `xs#1250`,
/// `<sym:4001814>`). Literals (`Const(3)`) and identifiers (`sub2`, `__arg0`)
/// keep their digits, so a changed program still hashes differently.
pub fn normalize(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        if chars[at].is_ascii_digit() && is_numbering(&chars[..at]) {
            out.push('_');
            while chars.get(at).is_some_and(char::is_ascii_digit) {
                at += 1;
            }
        } else {
            out.push(chars[at]);
            at += 1;
        }
    }
    out
}

/// Whether a run of digits right after `before` is a compiler-chosen number.
fn is_numbering(before: &[char]) -> bool {
    let ends_with = |suffix: &str| {
        let suffix: Vec<char> = suffix.chars().collect();
        before.ends_with(&suffix)
    };
    if before.last().is_some_and(char::is_ascii_digit) {
        return false;
    }
    if ["#", "%", "%t", "<sym:"].into_iter().any(ends_with) {
        return true;
    }
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    ["bb", "fn", "v", "b"].into_iter().any(|prefix| {
        let len = prefix.chars().count();
        ends_with(prefix)
            && before
                .len()
                .checked_sub(len + 1)
                .is_none_or(|start| !is_word(&before[start]))
    })
}

/// FNV-1a, 64-bit. Written out rather than taken from `std`, whose
/// `DefaultHasher` may change between Rust releases, and a sweep is compared
/// with sweeps made by other builds.
pub fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Where a sweep keeps the compiler it ran.
pub fn compiler_in(out_dir: &Path) -> PathBuf {
    out_dir.join(format!("flux{}", std::env::consts::EXE_SUFFIX))
}

/// What one compiler run printed.
pub(crate) struct Ran {
    pub finished: bool,
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `flux --no-cache --quiet <flags> <file>` from `root`, as a sweep
/// does, killing it after `timeout`.
pub(crate) fn run_flux(
    root: &Path,
    flux: &Path,
    flags: &[&str],
    file: &str,
    timeout: Duration,
) -> Ran {
    let mut command = Command::new(flux);
    command
        .args(["--no-cache", "--quiet"])
        .args(flags)
        .arg(file);
    run_with_timeout(command, root, timeout)
}

/// Runs `command` from `root` without colour or stdin, killing it after
/// `timeout`.
fn run_with_timeout(mut command: Command, root: &Path, timeout: Duration) -> Ran {
    let spawned = command
        .current_dir(root)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let (finished, exit, stdout, stderr) = match spawned {
        Ok(child) => wait_with_timeout(child, timeout),
        Err(e) => (true, -1, String::new(), format!("failed to start: {e}")),
    };
    Ran {
        finished,
        exit,
        stdout,
        stderr,
    }
}

/// How a run ended. `text` is its stderr without colour.
pub fn classify(finished: bool, exit: i32, text: &str, codes: &[String]) -> &'static str {
    let lower = text.to_lowercase();
    if !finished {
        "timeout"
    } else if lower.contains("internal compiler error") {
        "ice"
    } else if lower.contains("panicked at") || lower.lines().any(overflowed_stack) {
        "panic"
    } else if exit == 0 {
        "ok"
    } else if !codes.is_empty() {
        "error"
    } else {
        "crash"
    }
}

/// `thread '<name>' has overflowed its stack`.
fn overflowed_stack(line: &str) -> bool {
    line.find("thread '")
        .is_some_and(|at| line[at + "thread '".len()..].contains("' has overflowed its stack"))
}

/// Every `.flx` file under `examples`, `tests` and `lib`, or the files listed
/// in `list`; relative to `root`, with `/` separators, sorted and unique.
fn corpus(root: &Path, list: Option<&Path>) -> io::Result<Vec<String>> {
    let mut files = match list {
        Some(list) => fs::read_to_string(list)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| line.replace('\\', "/"))
            .collect(),
        None => {
            let mut files = Vec::new();
            for dir in ["examples", "tests", "lib"] {
                collect_flx(root, &root.join(dir), &mut files)?;
            }
            files
        }
    };
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_flx(root: &Path, dir: &Path, files: &mut Vec<String>) -> io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_flx(root, &path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "flx") {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

pub fn run(root: &Path, options: &Options) -> Result<Vec<FileResult>, String> {
    if !options.flux.is_file() {
        return Err(format!(
            "no compiler at {}; run cargo build first",
            options.flux.display()
        ));
    }
    let files = corpus(root, options.files.as_deref()).map_err(|e| e.to_string())?;

    let out_dir = &options.out_dir;
    if out_dir.exists() {
        fs::remove_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    }
    let runs = out_dir.join("runs");
    fs::create_dir_all(&runs).map_err(|e| e.to_string())?;

    // Run a copy: a running compiler locks its file on Windows, so a build
    // during the sweep would fail, and a rebuild would change the compiler
    // halfway.
    let flux = compiler_in(out_dir);
    fs::copy(&options.flux, &flux).map_err(|e| format!("copying the compiler: {e}"))?;

    let flags = options.mode.flags();
    println!(
        "sweep: {} files, mode {}, {} jobs; each runs\n  $ flux --no-cache --quiet {} <file>\n  out: {}",
        files.len(),
        options.mode.name(),
        options.jobs,
        flags.join(" "),
        out_dir.display()
    );
    let started = Instant::now();

    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(files.len()));
    thread::scope(|scope| {
        for _ in 0..options.jobs.max(1) {
            scope.spawn(|| {
                while let Some(file) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let result = sweep_file(root, &flux, flags, &runs, file, options.timeout);
                    results.lock().unwrap().push(result);
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if finished.is_multiple_of(25) || finished == files.len() {
                        eprint!("\r   {finished} / {} files", files.len());
                        let _ = io::stderr().flush();
                    }
                }
            });
        }
    });
    eprintln!();

    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.file.cmp(&b.file));
    write_outputs(out_dir, options.mode, &results, started.elapsed()).map_err(|e| e.to_string())?;
    Ok(results)
}

/// Sweeps the corpus twice with one compiler, into `<out>_a` and `<out>_b`,
/// and compares the two by content. Any difference means a dump depends on
/// something other than its input, such as `HashMap` order, and makes
/// `--cmp` unreliable. Returns whether the two agreed.
pub fn determinism(root: &Path, options: &Options) -> Result<bool, String> {
    let with_suffix = |suffix: &str| {
        let mut name = options.out_dir.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let (a, b) = (with_suffix("_a"), with_suffix("_b"));
    let mut files = 0;
    for dir in [&a, &b] {
        let results = run(
            root,
            &Options {
                out_dir: dir.clone(),
                ..options.clone()
            },
        )?;
        files = results.len();
    }
    let comparison = crate::compare::run(root, &a, &b, true)?;
    let deterministic = !comparison.failed && comparison.changed.is_empty();
    if deterministic {
        println!("deterministic: {files} files, mode {}", options.mode.name());
    } else {
        println!(
            "NOT deterministic: {} dump(s) differ between two runs of the same compiler; see {}",
            comparison.changed.len(),
            crate::compare::Comparison::diff_file(&b).display()
        );
    }
    Ok(deterministic)
}

/// The files `--mode cache` runs when no `--files` list is given: parity
/// fixtures are meant to run, and quickly, without the network.
const CACHE_CORPUS: &str = "tests/parity";

/// `sweep --determinism --mode cache`: runs each program twice, each time
/// with an empty `--cache-dir` and `FLUX_HOME` at the same path, and compares
/// the files the two runs wrote, byte for byte. A dump cannot show this: any
/// `--dump-*` flag turns the VM cache off. `options.mode` is not used.
/// Returns whether every file's two caches agreed.
pub fn cache_determinism(root: &Path, options: &Options) -> Result<bool, String> {
    if !options.flux.is_file() {
        return Err(format!(
            "no compiler at {}; run cargo build first",
            options.flux.display()
        ));
    }
    let files = if let Some(list) = &options.files {
        corpus(root, Some(list)).map_err(|e| e.to_string())?
    } else {
        let mut files = Vec::new();
        collect_flx(root, &root.join(CACHE_CORPUS), &mut files).map_err(|e| e.to_string())?;
        files.sort();
        files
    };
    let out_dir = &options.out_dir;
    if out_dir.exists() {
        fs::remove_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    }
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let flux = compiler_in(out_dir);
    fs::copy(&options.flux, &flux).map_err(|e| format!("copying the compiler: {e}"))?;
    println!(
        "cache determinism: {} files, {} jobs; each runs twice\n  $ flux --quiet --cache-dir <empty> <file>   (FLUX_HOME=<empty>)\n  out: {}",
        files.len(),
        options.jobs,
        out_dir.display()
    );

    let next = AtomicUsize::new(0);
    let differences = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..options.jobs.max(1) {
            scope.spawn(|| {
                while let Some(file) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let found = cache_twice(root, &flux, out_dir, file, options.timeout);
                    differences.lock().unwrap().extend(found);
                }
            });
        }
    });
    let mut differences = differences.into_inner().unwrap();
    differences.sort();
    let mut report = differences.join("\n");
    report.push('\n');
    if differences.is_empty() {
        report = "deterministic\n".to_string();
    }
    let report_file = out_dir.join("cache_diff.txt");
    fs::write(&report_file, report).map_err(|e| format!("{}: {e}", report_file.display()))?;
    if differences.is_empty() {
        println!("deterministic: {} files, mode cache", files.len());
    } else {
        for line in differences.iter().take(20) {
            println!("  {line}");
        }
        println!(
            "NOT deterministic: {} cache file(s) differ between two runs; see {}",
            differences.len(),
            report_file.display()
        );
    }
    Ok(differences.is_empty())
}

/// Runs `file` twice in one work folder, keeping each run's cache under
/// `a/` and `b/`, and returns how the two differ, one line per cache file.
/// The same folder for both runs keeps any path a cache records the same.
fn cache_twice(
    root: &Path,
    flux: &Path,
    out_dir: &Path,
    file: &str,
    timeout: Duration,
) -> Vec<String> {
    let name = file.replace(['\\', '/', ':'], "__");
    let work = out_dir.join("work").join(&name);
    let mut kept = Vec::new();
    for side in ["a", "b"] {
        let _ = fs::remove_dir_all(&work);
        if let Err(e) = fs::create_dir_all(work.join("home")) {
            return vec![format!("{file}: {e}")];
        }
        let mut command = Command::new(flux);
        command
            .arg("--quiet")
            .arg("--cache-dir")
            .arg(work.join("cache"))
            .arg(file)
            .env("FLUX_HOME", work.join("home"));
        let ran = run_with_timeout(command, root, timeout);
        if !ran.finished {
            return vec![format!("{file}: timed out")];
        }
        let target = out_dir.join(side).join(&name);
        let moved =
            fs::create_dir_all(out_dir.join(side)).and_then(|()| fs::rename(&work, &target));
        if let Err(e) = moved {
            return vec![format!("{file}: keeping run {side}: {e}")];
        }
        kept.push(target);
    }
    tree_differences(&kept[0], &kept[1])
        .into_iter()
        .map(|difference| format!("{file}: {difference}"))
        .collect()
}

/// How the files under `a` and `b` differ: `<path> differs`, `<path> only
/// in a` or `<path> only in b`, by path relative to each, in path order.
fn tree_differences(a: &Path, b: &Path) -> Vec<String> {
    let (files_a, files_b) = (tree(a), tree(b));
    let mut paths: Vec<&PathBuf> = files_a.keys().chain(files_b.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let shown = path.to_string_lossy().replace('\\', "/");
            match (files_a.get(path), files_b.get(path)) {
                (Some(x), Some(y)) if x == y => None,
                (Some(_), Some(_)) => Some(format!("{shown} differs")),
                (Some(_), None) => Some(format!("{shown} only in a")),
                (None, _) => Some(format!("{shown} only in b")),
            }
        })
        .collect()
}

/// Every file under `dir`, by path relative to it, with its bytes.
fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, files);
            } else if let Ok(bytes) = fs::read(&path) {
                let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
                files.insert(relative, bytes);
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

/// The `n` files of a sweep its determinism check reruns: those `ok`, in
/// the order of their path's hash, so a run of the same corpus picks the
/// same ones every time and the pick does not follow the folder layout.
pub fn determinism_sample(results: &[FileResult], n: usize) -> Vec<&FileResult> {
    let mut ok: Vec<&FileResult> = results.iter().filter(|r| r.status == "ok").collect();
    ok.sort_by_key(|r| (fnv64(r.file.as_bytes()), &r.file));
    ok.truncate(n);
    ok
}

/// Reruns `sample` with the compiler the sweep in `out_dir` kept, `jobs` at
/// a time, and returns each file whose dump is not the one the sweep
/// hashed, with both hashes.
pub fn recheck(
    root: &Path,
    out_dir: &Path,
    mode: Mode,
    sample: &[&FileResult],
    jobs: usize,
    timeout: Duration,
) -> Vec<(String, DumpHash, DumpHash)> {
    let flux = compiler_in(out_dir);
    let next = AtomicUsize::new(0);
    let differing = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, sample.len().max(1)) {
            scope.spawn(|| {
                while let Some(first) = sample.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let again = DumpHash::of(
                        &run_flux(root, &flux, mode.flags(), &first.file, timeout).stdout,
                    );
                    if !again.same_dump(&first.dump) {
                        let entry = (first.file.clone(), first.dump, again);
                        differing.lock().unwrap().push(entry);
                    }
                }
            });
        }
    });
    let mut differing = differing.into_inner().unwrap();
    differing.sort_by(|a, b| a.0.cmp(&b.0));
    differing
}

fn sweep_file(
    root: &Path,
    flux: &Path,
    flags: &[&str],
    runs: &Path,
    file: &str,
    timeout: Duration,
) -> FileResult {
    let clock = Instant::now();
    let Ran {
        finished,
        exit,
        stdout,
        stderr,
    } = run_flux(root, flux, flags, file, timeout);
    let ms = clock.elapsed().as_millis();

    // The dump goes to stdout and is long; diagnostics go to stderr. Keep the
    // dump's tail only, so a run's file stays readable.
    let text = strip_ansi(&stderr);
    let lines: Vec<&str> = stdout.split('\n').collect();
    let out_tail = lines[lines.len().saturating_sub(40)..].join("\n");
    let codes = diagnostic_codes(&text);
    let status = classify(finished, exit, &text, &codes);
    let codes = codes.join(",");

    let name = file.replace(['\\', '/', ':'], "__");
    let _ = fs::write(
        runs.join(format!("{name}.txt")),
        format!(
            "file: {file}\nstatus: {status}\nexit: {exit}\ncodes: {codes}\n\n--- stderr\n{text}\n--- stdout (tail)\n{out_tail}\n"
        ),
    );

    FileResult {
        file: file.to_string(),
        status,
        exit,
        codes,
        ms,
        dump: DumpHash::of(&stdout),
    }
}

/// Waits for `child`, killing it after `timeout`. Returns whether it finished,
/// its exit code (-1 when killed), stdout and stderr.
fn wait_with_timeout(mut child: Child, timeout: Duration) -> (bool, i32, String, String) {
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());

    let started = Instant::now();
    let (finished, exit) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (true, status.code().unwrap_or(-1)),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break (false, -1);
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => break (true, -1),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    (finished, exit, stdout, stderr)
}

/// Reads `stream` to its end on its own thread, so a full pipe never blocks
/// the child.
fn read_all(stream: Option<impl Read + Send + 'static>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// `hashes.tsv`: `# mode <name>`, then `file, fnv (hex), bytes, lines,
/// normalized fnv (hex)`.
pub(crate) fn format_hashes(mode: Mode, results: &[FileResult]) -> String {
    let mut text = format!("# mode {}\n", mode.name());
    for r in results {
        let _ = write!(
            text,
            "{}\t{:016x}\t{}\t{}",
            r.file, r.dump.fnv, r.dump.bytes, r.dump.lines
        );
        if let Some(normalized) = r.dump.normalized {
            let _ = write!(text, "\t{normalized:016x}");
        }
        text.push('\n');
    }
    text
}

fn write_outputs(
    out_dir: &Path,
    mode: Mode,
    results: &[FileResult],
    elapsed: Duration,
) -> io::Result<()> {
    let mut tsv = String::new();
    for r in results {
        tsv.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            r.file, r.status, r.exit, r.codes, r.ms
        ));
    }
    fs::write(out_dir.join("results.tsv"), tsv)?;
    fs::write(out_dir.join("hashes.tsv"), format_hashes(mode, results))?;

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in results {
        *counts.entry(r.status).or_default() += 1;
    }
    let mut summary = vec![
        format!("mode: {} ({})", mode.name(), mode.flags().join(" ")),
        format!("files: {}", results.len()),
        format!("elapsed: {}s", elapsed.as_secs()),
        String::new(),
    ];
    summary.extend(
        counts
            .iter()
            .map(|(status, count)| format!("{status:<8} {count}")),
    );
    fs::write(out_dir.join("summary.txt"), summary.join("\n") + "\n")?;

    let mut errors = String::new();
    for r in results.iter().filter(|r| r.status != "ok") {
        errors.push_str(&format!("{}\t{}\t{}\n", r.status, r.codes, r.file));
    }
    fs::write(out_dir.join("errors.txt"), errors)?;

    for line in summary {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(list: &[&str]) -> Vec<String> {
        list.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn hashes_with_fnv1a_64() {
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
        let dump = DumpHash::of("one\ntwo\n");
        assert_eq!((dump.bytes, dump.lines), (8, 2));
    }

    #[test]
    fn normalizing_takes_out_numbering_and_keeps_the_program() {
        let cases = [
            ("    v307 = LoadName(#243)", "    v_ = LoadName(#_)"),
            ("  b22(v6836: Tuple(0)):", "  b_(v_: Tuple(0)):"),
            (
                "    v11263 = MakeClosure(fn36, [v11260])",
                "    v_ = MakeClosure(fn_, [v_])",
            ),
            ("    let %t530 = ICmpEq(n, 0)", "    let %t_ = ICmpEq(n, 0)"),
            (
                "fn factorial [fn#1321] (%0, %1) {",
                "fn factorial [fn#_] (%_, %_) {",
            ),
            ("    br %8, bb2, bb3", "    br %_, bb_, bb_"),
            (
                "  reuse xs#1250 as Cons(h#1252, <sym:4001814>#1814)",
                "  reuse xs#_ as Cons(h#_, <sym:_>#_)",
            ),
            (
                "    v59452 = Const(3.141592653589793f)",
                "    v_ = Const(3.141592653589793f)",
            ),
            (
                "lam(__arg0, sub2, abc12) -> b",
                "lam(__arg0, sub2, abc12) -> b",
            ),
        ];
        for (dump, normalized) in cases {
            assert_eq!(normalize(dump), normalized, "{dump}");
        }
        let renumbered = DumpHash::of("v1 = Const(2)\nb4:\n");
        let original = DumpHash::of("v7 = Const(2)\nb9:\n");
        let changed = DumpHash::of("v7 = Const(3)\nb9:\n");
        assert!(!renumbered.same_dump(&original));
        assert_eq!(renumbered.normalized, original.normalized);
        assert_ne!(changed.normalized, original.normalized);
    }

    fn result(file: &str, status: &'static str) -> FileResult {
        FileResult {
            file: file.to_string(),
            status,
            exit: 0,
            codes: String::new(),
            ms: 1,
            dump: DumpHash::of(file),
        }
    }

    #[test]
    fn the_determinism_sample_is_stable_and_only_ok_files() {
        let results: Vec<FileResult> = (0..40)
            .map(|n| {
                result(
                    &format!("f{n}.flx"),
                    if n % 4 == 0 { "error" } else { "ok" },
                )
            })
            .collect();
        let pick = |results: &[FileResult]| -> Vec<String> {
            determinism_sample(results, 5)
                .iter()
                .map(|r| r.file.clone())
                .collect()
        };
        let first = pick(&results);
        assert_eq!(first.len(), 5);
        let mut reversed = results.clone();
        reversed.reverse();
        assert_eq!(
            pick(&reversed),
            first,
            "the order of the results does not matter"
        );
        assert!(determinism_sample(&results, 0).is_empty());
        assert!(
            determinism_sample(&results, 100)
                .iter()
                .all(|r| r.status == "ok")
        );
        assert_eq!(determinism_sample(&results, 100).len(), 30);
    }

    #[test]
    fn two_cache_trees_differ_by_path_and_bytes() {
        let scratch = std::env::temp_dir().join(format!("xtask-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&scratch);
        let write = |side: &str, path: &str, bytes: &str| {
            let file = scratch.join(side).join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, bytes).unwrap();
        };
        write("a", "vm/same.fxm", "1");
        write("b", "vm/same.fxm", "1");
        assert!(tree_differences(&scratch.join("a"), &scratch.join("b")).is_empty());
        write("a", "vm/x.fxm", "1");
        write("b", "vm/x.fxm", "2");
        write("a", "interfaces/gone.flxi", "1");
        write("b", "home/store/new", "1");
        assert_eq!(
            tree_differences(&scratch.join("a"), &scratch.join("b")),
            [
                "home/store/new only in b",
                "interfaces/gone.flxi only in a",
                "vm/x.fxm differs"
            ]
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    #[test]
    fn every_mode_reads_back_from_its_name() {
        for mode in [
            Mode::Cfg,
            Mode::Core,
            Mode::CoreOpt,
            Mode::Aether,
            Mode::Lir,
        ] {
            assert_eq!(Mode::parse(mode.name()), Some(mode));
        }
        assert_eq!(Mode::Lir.flags(), ["--native", "--dump-lir"]);
    }

    #[test]
    fn classifies_every_ending() {
        assert_eq!(classify(false, -1, "", &[]), "timeout");
        assert_eq!(
            classify(
                true,
                1,
                "error: Internal Compiler Error: x",
                &codes(&["E300"])
            ),
            "ice"
        );
        assert_eq!(
            classify(true, 101, "thread 'main' panicked at src/x.rs:1:1", &[]),
            "panic"
        );
        assert_eq!(
            classify(
                true,
                -1073741571,
                "\nthread 'main' has overflowed its stack\n",
                &[]
            ),
            "panic"
        );
        assert_eq!(classify(true, 0, "", &[]), "ok");
        assert_eq!(classify(true, 1, "error[E300]", &codes(&["E300"])), "error");
        assert_eq!(classify(true, 1, "something else", &[]), "crash");
    }

    #[test]
    fn a_timeout_wins_over_everything() {
        assert_eq!(
            classify(false, -1, "panicked at", &codes(&["E300"])),
            "timeout"
        );
    }

    #[test]
    fn an_overflow_needs_the_whole_message_on_one_line() {
        assert!(overflowed_stack("thread 'main' has overflowed its stack"));
        assert!(!overflowed_stack("thread 'main'\nhas overflowed its stack"));
        assert!(!overflowed_stack("stack overflowed"));
    }
}
