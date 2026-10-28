//! `cargo xtask rewrite` (proposal 0190 §10): rewrite each fixture in ways
//! that cannot change what it prints, and check with `parity-check` that the
//! copies still print it. This is metamorphic testing, as EMI does for C
//! compilers: every fixture becomes several tests whose answer is already
//! known.
//!
//! The copies go to `.verify/rewrite_<seed>/<kind>/`, mirroring their paths.
//! Each one is checked against the original's output: the expected-stdout
//! block it already has, or else what the original printed when run once
//! here, written into the copy's header. A copy that no longer compiles is
//! counted on its own line, as it is either a rewriter bug or a compiler
//! that depends on what was rewritten.

mod items;
mod let_wrap;
mod lex;
mod rename;
mod reorder;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::outcome::{Steps, format_duration};
use crate::proc::run;
use crate::store;
use crate::sweep::{fnv64, run_flux};
use crate::text::diagnostic_codes;
use crate::verify::{self, ParityCounts, cargo, flux_binary};
use items::Fixture;

/// What `rewrite` covers unless paths are named: two folders whose fixtures
/// keep everything they declare inside the file (neither has a `parity.dir`).
pub const DEFAULT_PATHS: [&str; 2] = ["tests/parity", "examples/guide"];
pub const DEFAULT_WAYS: &str = "vm,vm_opt,llvm,llvm_opt";
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Reorder,
    Rename,
    LetWrap,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Reorder, Kind::Rename, Kind::LetWrap];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Reorder => "reorder",
            Kind::Rename => "rename",
            Kind::LetWrap => "let-wrap",
        }
    }

    /// The kinds `--kind <name>` selects; `all` is every kind.
    pub fn parse(name: &str) -> Option<Vec<Kind>> {
        if name == "all" {
            return Some(Kind::ALL.to_vec());
        }
        Kind::ALL.into_iter().find(|k| k.name() == name).map(|k| vec![k])
    }

    fn rewrite(self, fixture: &Fixture, rng: &mut Rng) -> Option<String> {
        match self {
            Kind::Reorder => reorder::reorder(fixture, rng),
            Kind::Rename => rename::rename(fixture),
            Kind::LetWrap => let_wrap::let_wrap(fixture, rng),
        }
    }
}

pub struct Options {
    pub kinds: Vec<Kind>,
    pub seed: u64,
    pub ways: String,
    /// Files or folders, from the repository root.
    pub paths: Vec<String>,
    pub jobs: usize,
}

/// splitmix64: small, std-only, and the same on every platform, so a seed
/// names one set of copies.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n`; `n` must not be 0.
    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next_u64() % n as u64).expect("below n, which is a usize")
    }

    /// Fisher–Yates.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

/// A fixture that can be rewritten.
struct Original {
    /// From the repository root.
    file: String,
    source: String,
    /// The expected-stdout block to add to each copy, when the original has
    /// none: what the original printed.
    expected: Option<String>,
}

/// What became of one kind's copies.
#[derive(Default)]
struct Tally {
    rewritten: usize,
    unchanged: usize,
    /// `<file>: <why>` for each copy the rewriter got wrong.
    rewriter_errors: Vec<String>,
    /// `<file>: <first error>` for each copy that no longer compiles.
    not_compiling: Vec<String>,
    parity: ParityCounts,
}

impl Tally {
    fn failed(&self) -> bool {
        !self.rewriter_errors.is_empty()
            || !self.not_compiling.is_empty()
            || !self.parity.failures.is_empty()
            || self.parity.exit != 0
    }
}

/// Returns whether every copy compiled and matched.
pub fn run_rewrite(root: &Path, options: &Options) -> Result<bool, String> {
    let out = store::store_root(root).join(format!("rewrite_{}", options.seed));
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("outputs: {}", store::display(&out));
    let started = Instant::now();
    let mut steps = Steps::new(2 + options.kinds.len());

    steps.start("build");
    let build = run(cargo().current_dir(root).args(["build", "--all-features", "--quiet"]))
        .map_err(|e| e.to_string())?;
    if build != 0 {
        return Err("cargo build failed".to_string());
    }
    let flux = flux_binary(root);

    steps.start("read the fixtures and run the ones without expected output");
    let files = fixture_files(root, &options.paths)?;
    let prepared = parallel(&files, options.jobs, |file| prepare(root, &flux, file));
    let mut originals = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    for (file, result) in files.iter().zip(prepared) {
        match result {
            Ok(original) => originals.push(original),
            Err(reason) => skipped.push((file.clone(), reason)),
        }
    }
    println!(
        "{} fixture(s): {} to rewrite, {} skipped",
        files.len(),
        originals.len(),
        skipped.len()
    );

    let mut tallies = Vec::new();
    for &kind in &options.kinds {
        steps.start(&format!("{} on {}", kind.name(), options.ways));
        let tally = run_kind(root, &flux, &out, kind, &originals, options)?;
        tallies.push((kind, tally));
    }

    let summary = summary(options, &files, &skipped, &tallies, started.elapsed());
    write(&out.join("summary.txt"), &summary)?;
    write(&out.join("failures.txt"), &failures(&tallies))?;
    write(&out.join("skipped.txt"), &skipped_list(&skipped))?;
    print!("{summary}");
    Ok(tallies.iter().all(|(_, tally)| !tally.failed()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The `.flx` files under `paths`, sorted, from the repository root.
fn fixture_files(root: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, files)?;
            } else if path.extension().is_some_and(|e| e == "flx") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    for path in paths {
        let full = root.join(path);
        if full.is_dir() {
            walk(&full, &mut found).map_err(|e| format!("{path}: {e}"))?;
        } else if full.is_file() {
            found.push(full);
        } else {
            return Err(format!("{path}: no such file or folder"));
        }
    }
    let mut files: Vec<String> = found
        .iter()
        .map(|p| p.strip_prefix(root).unwrap_or(p).to_string_lossy().into_owned())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Runs `job` on each item on `jobs` threads, keeping the items' order.
fn parallel<T: Sync, R: Send>(items: &[T], jobs: usize, job: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(items.len()));
    thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    let result = job(item);
                    results.lock().unwrap().push((index, result));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

/// Reads `file` and decides whether it can be rewritten; `Err` is why not.
fn prepare(root: &Path, flux: &Path, file: &str) -> Result<Original, String> {
    let path = root.join(file);
    if path
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(root) && *dir != root)
        .any(|dir| dir.join("parity.dir").exists())
    {
        return Err("its folder's parity.dir would not travel with a copy".to_string());
    }
    let source = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let fixture = Fixture::parse(&source).map_err(|e| format!("cannot split: {e}"))?;
    if !fixture.has_main() {
        return Err("no fn main".to_string());
    }
    if fixture.items.iter().any(|item| item.keyword == "module") {
        return Err("declares a module".to_string());
    }
    let imports_outside_flow =
        fixture.tokens.windows(2).any(|w| w[0].is(&source, "import") && !w[1].is(&source, "Flow"));
    if imports_outside_flow {
        return Err("imports a module that would not travel with a copy".to_string());
    }
    let directives = header_directives(&source[..fixture.header_end]);
    if let Some(directive) = directives.iter().find(|d| {
        d.starts_with("skip:") || matches!(**d, "stdout: varies" | "expect: compile_error")
    }) {
        return Err(format!("// {directive}"));
    }
    let expected = if has_expected_stdout(&source) {
        None
    } else {
        let ran = run_flux(root, flux, &[], file, TIMEOUT);
        if !ran.finished {
            return Err("the original timed out".to_string());
        }
        if compile_errors(&ran.stderr) {
            return Err("the original does not compile".to_string());
        }
        Some(ran.stdout)
    };
    Ok(Original { file: file.to_string(), source, expected })
}

/// The text of each `//` line in `header`, trimmed.
fn header_directives(header: &str) -> Vec<&str> {
    header.lines().filter_map(|line| line.trim().strip_prefix("//")).map(str::trim).collect()
}

fn has_expected_stdout(source: &str) -> bool {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("//"))
        .any(|c| matches!(c.trim(), "parity-expected-stdout-begin" | "parity-oracle-stdout-begin"))
}

/// Whether `stderr` reports a compile error: a diagnostic code outside the
/// runtime's E1000–E1999 (`src/parity/runner.rs`).
fn compile_errors(stderr: &str) -> bool {
    diagnostic_codes(stderr).iter().any(|code| !(code.len() == 5 && code.starts_with("E1")))
}

/// `text` with an expected-stdout block for `stdout` inserted at `at`, the
/// end of its header.
fn with_expected(text: &str, at: usize, stdout: &str) -> String {
    let mut block = String::from("// parity-expected-stdout-begin\n");
    for line in stdout.trim().lines() {
        let _ = writeln!(block, "// {line}");
    }
    block.push_str("// parity-expected-stdout-end\n");
    format!("{}{block}{}", &text[..at], &text[at..])
}

/// Whether `a` and `b` are the same tokens in some order, which a reorder
/// must keep.
fn same_tokens(a: &str, b: &str) -> bool {
    let sorted = |source: &str| {
        lex::tokenize(source).map(|tokens| {
            let mut texts: Vec<&str> = tokens.iter().map(|t| t.text(source)).collect();
            texts.sort_unstable();
            texts.join("\u{0}")
        })
    };
    matches!((sorted(a), sorted(b)), (Ok(x), Ok(y)) if x == y)
}

/// Writes `kind`'s copies, sets aside those that no longer compile, and runs
/// `parity-check` on the rest.
fn run_kind(
    root: &Path,
    flux: &Path,
    out: &Path,
    kind: Kind,
    originals: &[Original],
    options: &Options,
) -> Result<Tally, String> {
    let dir = out.join(kind.name());
    let mut tally = Tally::default();
    let mut copies = Vec::new();
    for original in originals {
        let fixture =
            Fixture::parse(&original.source).map_err(|e| format!("{}: {e}", original.file))?;
        let salt = fnv64(format!("{}:{}", kind.name(), original.file).as_bytes());
        let mut rng = Rng::new(options.seed ^ salt);
        let Some(mut text) = kind.rewrite(&fixture, &mut rng) else {
            tally.unchanged += 1;
            continue;
        };
        if kind == Kind::Reorder && !same_tokens(&original.source, &text) {
            tally.rewriter_errors.push(format!("{}: the tokens changed", original.file));
            continue;
        }
        if let Some(stdout) = &original.expected {
            text = with_expected(&text, fixture.header_end, stdout);
        }
        let copy = dir.join(&original.file);
        if let Some(parent) = copy.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        write(&copy, &text)?;
        tally.rewritten += 1;
        copies.push((original.file.clone(), copy));
    }

    let compiled = parallel(&copies, options.jobs, |(_, copy)| {
        run_flux(root, flux, &["--dump-core"], &copy.to_string_lossy(), TIMEOUT)
    });
    for ((file, copy), ran) in copies.iter().zip(compiled) {
        let failure = if !ran.finished {
            Some("timed out compiling".to_string())
        } else if compile_errors(&ran.stderr) {
            let first = ran.stderr.lines().find(|l| l.contains("error[E")).unwrap_or("").trim();
            Some(first.to_string())
        } else {
            None
        };
        if let Some(failure) = failure {
            // Out of the folder, so parity-check does not count it again.
            fs::rename(copy, copy.with_extension("flx.no-compile"))
                .map_err(|e| format!("{}: {e}", copy.display()))?;
            tally.not_compiling.push(format!("{file}: {failure}"));
        }
    }
    println!(
        "{}: {} rewritten, {} unchanged, {} rewriter error(s), {} no longer compile",
        kind.name(),
        tally.rewritten,
        tally.unchanged,
        tally.rewriter_errors.len(),
        tally.not_compiling.len()
    );

    if tally.rewritten > tally.not_compiling.len() {
        let shown = dir.strip_prefix(root).unwrap_or(&dir).to_string_lossy().into_owned();
        let mut log = fs::File::create(out.join(format!("{}.log", kind.name())))
            .map_err(|e| e.to_string())?;
        tally.parity = verify::parity(root, flux, &shown, &options.ways, false, &mut log)
            .map_err(|e| e.to_string())?;
    }
    Ok(tally)
}

fn summary(
    options: &Options,
    files: &[String],
    skipped: &[(String, String)],
    tallies: &[(Kind, Tally)],
    took: Duration,
) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "rewrite, seed {}, ways {}: {} fixture(s), {} skipped, in {}",
        options.seed,
        options.ways,
        files.len(),
        skipped.len(),
        format_duration(took)
    );
    let mut reasons: Vec<(&str, usize)> = Vec::new();
    for (_, reason) in skipped {
        match reasons.iter_mut().find(|(r, _)| r == reason) {
            Some((_, n)) => *n += 1,
            None => reasons.push((reason, 1)),
        }
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    for (reason, n) in reasons {
        let _ = writeln!(text, "  skipped {n:>4}  {reason}");
    }
    for (kind, tally) in tallies {
        let _ = writeln!(
            text,
            "{:<9} {} rewritten, {} unchanged; {} rewriter error(s), {} no longer compile, {} MISMATCH, {} EXPECTED_OUTPUT_MISMATCH{}",
            kind.name(),
            tally.rewritten,
            tally.unchanged,
            tally.rewriter_errors.len(),
            tally.not_compiling.len(),
            tally.parity.mismatches,
            tally.parity.expected,
            if tally.failed() { "  FAILED" } else { "" }
        );
    }
    text
}

/// One line per failing copy: kind, what failed, and the file.
fn failures(tallies: &[(Kind, Tally)]) -> String {
    let mut text = String::new();
    for (kind, tally) in tallies {
        let kind = kind.name();
        for line in &tally.rewriter_errors {
            let _ = writeln!(text, "{kind}\trewriter error\t{line}");
        }
        for line in &tally.not_compiling {
            let _ = writeln!(text, "{kind}\tno longer compiles\t{line}");
        }
        for line in &tally.parity.failures {
            let _ = writeln!(text, "{kind}\tparity\t{line}");
        }
    }
    text
}

fn skipped_list(skipped: &[(String, String)]) -> String {
    let mut text = String::new();
    for (file, reason) in skipped {
        let _ = writeln!(text, "{file}\t{reason}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_parse_by_name_or_all() {
        assert_eq!(Kind::parse("let-wrap"), Some(vec![Kind::LetWrap]));
        assert_eq!(Kind::parse("all").map(|k| k.len()), Some(3));
        assert_eq!(Kind::parse("shuffle"), None);
    }

    #[test]
    fn a_seed_gives_one_sequence() {
        let draw = |seed| {
            let mut rng = Rng::new(seed);
            (0..4).map(|_| rng.below(100)).collect::<Vec<_>>()
        };
        assert_eq!(draw(1), draw(1));
        assert_ne!(draw(1), draw(2));
    }

    #[test]
    fn an_expected_block_goes_at_the_end_of_the_header() {
        let source = "// expect: success\n\nfn main() {}\n";
        let at = "// expect: success\n\n".len();
        assert_eq!(
            with_expected(source, at, "1\n\"two\"\n"),
            "// expect: success\n\n// parity-expected-stdout-begin\n// 1\n// \"two\"\n// parity-expected-stdout-end\nfn main() {}\n"
        );
    }

    #[test]
    fn an_existing_expected_block_is_found() {
        assert!(has_expected_stdout(
            "// h\n// parity-expected-stdout-begin\n// 1\n// parity-expected-stdout-end\n"
        ));
        assert!(!has_expected_stdout("// h\nfn main() {}\n"));
    }

    #[test]
    fn runtime_error_codes_are_not_compile_errors() {
        assert!(!compile_errors("error[E1001]: boom"));
        assert!(compile_errors("error[E024]: unknown name"));
    }
}
