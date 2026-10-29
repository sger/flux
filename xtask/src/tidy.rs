//! `cargo xtask tidy`: checks formatting, then that `tests/` is laid out the
//! way `tests/README.md` says, as rustc's `x test tidy` does, that a
//! fixture named for an error code expects that code, and that every known
//! flake cites an open KI. Rules that live only in prose drift; these fail the
//! build instead.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::affected::{Repo, declared_modules, relative_to};
use crate::census::{KnownIssues, ki_in};
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
    problems.extend(per_file_targets(&targets));
    problems.extend(uncompiled(&targets, repo.sources()));
    problems.extend(local_helpers(repo.sources()));
    problems.extend(misnamed_fixtures(&flux_fixtures(root)?));
    let nextest = std::fs::read_to_string(root.join(".config/nextest.toml"))
        .map_err(|e| format!(".config/nextest.toml: {e}"))?;
    let known = std::fs::read_to_string(root.join("docs/known_issues.md"))
        .map_err(|e| format!("docs/known_issues.md: {e}"))?;
    problems.extend(uncited_flakes(&nextest, &KnownIssues::parse(&known)));
    for problem in &problems {
        println!("tidy: {problem}");
    }
    if problems.is_empty() {
        println!(
            "tidy: formatted, tests/ matches tests/README.md, and every known flake cites an open KI"
        );
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
    let root = root.display().to_string();
    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|file| relative_to(&root, file))
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

/// `[[test]]` targets under `tests/` other than a folder's `main.rs`: each
/// folder is one binary, so a file joins it as a module instead.
fn per_file_targets(targets: &BTreeSet<String>) -> Vec<String> {
    targets
        .iter()
        .filter(|path| path.starts_with("tests/"))
        .filter(|path| {
            let rest = path.strip_prefix("tests/").unwrap_or(path);
            !matches!(rest.split_once('/'), Some((_, "main.rs")))
        })
        .map(|path| {
            format!(
                "{path} is its own [[test]] target; declare it with `mod` in its folder's main.rs"
            )
        })
        .collect()
}

/// Test files nothing compiles: not a `[[test]]` target, not a module its
/// folder's target `main.rs` declares, and not named by a `#[path]` in
/// another test file. `support/` and `snapshots/` hold modules that are
/// included in other ways.
fn uncompiled(targets: &BTreeSet<String>, sources: &[(String, String)]) -> Vec<String> {
    let mut included = BTreeSet::new();
    for (path, text) in sources {
        let dir = Path::new(path).parent().unwrap_or(Path::new(""));
        for target in path_attributes(text) {
            included.insert(normalize(&dir.join(target)));
        }
    }
    // `mod x;` in a target `main.rs`, or in a `mod.rs` that is itself
    // compiled, compiles `x.rs` or `x/mod.rs` beside it. Repeat until a pass
    // adds nothing, since a nested `mod.rs` is only compiled once found.
    loop {
        let mut found = Vec::new();
        for (path, text) in sources {
            let declares = (path.ends_with("/main.rs") && targets.contains(path))
                || (path.ends_with("/mod.rs") && included.contains(path));
            if !declares {
                continue;
            }
            let dir = Path::new(path).parent().unwrap_or(Path::new(""));
            for module in declared_modules(text) {
                found.push(normalize(&dir.join(format!("{module}.rs"))));
                found.push(normalize(&dir.join(module).join("mod.rs")));
            }
        }
        let before = included.len();
        included.extend(found);
        if included.len() == before {
            break;
        }
    }
    sources
        .iter()
        .map(|(path, _)| path)
        .filter(|path| !path.starts_with("tests/support/") && !path.starts_with("tests/snapshots/"))
        .filter(|path| !targets.contains(*path) && !included.contains(*path))
        .map(|path| {
            format!(
                "{path} is never compiled; declare it with `mod` in the main.rs or mod.rs above it"
            )
        })
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

/// Every `.flx` file under `examples/` and `tests/`, with its text.
fn flux_fixtures(root: &Path) -> Result<Vec<(String, String)>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if path.extension().is_some_and(|ext| ext == "flx") {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let relative = path.strip_prefix(root).unwrap_or(&path);
                out.push((normalize(relative), text));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for dir in ["examples", "tests"] {
        walk(root, &root.join(dir), &mut out)?;
    }
    out.sort();
    Ok(out)
}

/// A fixture whose name ends in an error code, such as `…_e421.flx`, must list
/// that code in an `// expect-error:` line or a `//~` annotation. Readers trust the name, and KI-137
/// found twenty fixtures where the reported code had moved on without it.
fn misnamed_fixtures(fixtures: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (path, text) in fixtures {
        let Some(stem) = path.strip_suffix(".flx") else { continue };
        let Some((_, digits)) = stem.rsplit_once("_e") else { continue };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let code = format!("E{digits}");
        let expected = text.lines().any(|line| {
            let header = line
                .strip_prefix("//")
                .and_then(|rest| rest.trim_start().strip_prefix("expect-error:"));
            let annotation =
                line.find("//~").map(|at| line[at + 3..].trim_start_matches(['^', 'v']));
            header.or(annotation).is_some_and(|codes| codes.split([',', ' ']).any(|c| c == code))
        });
        if !expected {
            problems.push(format!(
                "{path} is named for {code}, but no `// expect-error:` line or `//~` annotation lists it; rename the file or fix the header"
            ));
        }
    }
    problems
}

/// A nextest override that lets a pass on a retry pass the run (`flaky-result
/// = "pass"`) must cite a KI that is open, as Go's `SkipFlaky` takes an issue
/// and GHC's `fragile` a ticket: otherwise a flake stays hidden with nothing
/// tracking it, or after its KI is closed.
fn uncited_flakes(nextest: &str, known: &KnownIssues) -> Vec<String> {
    // Each table starts at its `[…]` header and runs to the next one.
    let mut tables: Vec<(usize, Vec<&str>)> = Vec::new();
    for (number, line) in nextest.lines().enumerate() {
        if line.trim_start().starts_with('[') {
            tables.push((number + 1, Vec::new()));
        }
        if let Some((_, lines)) = tables.last_mut() {
            lines.push(line);
        }
    }
    let lets_flakes_pass = |lines: &[&str]| {
        lines.iter().any(|line| {
            let setting = line.split('#').next().unwrap_or_default();
            setting.replace(' ', "") == "flaky-result=\"pass\""
        })
    };
    tables
        .into_iter()
        .filter(|(_, lines)| lines[0].contains(".overrides]]") && lets_flakes_pass(lines))
        .filter_map(|(number, lines)| match lines.iter().find_map(|line| ki_in(line)) {
            Some(id) if known.get(&id).is_some_and(|issue| issue.open) => None,
            Some(id) => Some(format!(
                ".config/nextest.toml:{number}: a known-flaky override cites {id}, which is not open in docs/known_issues.md; remove the override or cite the open KI"
            )),
            None => Some(format!(
                ".config/nextest.toml:{number}: a known-flaky override (`flaky-result = \"pass\"`) cites no KI; file one and name it in a comment"
            )),
        })
        .collect()
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
    fn a_file_is_compiled_as_a_target_a_declared_module_or_through_a_path_attribute() {
        let targets: BTreeSet<String> =
            ["tests/vm_runtime/a_tests.rs".to_string(), "tests/lexer/main.rs".to_string()].into();
        let files = sources(&[
            ("tests/vm_runtime/a_tests.rs", "#[path = \"../shared/b.rs\"]\nmod b;"),
            ("tests/shared/b.rs", ""),
            ("tests/support/scratch.rs", ""),
            ("tests/vm_runtime/forgotten.rs", "#[test] fn t() {}"),
            ("tests/lexer/main.rs", "mod token_tests;\nmod big;"),
            ("tests/lexer/token_tests.rs", ""),
            ("tests/lexer/undeclared.rs", ""),
            ("tests/lexer/big/mod.rs", "mod part;"),
            ("tests/lexer/big/part.rs", ""),
            ("tests/lexer/big/stray.rs", ""),
        ]);
        assert_eq!(
            uncompiled(&targets, &files),
            [
                "tests/vm_runtime/forgotten.rs is never compiled; declare it with `mod` in the main.rs or mod.rs above it",
                "tests/lexer/undeclared.rs is never compiled; declare it with `mod` in the main.rs or mod.rs above it",
                "tests/lexer/big/stray.rs is never compiled; declare it with `mod` in the main.rs or mod.rs above it",
            ]
        );
    }

    #[test]
    fn only_a_folders_main_rs_is_a_test_target() {
        let targets: BTreeSet<String> = [
            "tests/lexer/main.rs".to_string(),
            "tests/lexer/token_tests.rs".to_string(),
            "tests/support/main.rs".to_string(),
            "crates/x/tests/a.rs".to_string(),
        ]
        .into();
        assert_eq!(
            per_file_targets(&targets),
            [
                "tests/lexer/token_tests.rs is its own [[test]] target; declare it with `mod` in its folder's main.rs"
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

    #[test]
    fn a_fixture_named_for_a_code_must_expect_it() {
        let files = sources(&[
            ("examples/a/ok_e300.flx", "// expect: compile_error\n// expect-error: E300\n"),
            ("examples/a/annotated_e004.flx", "// expect: compile_error\nlet x = y //~ E004\n"),
            ("examples/a/drifted_e421.flx", "// expect-error: E300\n"),
            ("examples/a/no_header_e406.flx", "fn main() {}\n"),
            ("examples/a/not_a_code_else.flx", ""),
            ("examples/a/plain.flx", ""),
        ]);
        let problems = misnamed_fixtures(&files);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("examples/a/drifted_e421.flx is named for E421"));
        assert!(problems[1].starts_with("examples/a/no_header_e406.flx is named for E406"));
    }

    #[test]
    fn a_known_flake_must_cite_an_open_ki() {
        let known = KnownIssues::parse(
            "## Open\n\n### KI-144 — open one\n\n### KI-120 — FIXED 2026-10-01\n\n## Resolved\n\n### KI-010 — old\n",
        );
        let nextest = r#"[profile.ci]
retries = 1
flaky-result = "fail"

# A commented example is not an override:
# flaky-result = "pass"

[[profile.ci.overrides]]
# KI-144: cited and open
filter = "test(=a)"
flaky-result = "pass"

[[profile.ci.overrides]]
# KI-120: fixed
filter = "test(=b)"
flaky-result = "pass"

[[profile.ci.overrides]]
filter = "test(=c)"
flaky-result = "pass" # no KI

[[profile.ci.overrides]]
# KI-010
filter = "test(=d)"
slow-timeout = "60s"
"#;
        let problems = uncited_flakes(nextest, &known);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].starts_with(".config/nextest.toml:13: a known-flaky override cites KI-120")
        );
        assert!(problems[1].starts_with(".config/nextest.toml:18: a known-flaky override"));
        assert!(problems[1].contains("cites no KI"));
    }
}
