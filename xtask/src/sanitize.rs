//! `cargo xtask sanitize` (proposal 0190 §15): the native ways over the
//! parity fixtures with the C runtime built with AddressSanitizer and
//! UndefinedBehaviorSanitizer, as Lean's CI runs a sanitizer job.
//!
//! `FLUX_RT_SANITIZE=1` makes every native build link `libflux_rt_sanitize.a`
//! (`src/llvm/pipeline.rs`), and parity's runs inherit it. A sanitizer report
//! stops the program, so its fixture fails. Parity's log cuts long stderr
//! short, so each failing fixture runs once more on each native way, and the
//! report gives what a sanitizer said there and where. Occasional, not part
//! of verify: the runs are slower, and a report means memory the debug heap
//! cannot see.
//!
//! With `--thread`, the runtime is built with ThreadSanitizer instead
//! (`FLUX_RT_SANITIZE=thread`), and a thread-local count changed on a second
//! thread is reported as a data race (audit L7).

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::outcome::format_duration;
use crate::proc::run;
use crate::store;
use crate::sweep::run_with_timeout;
use crate::verify::{ParityCounts, cargo, flux_binary, parity_with_env};

pub struct Options {
    /// The ways to run, comma-separated: native ones, since only they link
    /// the C runtime.
    pub ways: String,
    pub dirs: Vec<String>,
    /// ThreadSanitizer instead of AddressSanitizer and
    /// UndefinedBehaviorSanitizer.
    pub thread: bool,
}

/// What the sanitized runs inherit. Leak detection stays off: native
/// programs still leak (KI-119, KI-121), which `--leak-check` counts.
const ENV: [(&str, &str); 3] = [
    ("FLUX_RT_SANITIZE", "1"),
    ("ASAN_OPTIONS", "detect_leaks=0"),
    ("UBSAN_OPTIONS", "print_stacktrace=1"),
];

/// What the thread-sanitized runs inherit: the first race stops the
/// program, so its fixture fails.
const THREAD_ENV: [(&str, &str); 2] =
    [("FLUX_RT_SANITIZE", "thread"), ("TSAN_OPTIONS", "halt_on_error=1")];

/// Returns `Ok(true)` when every fixture passed.
pub fn run_sanitize(root: &Path, options: &Options) -> Result<bool, String> {
    let started = Instant::now();
    let env: &[(&str, &str)] = if options.thread { &THREAD_ENV } else { &ENV };
    let out = store::run_dir(root, if options.thread { "sanitize-thread" } else { "sanitize" })?;
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let build = run(cargo().current_dir(root).args(["build", "--all-features", "--quiet"]))
        .map_err(|e| e.to_string())?;
    if build != 0 {
        return Err("cargo build failed".to_string());
    }

    let log_path = out.join("parity.txt");
    let mut log =
        fs::File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
    let flux = flux_binary(root);
    let mut counts = ParityCounts::default();
    let mut failed_runs = 0;
    for dir in &options.dirs {
        println!("=== {dir}");
        let run = parity_with_env(root, &flux, dir, &options.ways, false, &mut log, env, &[])
            .map_err(|e| e.to_string())?;
        if run.exit != 0 {
            failed_runs += 1;
        }
        counts.absorb(run);
    }
    drop(log);

    let mut findings = Vec::new();
    for file in counts.failures.iter().filter_map(|line| line.split_whitespace().nth(1)) {
        for way in options.ways.split(',').map(str::trim) {
            let Some(flags) = native_flags(way) else {
                continue;
            };
            let mut command = Command::new(&flux);
            command.envs(env.iter().copied()).args(["--no-cache", "--quiet"]).args(flags).arg(file);
            let ran = run_with_timeout(command, root, Duration::from_secs(120));
            findings.extend(
                sanitizer_lines(&ran.stderr)
                    .into_iter()
                    .map(|line| format!("{file} {way}: {line}")),
            );
        }
    }
    let ok = counts.mismatches == 0 && counts.expected == 0 && failed_runs == 0;
    let report = report(options, &counts, failed_runs, &findings, &log_path, started.elapsed());
    let report_path = out.join("report.txt");
    fs::write(&report_path, &report).map_err(|e| format!("{}: {e}", report_path.display()))?;
    print!("{report}");
    println!("report: {}", store::display(&report_path));
    Ok(ok)
}

/// The flags that run a native way, or `None` for a way that does not link
/// the C runtime.
fn native_flags(way: &str) -> Option<&'static [&'static str]> {
    match way {
        "llvm" => Some(&["--native"]),
        "llvm_opt" => Some(&["--native", "--optimize"]),
        _ => None,
    }
}

/// Each sanitizer report in `stderr`: its first line, and the first stack
/// frame after it where there is one: `#0 <address> in <function>
/// <file>:<line>` from AddressSanitizer, `#0 <function> <file>:<line>
/// (<module>)` from ThreadSanitizer.
fn sanitizer_lines(stderr: &str) -> Vec<String> {
    let lines: Vec<&str> = stderr.lines().map(str::trim).collect();
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let reported = line.contains("ERROR: AddressSanitizer")
            || line.contains("ERROR: UndefinedBehaviorSanitizer")
            || line.contains("WARNING: ThreadSanitizer:")
            || line.contains("runtime error:");
        if !reported {
            continue;
        }
        let frame = lines[at + 1..]
            .iter()
            .take(8)
            .find_map(|next| next.strip_prefix("#0 "))
            .map(|frame| {
                let place = frame.split_once(" in ").map_or(frame, |(_, place)| place);
                let place = place.split(" (").next().unwrap_or(place);
                format!(" (in {place})")
            })
            .unwrap_or_default();
        found.push(format!("{line}{frame}"));
    }
    found
}

fn report(
    options: &Options,
    counts: &ParityCounts,
    failed_runs: usize,
    findings: &[String],
    log: &Path,
    took: Duration,
) -> String {
    let mut report = format!(
        "sanitize: {} on {} ({} MISMATCH, {} EXPECTED_OUTPUT_MISMATCH, {failed_runs} of {} run(s) exited non-zero) in {}\n",
        options.ways,
        options.dirs.join(", "),
        counts.mismatches,
        counts.expected,
        options.dirs.len(),
        format_duration(took)
    );
    if findings.is_empty() {
        report.push_str("no sanitizer reports\n");
    } else {
        report.push_str(&format!("{} sanitizer report(s):\n", findings.len()));
        for line in findings {
            report.push_str(&format!("  {line}\n"));
        }
    }
    for failure in &counts.failures {
        report.push_str(&format!("{failure}\n"));
    }
    report.push_str(&format!("log: {}\n", store::display(log)));
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sanitizer_report_is_named_with_its_first_frame() {
        let stderr = "42\n\
                      ==123==ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n\
                      WRITE of size 1 at 0x1 thread T0\n\
                      \x20   #0 0x10 in flux_rt_shutdown flux_rt.c:183\n\
                      \x20   #1 0x20 in main flux_rt.c:4400\n\
                      rc.c:12:5: runtime error: signed integer overflow\n";
        assert_eq!(
            sanitizer_lines(stderr),
            [
                "==123==ERROR: AddressSanitizer: heap-use-after-free on address 0x1 \
                 (in flux_rt_shutdown flux_rt.c:183)",
                "rc.c:12:5: runtime error: signed integer overflow",
            ]
        );
        assert!(sanitizer_lines("42\n").is_empty());
    }

    #[test]
    fn a_thread_sanitizer_race_is_named_with_its_first_frame() {
        let stderr = "WARNING: ThreadSanitizer: data race (pid=1)\n\
                      \x20 Atomic read of size 4 at 0x1 by thread T2:\n\
                      \x20   #0 flux_dup rc.c:571 (program:arm64+0x10)\n\
                      \x20   #1 flux_array_get array.c:117 (program:arm64+0x20)\n";
        assert_eq!(
            sanitizer_lines(stderr),
            ["WARNING: ThreadSanitizer: data race (pid=1) (in flux_dup rc.c:571)"]
        );
    }

    #[test]
    fn only_native_ways_are_rerun() {
        assert_eq!(native_flags("llvm_opt"), Some(&["--native", "--optimize"][..]));
        assert_eq!(native_flags("vm"), None);
    }
}
