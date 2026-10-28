//! `cargo xtask tidy`: checks formatting, then that `tests/` is laid out the
//! way `tests/README.md` says, as rustc's `x test tidy` does. Rules that live
//! only in prose drift; these fail the build instead.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::affected::Repo;
use crate::verify::cargo;

pub fn run(root: &Path) -> Result<bool, String> {
    let readme = std::fs::read_to_string(root.join("tests/README.md"))
        .map_err(|e| format!("tests/README.md: {e}"))?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("tests")).map_err(|e| format!("tests/: {e}"))? {
        let entry = entry.map_err(|e| format!("tests/: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    let repo = Repo::load(root)?;
    let targets: BTreeSet<String> = repo.test_sources().map(str::to_string).collect();

    let mut problems = unformatted(root)?;
    problems.extend(layout_problems(&readme, &dirs, &files));
    problems.extend(uncompiled(&targets, repo.sources()));
    problems.extend(local_helpers(repo.sources()));
    for problem in &problems {
        println!("tidy: {problem}");
    }
    if problems.is_empty() {
        println!("tidy: formatted, and tests/ matches tests/README.md");
    }
    Ok(problems.is_empty())
}

/// One problem per file `cargo fmt` would change.
fn unformatted(root: &Path) -> Result<Vec<String>, String> {
    let output = cargo()
        .current_dir(root)
        .args(["fmt", "--all", "--", "--check", "--files-with-diff"])
        .output()
        .map_err(|e| format!("cargo fmt: {e}"))?;
    let prefix = format!("{}/", root.display());
    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|file| file.strip_prefix(&prefix).unwrap_or(file).to_string())
        .collect();
    if !output.status.success() && files.is_empty() {
        return Err(format!("cargo fmt failed:\n{}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(files
        .into_iter()
        .map(|file| format!("{file} is not formatted; run cargo fmt --all"))
        .collect())
}

/// The folders the README's layout tree names: its `├── name/` lines.
fn readme_dirs(readme: &str) -> BTreeSet<String> {
    readme
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("├── ").or_else(|| line.strip_prefix("└── "))?;
            Some(rest.split('/').next()?.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// Folders missing from the README's tree or listed there but gone, and
/// files sitting directly in `tests/`.
fn layout_problems(readme: &str, dirs: &[String], files: &[String]) -> Vec<String> {
    let listed = readme_dirs(readme);
    let present: BTreeSet<&String> = dirs.iter().collect();
    let mut problems = Vec::new();
    for dir in &present {
        if !listed.contains(*dir) {
            problems.push(format!(
                "tests/{dir}/ is not in tests/README.md's layout; add it and say what goes there"
            ));
        }
    }
    for dir in &listed {
        if !present.contains(dir) {
            problems.push(format!("tests/README.md lists tests/{dir}/, which does not exist"));
        }
    }
    let mut files: Vec<&String> =
        files.iter().filter(|name| name.as_str() != "README.md").collect();
    files.sort();
    for file in files {
        problems.push(format!(
            "tests/{file} sits directly in tests/; put it in the folder for its phase"
        ));
    }
    problems
}

/// Test files nothing compiles: not a `[[test]]` target and not named by a
/// `#[path]` in another test file. `support/` and `snapshots/` hold modules
/// that are included in other ways.
fn uncompiled(targets: &BTreeSet<String>, sources: &[(String, String)]) -> Vec<String> {
    let mut included = BTreeSet::new();
    for (path, text) in sources {
        let dir = Path::new(path).parent().unwrap_or(Path::new(""));
        for target in path_attributes(text) {
            included.insert(normalize(&dir.join(target)));
        }
    }
    sources
        .iter()
        .map(|(path, _)| path)
        .filter(|path| !path.starts_with("tests/support/") && !path.starts_with("tests/snapshots/"))
        .filter(|path| !targets.contains(*path) && !included.contains(*path))
        .map(|path| format!("{path} is never compiled; give it a [[test]] entry in Cargo.toml"))
        .collect()
}

/// The paths in a file's `#[path = "…"]` attributes.
fn path_attributes(text: &str) -> impl Iterator<Item = &str> {
    text.split("#[path").skip(1).filter_map(|rest| {
        let start = rest.find('"')? + 1;
        let len = rest[start..].find('"')?;
        Some(&rest[start..start + len])
    })
}

fn normalize(path: &Path) -> String {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out.to_string_lossy().replace('\\', "/")
}

/// Helpers `tests/support/` already provides, written again in a test: each
/// copy is how fixed temp paths and leaked folders came back before.
fn local_helpers(sources: &[(String, String)]) -> Vec<String> {
    const PATTERNS: [(&str, &str); 3] = [
        ("fn workspace_root(", "use scratch::workspace_root"),
        ("env::temp_dir(", "use a Scratch"),
        (".join(\"test-scratch\")", "use a Scratch"),
    ];
    let mut problems = Vec::new();
    for (path, text) in sources {
        if path.starts_with("tests/support/") {
            continue;
        }
        for (number, line) in text.lines().enumerate() {
            for (pattern, fix) in PATTERNS {
                if line.contains(pattern) {
                    problems.push(format!("{path}:{}: `{pattern}`; {fix}", number + 1));
                }
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const README: &str =
        "```\ntests/\n├── support/          Helpers\n└── parity/           Programs\n```\n";

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    fn sources(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(path, text)| (path.to_string(), text.to_string())).collect()
    }

    #[test]
    fn the_readme_tree_names_its_folders() {
        assert_eq!(readme_dirs(README).into_iter().collect::<Vec<_>>(), ["parity", "support"]);
    }

    #[test]
    fn a_layout_matching_the_readme_passes() {
        let dirs = strings(&["support", "parity"]);
        assert!(layout_problems(README, &dirs, &strings(&["README.md"])).is_empty());
    }

    #[test]
    fn an_unlisted_folder_a_missing_one_and_a_loose_file_are_reported() {
        let dirs = strings(&["support", "replay"]);
        let problems = layout_problems(README, &dirs, &strings(&["README.md", "stray.rs"]));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("tests/replay/ is not in"));
        assert!(problems[1].starts_with("tests/README.md lists tests/parity/"));
        assert!(problems[2].starts_with("tests/stray.rs sits directly"));
    }

    #[test]
    fn a_file_is_compiled_as_a_target_or_through_a_path_attribute() {
        let targets: BTreeSet<String> = ["tests/vm_runtime/a_tests.rs".to_string()].into();
        let files = sources(&[
            ("tests/vm_runtime/a_tests.rs", "#[path = \"../shared/b.rs\"]\nmod b;"),
            ("tests/shared/b.rs", ""),
            ("tests/support/scratch.rs", ""),
            ("tests/vm_runtime/forgotten.rs", "#[test] fn t() {}"),
        ]);
        assert_eq!(
            uncompiled(&targets, &files),
            [
                "tests/vm_runtime/forgotten.rs is never compiled; give it a [[test]] entry in Cargo.toml"
            ]
        );
    }

    #[test]
    fn a_copied_helper_is_reported_with_its_line() {
        let files = sources(&[
            (
                "tests/vm_runtime/a_tests.rs",
                "use x;\nfn workspace_root() -> &'static Path {}\nlet d = std::env::temp_dir();",
            ),
            ("tests/support/scratch.rs", "pub fn workspace_root() {}"),
            ("tests/vm_runtime/b_tests.rs", "let s = \"target/test-scratch/x.bin\";"),
        ]);
        let problems = local_helpers(&files);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("tests/vm_runtime/a_tests.rs:2:"));
        assert!(problems[1].starts_with("tests/vm_runtime/a_tests.rs:3:"));
    }
}
