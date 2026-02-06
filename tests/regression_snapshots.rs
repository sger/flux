use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn discover_fixtures(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir).unwrap_or_else(|e| {
            panic!("failed to read fixture directory `{}`: {e}", dir.display())
        });

        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("failed to read fixture entry: {e}"));
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("flx") {
                out.push(path);
            }
        }
    }

    let mut fixtures = Vec::new();
    walk(root, &mut fixtures);
    fixtures.sort();
    fixtures
}

fn snapshot_name(fixtures_root: &Path, fixture: &Path) -> String {
    let rel = fixture
        .strip_prefix(fixtures_root)
        .unwrap_or_else(|_| panic!("fixture `{}` is not under root", fixture.display()));
    let mut name = rel.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = name.strip_suffix(".flx") {
        name = stripped.to_string();
    }
    name.replace('/', "__")
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }

    out
}

fn normalize_output(output: &str, workspace_root: &Path) -> String {
    let mut normalized = output.replace("\r\n", "\n").replace('\\', "/");
    normalized = strip_ansi(&normalized);

    let mut prefixes = vec![workspace_root.to_string_lossy().replace('\\', "/")];
    if let Ok(canonical) = workspace_root.canonicalize() {
        prefixes.push(canonical.to_string_lossy().replace('\\', "/"));
    }

    for prefix in prefixes {
        if prefix.is_empty() {
            continue;
        }
        let with_slash = format!("{prefix}/");
        normalized = normalized.replace(&with_slash, "");
        normalized = normalized.replace(&prefix, "");
    }

    let mut cleaned = String::new();
    for line in normalized.lines() {
        if line.starts_with("Finished `") || line.starts_with("Running `") {
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }

    cleaned
}

#[test]
fn regression_fixtures_cli_output() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_root = workspace_root.join("tests/fixtures");
    let flux_bin = PathBuf::from(env!("CARGO_BIN_EXE_flux"));

    let fixtures = discover_fixtures(&fixtures_root);
    assert!(
        !fixtures.is_empty(),
        "no fixtures found under `{}`",
        fixtures_root.display()
    );

    for fixture in fixtures {
        let snapshot = snapshot_name(&fixtures_root, &fixture);
        let rel = fixture
            .strip_prefix(workspace_root)
            .unwrap_or(&fixture)
            .to_string_lossy()
            .replace('\\', "/");

        let output = Command::new(&flux_bin)
            .arg("--no-cache")
            .arg(&fixture)
            .env("NO_COLOR", "1")
            .output()
            .unwrap_or_else(|e| panic!("failed to run flux for `{}`: {e}", fixture.display()));

        let mut combined = String::new();
        combined.push_str(&String::from_utf8_lossy(&output.stdout));
        combined.push_str(&String::from_utf8_lossy(&output.stderr));

        let exit_code = output.status.code().unwrap_or(-1);
        let rendered = format!(
            "$ flux --no-cache {rel}\nexit_code: {exit_code}\n\n{}",
            normalize_output(&combined, workspace_root)
        );

        insta::with_settings!({
            snapshot_path => "snapshots/regression",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!(snapshot, rendered);
        });
    }
}
