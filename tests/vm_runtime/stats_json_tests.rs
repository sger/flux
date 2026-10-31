//! `flux --stats-json <file>`: the `--stats` timings as JSON, one record per
//! phase, written without changing what the program prints.

use crate::support::flux_runner::{flux_source, scratch::Scratch};

#[test]
fn stats_json_records_each_phase_of_a_vm_run() {
    let scratch = Scratch::new("stats-json");
    let file = scratch.join("stats.json");
    let file_arg = file.to_str().expect("a UTF-8 scratch path");

    let ran = flux_source("fn main() with IO {\n    print(6 * 7)\n}\n", "stats-json")
        .args(&["--stats-json", file_arg])
        .run();
    assert!(ran.success, "flux failed:\n{}", ran.stderr);
    assert_eq!(ran.stdout.trim(), "42");

    let text = std::fs::read_to_string(&file).expect("--stats-json wrote its file");
    let stats: serde_json::Value = serde_json::from_str(&text).expect("the file is JSON");
    assert_eq!(stats["version"], 1);
    assert_eq!(stats["backend"], "vm");
    let phases: Vec<_> = stats["phases"]
        .as_array()
        .expect("a phases list")
        .iter()
        .map(|phase| {
            assert!(phase["ms"].as_f64().is_some_and(|ms| ms >= 0.0), "{phase}");
            phase["phase"].as_str().expect("a phase name")
        })
        .collect();
    assert_eq!(phases, ["parse", "compile", "execute"]);
}
