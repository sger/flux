use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static MAP_CONTRACT_TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn write_test_program(source: &str) -> PathBuf {
    let temp_root = workspace_root().join("target/tmp/map_contract_tests");
    std::fs::create_dir_all(&temp_root).unwrap();

    let id = MAP_CONTRACT_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = temp_root.join(format!("map_contract_{id}.flx"));
    std::fs::write(&path, source).unwrap();
    path
}

fn run_strict(source: &str) -> (String, bool) {
    let path = write_test_program(source);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--strict", "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux strict map contract test: {e}"));

    let mut combined = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    combined.push_str(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
    (combined, output.status.success())
}

#[test]
fn flow_map_public_api_has_strict_boundary_contracts() {
    let (out, ok) = run_strict(
        r#"
import Flow.Map as Map
import Flow.Array as Array

fn main() with IO {
    let base = {"name": "flux", "lang": "en"}
    let updated = Map.set(base, "lang", "flux")
    let merged = Map.merge(updated, {"version": "1"})

    print(Map.get(merged, "name"))
    print(Map.get(merged, "missing"))
    print(Map.has(merged, "version"))
    print(Array.sort(Map.keys(merged)))
    print(Array.sort(Map.values(merged)))
    print(Map.delete(merged, "version"))
    print(Map.size(merged))
}
"#,
    );

    assert!(ok, "strict Map API program failed:\n{out}");
    assert!(
        !out.contains("E425"),
        "Map API should not produce unresolved strict boundary diagnostics:\n{out}"
    );

    let lines: Vec<&str> = out
        .lines()
        .filter(|line| !line.starts_with('[') || line.starts_with("[|"))
        .collect();
    assert_eq!(
        lines,
        vec![
            "Some(\"flux\")",
            "None",
            "true",
            "[|\"lang\", \"name\", \"version\"|]",
            "[|\"1\", \"flux\", \"flux\"|]",
            "{\"lang\": \"flux\", \"name\": \"flux\"}",
            "3",
        ]
    );
}
