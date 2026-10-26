//! Picks the tests a change is likely to break, for `cargo xtask affected` and
//! `cargo xtask phase` (TESTING.md).
//!
//! The compiler is one crate, so cargo's dependency graph ties every test to
//! every source file. The phase map below is a judgement, not dependency-sound;
//! `cargo test-all` is the gate.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::proc::run;
use crate::verify::cargo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Parse,
    Ast,
    Typeck,
    Effects,
    Typeclasses,
    Core,
    Aether,
    Bytecode,
    Vm,
    Llvm,
    Debug,
    Diagnostics,
    Examples,
    Integration,
    Flume,
}

impl Phase {
    pub const ALL: [Phase; 15] = [
        Phase::Parse,
        Phase::Ast,
        Phase::Typeck,
        Phase::Effects,
        Phase::Typeclasses,
        Phase::Core,
        Phase::Aether,
        Phase::Bytecode,
        Phase::Vm,
        Phase::Llvm,
        Phase::Debug,
        Phase::Diagnostics,
        Phase::Examples,
        Phase::Integration,
        Phase::Flume,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Phase::Parse => "parse",
            Phase::Ast => "ast",
            Phase::Typeck => "typeck",
            Phase::Effects => "effects",
            Phase::Typeclasses => "typeclasses",
            Phase::Core => "core",
            Phase::Aether => "aether",
            Phase::Bytecode => "bytecode",
            Phase::Vm => "vm",
            Phase::Llvm => "llvm",
            Phase::Debug => "debug",
            Phase::Diagnostics => "diagnostics",
            Phase::Examples => "examples",
            Phase::Integration => "integration",
            Phase::Flume => "flume",
        }
    }

    pub fn parse(name: &str) -> Option<Phase> {
        Phase::ALL.into_iter().find(|phase| phase.name() == name)
    }

    /// Integration-test directories under `tests/`.
    fn test_dirs(self) -> &'static [&'static str] {
        match self {
            Phase::Parse => &["lexer", "parser"],
            Phase::Ast => &["ast_passes"],
            Phase::Typeck => &["type_inference"],
            Phase::Core => &["core_ir"],
            Phase::Aether => &["aether"],
            Phase::Bytecode => &["bytecode"],
            Phase::Vm => &["vm_runtime"],
            Phase::Llvm => &["native_llvm"],
            Phase::Debug => &["debug", "debuginfo"],
            Phase::Diagnostics => &["diagnostics", "error_fixtures"],
            Phase::Examples => &["examples"],
            Phase::Integration => &["integration"],
            Phase::Flume => &["flume"],
            // Selected by name instead; see `Selection::add_phase`.
            Phase::Effects | Phase::Typeclasses => &[],
        }
    }

    /// Module prefixes of the unit tests in `src/`.
    fn unit_modules(self) -> &'static [&'static str] {
        match self {
            Phase::Parse => &["syntax"],
            Phase::Ast => &["ast"],
            Phase::Typeck => &["types"],
            Phase::Core => &["core"],
            Phase::Aether => &["aether"],
            Phase::Bytecode => &["cfg", "bytecode"],
            Phase::Vm => &[
                "vm",
                "runtime",
                "driver::run_program::backend::vm",
                "driver::pipeline::vm",
            ],
            Phase::Llvm => &[
                "lir",
                "llvm",
                "driver::run_program::backend::native",
                "driver::pipeline::native",
            ],
            Phase::Debug => &["debuginfo", "debug_protocol"],
            Phase::Diagnostics => &["diagnostics"],
            Phase::Effects
            | Phase::Typeclasses
            | Phase::Examples
            | Phase::Integration
            | Phase::Flume => &[],
        }
    }

    /// Its tests are gated on `cfg(feature = "llvm")`. `--all-features` also
    /// shares `cargo test-all`'s build.
    fn needs_llvm(self) -> bool {
        matches!(self, Phase::Llvm | Phase::Debug)
    }
}

pub fn phase_names() -> Vec<&'static str> {
    Phase::ALL.map(Phase::name).to_vec()
}

/// What the selection rules need to know about the checkout.
#[derive(Default)]
pub struct Repo {
    /// `flux`'s integration-test binaries and their sources, relative to the
    /// root. Read from cargo metadata: the `[[test]]` names keep old flat
    /// filenames that don't match their files.
    tests: Vec<(String, String)>,
    /// The other workspace members and their directories, ending in `/`.
    members: Vec<(String, String)>,
    /// Every `.rs` file under `tests/`, with its text.
    sources: Vec<(String, String)>,
}

impl Repo {
    pub fn load(root: &Path) -> Result<Repo, String> {
        let output = cargo()
            .current_dir(root)
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .output()
            .map_err(|e| format!("cargo metadata: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo metadata failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let metadata: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("cargo metadata output: {e}"))?;
        // Cargo reports canonical paths, which differ from `root` when the
        // checkout is reached through a symlink.
        let workspace_root = metadata
            .get("workspace_root")
            .and_then(Value::as_str)
            .ok_or("cargo metadata has no workspace_root")?;
        let prefix = format!("{workspace_root}/");
        let relative = |path: &str| path.strip_prefix(&prefix).unwrap_or(path).to_string();
        let mut repo = Repo::default();
        for package in metadata
            .get("packages")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            let name = package.get("name").and_then(Value::as_str).unwrap_or("");
            let manifest = package
                .get("manifest_path")
                .and_then(Value::as_str)
                .unwrap_or("");
            let dir = relative(manifest.strip_suffix("Cargo.toml").unwrap_or(manifest));
            if name != "flux" {
                if !dir.is_empty() {
                    repo.members.push((name.to_string(), dir));
                }
                continue;
            }
            for target in package
                .get("targets")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
            {
                let is_test = target
                    .get("kind")
                    .and_then(Value::as_array)
                    .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some("test")));
                if !is_test {
                    continue;
                }
                let (Some(name), Some(path)) = (
                    target.get("name").and_then(Value::as_str),
                    target.get("src_path").and_then(Value::as_str),
                ) else {
                    continue;
                };
                repo.tests.push((name.to_string(), relative(path)));
            }
        }
        collect_sources(root, Path::new("tests"), &mut repo.sources);
        Ok(repo)
    }

    fn binaries_in_test_dir(&self, dir: &str) -> impl Iterator<Item = &str> {
        let prefix = format!("tests/{dir}/");
        self.tests
            .iter()
            .filter(move |(_, path)| path.starts_with(&prefix))
            .map(|(name, _)| name.as_str())
    }

    fn binary_for_source(&self, path: &str) -> Option<&str> {
        self.tests
            .iter()
            .find(|(_, source)| source == path)
            .map(|(name, _)| name.as_str())
    }

    /// The deepest other workspace member whose directory holds `path`.
    fn member_for_path(&self, path: &str) -> Option<&str> {
        self.members
            .iter()
            .filter(|(_, dir)| path.starts_with(dir.as_str()))
            .max_by_key(|(_, dir)| dir.len())
            .map(|(name, _)| name.as_str())
    }

    /// Test sources that mention `needle`. A helper under `tests/support/` is
    /// replaced by the test files that include it.
    fn sources_mentioning(&self, needle: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        for (path, text) in &self.sources {
            if !text.contains(needle) {
                continue;
            }
            match path.strip_prefix("tests/support/") {
                Some(helper) => found.extend(self.sources_mentioning(&format!("support/{helper}"))),
                None => {
                    found.insert(path.clone());
                }
            }
        }
        found
    }
}

fn collect_sources(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = dir.join(entry.file_name());
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_sources(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && let Ok(text) = std::fs::read_to_string(root.join(&path))
        {
            out.push((path.to_string_lossy().replace('\\', "/"), text));
        }
    }
}

/// The phases of a path under `src/` or `runtime/`. Empty for code every
/// phase uses (the pipeline driver, shared types, the CLI), which means the
/// whole fast tier.
fn phases_for_src_path(path: &str) -> &'static [Phase] {
    let under = |dirs: &[&str]| dirs.iter().any(|dir| path.starts_with(dir));
    if under(&["src/syntax/"]) {
        &[Phase::Parse]
    } else if under(&["src/ast/"]) {
        &[Phase::Ast]
    } else if under(&["src/types/"]) {
        &[Phase::Typeck]
    } else if under(&["src/core/"]) {
        &[Phase::Core]
    } else if under(&["src/aether/"]) {
        &[Phase::Aether]
    } else if under(&["src/cfg/", "src/bytecode/"]) {
        &[Phase::Bytecode]
    } else if under(&[
        "src/vm/",
        "src/driver/run_program/backend/vm.rs",
        "src/driver/pipeline/vm/",
    ]) {
        &[Phase::Vm]
    } else if under(&["src/runtime/"]) {
        // Both backends use it.
        &[Phase::Vm, Phase::Llvm]
    } else if under(&[
        "src/lir/",
        "src/llvm/",
        "src/parity/",
        "runtime/",
        "src/driver/run_program/backend/native.rs",
        "src/driver/pipeline/native/",
    ]) {
        &[Phase::Llvm]
    } else if under(&["src/debuginfo/"]) || path == "src/debug_protocol.rs" {
        &[Phase::Debug]
    } else if under(&["src/diagnostics/"]) {
        &[Phase::Diagnostics]
    } else {
        &[]
    }
}

/// Test directories whose binaries are gated on `cfg(feature = "llvm")`.
fn source_needs_llvm(path: &str) -> bool {
    ["tests/native_llvm/", "tests/debug/", "tests/debuginfo/"]
        .iter()
        .any(|dir| path.starts_with(dir))
}

/// The tests to run as nextest filtersets, or why to run them all.
#[derive(Debug, Default)]
pub struct Selection {
    filters: BTreeSet<String>,
    all_features: bool,
    workspace: bool,
    fallback: Option<String>,
    notes: Vec<String>,
}

impl Selection {
    fn add_phase(&mut self, repo: &Repo, phase: Phase) {
        match phase {
            // These cut across every directory.
            Phase::Effects => {
                self.filters
                    .insert("binary(/effect/) | test(/effect/)".to_string());
            }
            Phase::Typeclasses => {
                self.filters
                    .insert("binary(/class/) | test(/class/)".to_string());
            }
            _ => {
                for dir in phase.test_dirs() {
                    for binary in repo.binaries_in_test_dir(dir) {
                        self.filters.insert(format!("binary_id(=flux::{binary})"));
                    }
                }
                for module in phase.unit_modules() {
                    self.filters
                        .insert(format!("(package(flux) & kind(lib) & test(/^{module}::/))"));
                }
            }
        }
        self.all_features |= phase.needs_llvm();
    }

    fn add_sources(&mut self, repo: &Repo, sources: &BTreeSet<String>) {
        for source in sources {
            if let Some(binary) = repo.binary_for_source(source) {
                self.filters.insert(format!("binary_id(=flux::{binary})"));
                self.all_features |= source_needs_llvm(source);
            }
        }
    }

    /// A fixture, snapshot or other non-Rust input under `tests/` or
    /// `examples/`. Selects the tests that name the file, else the ones that
    /// name its directory.
    fn add_fixture(&mut self, repo: &Repo, path: &str) {
        let file_name = path.rsplit('/').next().unwrap_or(path);
        let stem = file_name.split('.').next().unwrap_or(file_name);
        let sources = if let Some(rel) = path.strip_prefix("examples/") {
            if let Some((dir, _)) = rel.split_once('/') {
                // One test function per top-level examples/ directory.
                self.filters.insert(format!(
                    "(binary_id(=flux::examples_fixtures_snapshots) & test(=examples_{}))",
                    snake_case(dir)
                ));
                repo.sources_mentioning(&format!("examples/{dir}"))
            } else {
                repo.sources_mentioning(path)
            }
        } else {
            let rel = path.strip_prefix("tests/").unwrap_or(path);
            let top = rel.split('/').next().unwrap_or(rel);
            let mut sources = repo.sources_mentioning(&format!("\"{stem}"));
            sources.extend(repo.sources_mentioning(&format!("/{stem}")));
            // parity-check isn't part of the nextest tiers.
            if sources.is_empty() && top == "parity" {
                self.notes.push(format!(
                    "{path} runs only under parity-check: cargo run -- parity-check {path}"
                ));
                return;
            }
            let mut dir = Path::new(rel).parent();
            while sources.is_empty()
                && let Some(d) = dir.filter(|d| !d.as_os_str().is_empty())
            {
                sources = repo.sources_mentioning(&d.to_string_lossy());
                dir = d.parent();
            }
            self.all_features |= matches!(top, "parity" | "parity_debug");
            if sources.is_empty() {
                self.fallback = Some(format!("no test mentions {path}"));
                return;
            }
            sources
        };
        self.add_sources(repo, &sources);
    }

    /// The `cargo` arguments that run the selection: `cargo test-fast` for a fallback.
    pub fn cargo_args(&self, extra: &[String]) -> Vec<String> {
        let mut args: Vec<String> = if self.fallback.is_some() {
            vec!["test-fast".to_string()]
        } else {
            let mut args = vec!["nextest".to_string(), "run".to_string()];
            // Without --workspace, `flux` keeps the same build as `cargo test-fast`.
            if self.workspace {
                args.push("--workspace".to_string());
            }
            if self.all_features {
                args.push("--all-features".to_string());
            }
            args.push("-E".to_string());
            args.push(
                self.filters
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
            args
        };
        args.extend(extra.iter().cloned());
        args
    }

    pub fn is_empty(&self) -> bool {
        self.fallback.is_none() && self.filters.is_empty()
    }
}

/// The selection for changed paths, relative to the root.
pub fn select_for_paths(repo: &Repo, paths: &[String]) -> Selection {
    let mut selection = Selection::default();
    let full_fast_tier = ["Cargo.toml", "Cargo.lock", "build.rs"];
    for path in paths {
        let path = path.as_str();
        let under = |dirs: &[&str]| dirs.iter().any(|dir| path.starts_with(dir));
        if path.ends_with(".md")
            || under(&[
                "docs/",
                "editors/",
                "benchmarks/",
                "benches/",
                ".github/",
                ".forgejo/",
                "scripts/",
            ])
        {
            continue;
        }
        if full_fast_tier.contains(&path) || under(&[".cargo/", ".config/", "lib/"]) {
            // Manifests and the Flux stdlib reach every test.
            selection.fallback = Some(format!("{path} reaches every test"));
        } else if under(&["src/", "runtime/"]) {
            let phases = phases_for_src_path(path);
            if phases.is_empty() {
                selection.fallback = Some(format!("{path} feeds every phase"));
            }
            for &phase in phases {
                selection.add_phase(repo, phase);
            }
        } else if let Some(helper) = path.strip_prefix("tests/support/") {
            let includers = repo.sources_mentioning(&format!("support/{helper}"));
            selection.add_sources(repo, &includers);
        } else if path.starts_with("tests/") && repo.binary_for_source(path).is_some() {
            selection.add_sources(repo, &BTreeSet::from([path.to_string()]));
        } else if under(&["tests/", "examples/"]) {
            selection.add_fixture(repo, path);
        } else if let Some(member) = repo.member_for_path(path) {
            selection.filters.insert(format!("rdeps({member})"));
            selection.workspace = true;
        } else {
            let sources = repo.sources_mentioning(path);
            if sources.is_empty() {
                selection.fallback = Some(format!("no rule for {path}"));
            }
            selection.add_sources(repo, &sources);
        }
    }
    selection
}

/// Fails on a name that isn't a phase.
pub fn select_for_phases(repo: &Repo, names: &[String]) -> Result<Selection, String> {
    let mut selection = Selection::default();
    for name in names {
        let phase = Phase::parse(name)
            .ok_or_else(|| format!("unknown phase {name} (phases: {})", phase_names().join(" ")))?;
        selection.add_phase(repo, phase);
    }
    Ok(selection)
}

/// Staged, unstaged and untracked files, plus everything committed since
/// `base` when given.
pub fn changed_paths(root: &Path, base: Option<&str>) -> Result<Vec<String>, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "git {} failed:\n{}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let mut text = git(&["diff", "--name-only", "HEAD"])?;
    text += &git(&["ls-files", "--others", "--exclude-standard"])?;
    if let Some(base) = base {
        text += &git(&["diff", "--name-only", &format!("{base}...HEAD")])?;
    }
    let paths: BTreeSet<String> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    Ok(paths.into_iter().collect())
}

/// Prints the notes, then runs the selection, or prints its command with
/// `dry_run`. `Ok(false)` means tests ran and failed.
pub fn run_selection(
    root: &Path,
    selection: &Selection,
    extra: &[String],
    dry_run: bool,
) -> Result<bool, String> {
    for note in &selection.notes {
        eprintln!("xtask: {note}");
    }
    if let Some(reason) = &selection.fallback {
        eprintln!("xtask: falling back to the whole fast tier: {reason}");
    } else if selection.is_empty() {
        eprintln!("xtask: no changed file selects a test");
        return Ok(true);
    }
    let args = selection.cargo_args(extra);
    if dry_run {
        println!(
            "cargo {}",
            args.iter()
                .map(|a| shell_quote(a))
                .collect::<Vec<_>>()
                .join(" ")
        );
        return Ok(true);
    }
    let code = run(cargo().current_dir(root).args(&args)).map_err(|e| format!("cargo: {e}"))?;
    Ok(code == 0)
}

fn shell_quote(arg: &str) -> String {
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_=./:".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// `ModuleGraph` becomes `module_graph`, matching the examples test functions.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() && previous_lower {
            out.push('_');
        }
        previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Repo {
        let owned = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        Repo {
            tests: owned(&[
                ("llvm_codegen", "tests/native_llvm/llvm_codegen.rs"),
                (
                    "type_inference_tests",
                    "tests/type_inference/type_inference_tests.rs",
                ),
                (
                    "examples_fixtures_snapshots",
                    "tests/examples/examples_fixtures_snapshots.rs",
                ),
                (
                    "effect_runtime_parity_tests",
                    "tests/native_llvm/effect_runtime_parity_tests.rs",
                ),
                ("aether_cli_snapshots", "tests/aether/cli_snapshots.rs"),
                (
                    "type_semantics_matrix_tests",
                    "tests/type_inference/type_semantics_matrix_tests.rs",
                ),
                (
                    "native_debug_tests",
                    "tests/native_llvm/native_debug_tests.rs",
                ),
            ]),
            members: owned(&[("flux-lsp", "crates/flux-lsp/"), ("xtask", "xtask/")]),
            sources: owned(&[
                (
                    "tests/native_llvm/effect_runtime_parity_tests.rs",
                    r#"run("tests/parity/fs_async.flx")"#,
                ),
                (
                    "tests/aether/cli_snapshots.rs",
                    r#"snapshot_path => "../snapshots/aether""#,
                ),
                ("tests/support/semantic_infer.rs", "pub fn infer() {}"),
                (
                    "tests/type_inference/type_semantics_matrix_tests.rs",
                    r#"#[path = "../support/semantic_infer.rs"]"#,
                ),
                (
                    "tests/native_llvm/native_debug_tests.rs",
                    "tools/lldb/flux_lldb.py",
                ),
            ]),
        }
    }

    fn select(paths: &[&str]) -> Selection {
        let paths: Vec<String> = paths.iter().map(ToString::to_string).collect();
        select_for_paths(&repo(), &paths)
    }

    fn filters(selection: &Selection) -> Vec<&str> {
        selection.filters.iter().map(String::as_str).collect()
    }

    #[test]
    fn a_backend_file_selects_the_native_tests_with_all_features() {
        let selection = select(&["src/llvm/pipeline.rs"]);
        assert!(selection.all_features);
        assert_eq!(
            filters(&selection),
            [
                "(package(flux) & kind(lib) & test(/^driver::pipeline::native::/))",
                "(package(flux) & kind(lib) & test(/^driver::run_program::backend::native::/))",
                "(package(flux) & kind(lib) & test(/^lir::/))",
                "(package(flux) & kind(lib) & test(/^llvm::/))",
                "binary_id(=flux::effect_runtime_parity_tests)",
                "binary_id(=flux::llvm_codegen)",
                "binary_id(=flux::native_debug_tests)",
            ]
        );
    }

    #[test]
    fn the_native_driver_selects_the_native_tests_not_the_fast_tier() {
        let selection = select(&["src/driver/run_program/backend/native.rs"]);
        assert_eq!(selection.fallback, None);
        assert!(selection.all_features);
        assert!(filters(&selection).contains(&"binary_id(=flux::llvm_codegen)"));
        assert!(filters(&selection).contains(
            &"(package(flux) & kind(lib) & test(/^driver::run_program::backend::native::/))"
        ));
    }

    #[test]
    fn an_inference_file_selects_its_phase_with_default_features() {
        let selection = select(&["src/types/scheme.rs"]);
        assert!(!selection.all_features);
        assert_eq!(
            filters(&selection),
            [
                "(package(flux) & kind(lib) & test(/^types::/))",
                "binary_id(=flux::type_inference_tests)",
                "binary_id(=flux::type_semantics_matrix_tests)",
            ]
        );
    }

    #[test]
    fn an_example_selects_its_directorys_test_function() {
        let selection = select(&["examples/ModuleGraph/main.flx"]);
        assert_eq!(
            filters(&selection),
            ["(binary_id(=flux::examples_fixtures_snapshots) & test(=examples_module_graph))"]
        );
    }

    #[test]
    fn a_parity_fixture_selects_the_test_that_names_it() {
        let selection = select(&["tests/parity/fs_async.flx"]);
        assert_eq!(
            filters(&selection),
            ["binary_id(=flux::effect_runtime_parity_tests)"]
        );
        assert!(selection.all_features);
    }

    #[test]
    fn a_parity_fixture_no_test_names_points_at_parity_check() {
        let selection = select(&["tests/parity/adt_either_match.flx"]);
        assert!(selection.is_empty());
        assert_eq!(selection.notes.len(), 1);
        assert!(selection.notes[0].contains("parity-check tests/parity/adt_either_match.flx"));
    }

    #[test]
    fn a_snapshot_selects_the_test_that_names_its_directory() {
        let selection = select(&["tests/snapshots/aether/aether__x.snap"]);
        assert_eq!(
            filters(&selection),
            ["binary_id(=flux::aether_cli_snapshots)"]
        );
    }

    #[test]
    fn a_support_file_selects_the_tests_that_include_it() {
        let selection = select(&["tests/support/semantic_infer.rs"]);
        assert_eq!(
            filters(&selection),
            ["binary_id(=flux::type_semantics_matrix_tests)"]
        );
    }

    #[test]
    fn another_member_selects_its_reverse_dependencies_across_the_workspace() {
        let selection = select(&["crates/flux-lsp/src/main.rs"]);
        assert_eq!(filters(&selection), ["rdeps(flux-lsp)"]);
        assert!(
            selection
                .cargo_args(&[])
                .contains(&"--workspace".to_string())
        );
    }

    #[test]
    fn a_tool_file_selects_the_tests_that_name_it() {
        let selection = select(&["tools/lldb/flux_lldb.py"]);
        assert_eq!(
            filters(&selection),
            ["binary_id(=flux::native_debug_tests)"]
        );
    }

    #[test]
    fn docs_select_nothing() {
        assert!(select(&["docs/guide/01.md", "README.md"]).is_empty());
    }

    #[test]
    fn shared_code_falls_back_to_the_fast_tier() {
        let selection = select(&["src/shared/cache_paths.rs", "src/types/scheme.rs"]);
        assert_eq!(
            selection.fallback.as_deref(),
            Some("src/shared/cache_paths.rs feeds every phase")
        );
        assert_eq!(
            selection.cargo_args(&["--no-fail-fast".to_string()]),
            ["test-fast", "--no-fail-fast"]
        );
    }

    #[test]
    fn an_unknown_phase_is_an_error() {
        assert!(select_for_phases(&repo(), &["typo".to_string()]).is_err());
        let selection = select_for_phases(&repo(), &["effects".to_string()]).unwrap();
        assert_eq!(filters(&selection), ["binary(/effect/) | test(/effect/)"]);
    }

    #[test]
    fn every_phase_name_parses_back() {
        for phase in Phase::ALL {
            assert_eq!(Phase::parse(phase.name()), Some(phase));
        }
    }

    #[test]
    fn snake_case_matches_the_test_function_names() {
        assert_eq!(snake_case("ModuleGraph"), "module_graph");
        assert_eq!(snake_case("guide_type_system"), "guide_type_system");
        assert_eq!(snake_case("Modules"), "modules");
    }
}
