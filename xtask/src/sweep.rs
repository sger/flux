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
//!
//! Every run uses `--no-cache`, so no cached module hides a change.
//!
//! Writes to the out dir:
//! - `runs/<file>.txt`: each file's output
//! - `results.tsv`: file, status, exit code, diagnostic codes, milliseconds
//! - `summary.txt`: how many files ended each way
//! - `errors.txt`: the files that did not end `ok`
//!
//! Statuses: ok, error (diagnostics, none internal), ice (an internal compiler
//! error), panic (the compiler panicked), timeout, crash (any other failure).

use std::collections::BTreeMap;
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
}

impl Mode {
    pub fn parse(name: &str) -> Option<Mode> {
        match name {
            "cfg" => Some(Mode::Cfg),
            "core" => Some(Mode::Core),
            "core-opt" => Some(Mode::CoreOpt),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Cfg => "cfg",
            Mode::Core => "core",
            Mode::CoreOpt => "core-opt",
        }
    }

    fn flags(self) -> &'static [&'static str] {
        match self {
            Mode::Cfg => &["--dump-cfg"],
            Mode::Core => &["--dump-core"],
            Mode::CoreOpt => &["--optimize", "--dump-core"],
        }
    }
}

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
    let flux = out_dir.join(options.flux.file_name().expect("compiler file name"));
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

fn sweep_file(
    root: &Path,
    flux: &Path,
    flags: &[&str],
    runs: &Path,
    file: &str,
    timeout: Duration,
) -> FileResult {
    let clock = Instant::now();
    let spawned = Command::new(flux)
        .args(["--no-cache", "--quiet"])
        .args(flags)
        .arg(file)
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
