//! `flux --metrics` (proposal 0190 S11): what it writes, and that it writes
//! the same counts on every run.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

use crate::support::scratch::{Scratch, workspace_root};

const PROGRAM: &str = r#"
fn square(x: Int) -> Int {
    x * x
}

fn sum_squares(xs: List<Int>) -> Int {
    match xs {
        [h | t] -> square(h) + sum_squares(t),
        _ -> 0,
    }
}

fn main() with IO {
    print(sum_squares([1, 2, 3, 4]))
}
"#;

fn run(scratch: &Scratch, program: &Path, metrics: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .arg(program)
        .args(["--metrics", metrics.to_str().unwrap()])
        .args(args)
        .args(scratch.cache_args())
        .output()
        .expect("run flux")
}

fn read(metrics: &Path) -> BTreeMap<String, u64> {
    std::fs::read_to_string(metrics)
        .unwrap_or_else(|e| panic!("read {}: {e}", metrics.display()))
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('\t').expect("key<TAB>value");
            (key.to_string(), value.parse().expect("a count"))
        })
        .collect()
}

fn metrics_of(source: &str, args: &[&str]) -> BTreeMap<String, u64> {
    let scratch = Scratch::new("metrics");
    let program = scratch.write("main.flx", source);
    let metrics = scratch.join("metrics.tsv");
    let output = run(&scratch, &program, &metrics, args);
    assert!(output.status.success(), "flux failed: {}", String::from_utf8_lossy(&output.stderr));
    read(&metrics)
}

#[test]
fn metrics_has_every_count() {
    let metrics = metrics_of(PROGRAM, &["--no-cache"]);
    let keys: Vec<&str> = metrics.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "aether_allocs",
            "aether_drops",
            "aether_dups",
            "aether_reuses",
            "bytecode_bytes",
            "compiler_allocs",
            "core_size",
            "vm_allocs",
            "vm_instructions",
        ]
    );
    assert!(metrics["vm_instructions"] > 0);
    assert!(metrics["core_size"] > 0);
}

#[test]
fn metrics_repeat_on_every_run() {
    let first = metrics_of(PROGRAM, &["--no-cache"]);
    let second = metrics_of(PROGRAM, &["--no-cache"]);
    for (key, value) in &first {
        // The allocator's counts move a little between runs, which is why
        // verify gives them a tolerance.
        if key.ends_with("_allocs") && !key.starts_with("aether") {
            let other = second[key] as f64;
            assert!((*value as f64 - other).abs() <= other * 0.01, "{key}: {value} vs {other}");
        } else {
            assert_eq!(*value, second[key], "{key}");
        }
    }
}

#[test]
fn metrics_core_size_is_counted_after_optimization() {
    let source = r#"
fn answer() -> Int {
    (1 + 2) * (3 + 4) + (5 + 6) * (7 + 8)
}

fn main() with IO {
    print(answer())
}
"#;
    let plain = metrics_of(source, &["--no-cache"]);
    let optimized = metrics_of(source, &["--no-cache", "--optimize"]);
    assert!(
        optimized["core_size"] < plain["core_size"],
        "optimized {} against plain {}",
        optimized["core_size"],
        plain["core_size"]
    );
}

#[test]
fn metrics_bytecode_counts_the_entry_files_functions() {
    let scratch = Scratch::new("metrics");
    let program = scratch.write("main.flx", PROGRAM);
    let metrics = scratch.join("metrics.tsv");
    let output = run(&scratch, &program, &metrics, &["--no-cache", "--stats"]);
    assert!(output.status.success());
    // `--stats` counts `<main>`'s instructions only.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let main_bytes: u64 = stderr
        .lines()
        .find(|line| line.contains("instructions") && line.ends_with("bytes"))
        .and_then(|line| line.split_whitespace().rev().nth(1))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no instruction bytes in --stats:\n{stderr}"));
    assert!(read(&metrics)["bytecode_bytes"] > main_bytes);
}

#[test]
fn metrics_of_a_failing_program_keep_the_compile_time_counts() {
    let scratch = Scratch::new("metrics");
    let program = scratch.write("main.flx", "fn main() with IO {\n    panic(\"stop\")\n}\n");
    let metrics = scratch.join("metrics.tsv");
    let output = run(&scratch, &program, &metrics, &["--no-cache"]);
    assert!(!output.status.success());
    let metrics = read(&metrics);
    assert!(metrics.contains_key("core_size"));
    assert!(!metrics.contains_key("vm_instructions"));
}

#[test]
fn metrics_are_written_with_the_cache_on() {
    let scratch = Scratch::new("metrics");
    let program = scratch.write("main.flx", PROGRAM);
    let cold = scratch.join("cold.tsv");
    let warm = scratch.join("warm.tsv");
    // The first run fills the cache and the second could reuse it, but
    // `--metrics` compiles everything both times.
    assert!(run(&scratch, &program, &cold, &[]).status.success());
    assert!(run(&scratch, &program, &warm, &[]).status.success());
    let (cold, warm) = (read(&cold), read(&warm));
    assert_eq!(cold["core_size"], warm["core_size"]);
    assert_eq!(cold["bytecode_bytes"], warm["bytecode_bytes"]);
    assert_eq!(cold["vm_instructions"], warm["vm_instructions"]);
}
