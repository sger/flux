//! Fixture metadata parsing for parity corpus files.
//!
//! Reads inline metadata from comment headers in `.flx` fixture files:
//!
//! ```text
//! // parity: vm, llvm
//! // expect: success
//! // expect-error: E300
//! // parity-expected-stderr-begin
//! // error[E300]: type mismatch
//! // parity-expected-stderr-end
//! // skip: reason to skip this fixture
//! // skip-way: llvm, llvm_strict: KI-091 a known divergence on these ways
//! // requires: unix        (or `windows`) — skip elsewhere, with a reason
//! // requires: network     — skip unless the run allows it (`--allow network`)
//! // census-skip: reason    — leave the file out of `--census` runs only
//! // stdout: varies         — output depends on the clock or the host; compare exits only
//! ```
//!
//! A `parity.dir` file gives the same directives, one per line and without the
//! `//`, to every fixture in its folder and below; a fixture's own header
//! wins. A line `name.flx: <directive>` applies to that one file, for fixtures
//! whose line numbers cannot move (snapshots, debugger scripts):
//!
//! ```text
//! # examples/diagnostics/parity.dir
//! expect: any
//! hint_demos/math_module/Math.flx: expect: success
//! // bug: description of the bug shape
//! ```

use std::path::Path;

use super::Way;

/// Expected outcome for a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Success,
    CompileError,
    RuntimeError,
    /// A diagnostic demo: it may compile, or fail at compile or run time,
    /// but every way must do the same. Its messages are pinned elsewhere, by
    /// snapshots, so only the ways' agreement is checked here.
    Any,
}

impl Expect {
    /// The header spelling.
    pub fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::CompileError => "compile_error",
            Self::RuntimeError => "runtime_error",
            Self::Any => "any",
        }
    }
}

/// A way a fixture is known to diverge on, and why. The reason names the
/// open known issue (`KI-nnn`) that tracks it, so no skip is anonymous and
/// each one is removed with the bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipWay {
    pub way: Way,
    pub reason: String,
}

/// Parsed metadata from a parity fixture file.
#[derive(Debug)]
pub struct FixtureMeta {
    /// Which ways to compare (defaults to `[Vm, Llvm]`).
    pub ways: Vec<Way>,
    /// Whether a `// parity:` line chose `ways`, rather than the default.
    pub declared_ways: bool,
    /// Expected outcome (defaults to `Success`).
    pub expect: Expect,
    /// Expected diagnostic error codes for error fixtures.
    pub expected_errors: Vec<String>,
    /// Extra CLI args forwarded only for this fixture.
    pub extra_args: Vec<String>,
    /// One-line description of the bug shape.
    pub bug: Option<String>,
    /// Optional reason to skip this fixture while still reporting it.
    pub skip: Option<String>,
    /// Ways removed from this fixture's run, from `// skip-way:`. Unlike
    /// `// parity:`, they hold when `--ways` is given.
    pub skip_ways: Vec<SkipWay>,
    /// Why `--census` leaves this fixture out (`// census-skip:`): it is run
    /// by its own harness, such as a debugger or replay test.
    pub census_skip: Option<String>,
    /// `// stdout: varies`: the program prints the time, timings or the host's
    /// core count, so only its ways' exits are compared.
    pub stdout_varies: bool,
    /// Header lines that could not be understood. A fixture with any fails
    /// instead of running, so a typo cannot silently widen or narrow a run.
    pub meta_errors: Vec<String>,
    /// Platform this fixture requires, verbatim from `// requires:`.
    ///
    /// A fixture that spawns `/bin/echo`, or asserts that reading a directory
    /// gives `IsADirectory`, is making a claim about the host OS rather than
    /// about Flux. Declaring that here keeps the coverage where it is real and
    /// turns the other platforms into a reported `SKIP` rather than a failure
    /// nobody can act on. An unmet requirement is folded into [`Self::skip`],
    /// so the reporter needs no special case.
    pub requires: Option<String>,
    /// Optional expected normalized stdout block.
    pub expected_stdout: Option<String>,
    /// Optional expected normalized stderr block.
    pub expected_stderr: Option<String>,
}

impl Default for FixtureMeta {
    fn default() -> Self {
        Self {
            ways: vec![Way::Vm, Way::Llvm],
            declared_ways: false,
            expect: Expect::Success,
            expected_errors: Vec::new(),
            extra_args: Vec::new(),
            bug: None,
            skip: None,
            skip_ways: Vec::new(),
            census_skip: None,
            stdout_varies: false,
            meta_errors: Vec::new(),
            requires: None,
            expected_stdout: None,
            expected_stderr: None,
        }
    }
}

/// Parse fixture metadata from inline comments at the top of a `.flx` file.
///
/// Returns `FixtureMeta::default()` if no metadata comments are found.
pub fn parse_fixture_meta(path: &Path) -> FixtureMeta {
    parse_fixture_meta_allowing(path, &[])
}

/// [`parse_fixture_meta`], with the `// requires:` resources this run may use
/// (`network`) in `allowed`.
pub fn parse_fixture_meta_allowing(path: &Path, allowed: &[String]) -> FixtureMeta {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return FixtureMeta::default(),
    };

    let mut meta = directory_defaults(path);
    let mut collecting_block: Option<ExpectedBlockKind> = None;
    let mut expected_block_lines: Vec<String> = Vec::new();

    let mut in_header = true;

    for line in content.lines() {
        let trimmed = line.trim();

        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            in_header = false;
        }

        let Some(comment) = trimmed.strip_prefix("//") else {
            continue;
        };
        let comment_trimmed = comment.trim();

        if let Some(kind) = collecting_block {
            if kind.ends_at(comment_trimmed) {
                let value = expected_block_lines.join("\n").trim().to_string();
                match kind {
                    ExpectedBlockKind::Stdout => meta.expected_stdout = Some(value),
                    ExpectedBlockKind::Stderr => meta.expected_stderr = Some(value),
                }
                collecting_block = None;
                expected_block_lines.clear();
                continue;
            }
            // Preserve internal whitespace for multi-line strings in expected
            // output. We only strip the `// ` prefix (one leading space), not
            // the rest of the content.
            let content_line = comment.strip_prefix(' ').unwrap_or(comment).to_string();
            expected_block_lines.push(content_line);
            continue;
        }
        let comment = comment_trimmed;

        if comment == "parity-expected-stdout-begin" || comment == "parity-oracle-stdout-begin" {
            collecting_block = Some(ExpectedBlockKind::Stdout);
            continue;
        }

        if comment == "parity-expected-stderr-begin" {
            collecting_block = Some(ExpectedBlockKind::Stderr);
            continue;
        }

        if !in_header {
            continue;
        }

        apply_directive(&mut meta, comment);
    }

    if let Some(kind) = collecting_block {
        let value = expected_block_lines.join("\n").trim().to_string();
        match kind {
            ExpectedBlockKind::Stdout => meta.expected_stdout = Some(value),
            ExpectedBlockKind::Stderr => meta.expected_stderr = Some(value),
        }
    }

    apply_platform_requirement(&mut meta, allowed);
    meta
}

/// Applies one header directive (the text after `//`) to `meta`. Text that is
/// not a directive is ignored: headers are also prose.
fn apply_directive(meta: &mut FixtureMeta, comment: &str) {
    if let Some(value) = comment.strip_prefix("parity:") {
        let value = value.trim();
        let ways: Vec<Way> = value
            .split(',')
            .filter_map(|s| Way::parse(s.trim()))
            .collect();
        if !ways.is_empty() {
            meta.ways = ways;
            meta.declared_ways = true;
        }
    } else if let Some(value) = comment.strip_prefix("expect:") {
        let value = value.trim();
        meta.expect = match value {
            "success" => Expect::Success,
            "compile_error" => Expect::CompileError,
            "runtime_error" => Expect::RuntimeError,
            "any" => Expect::Any,
            _ => Expect::Success,
        };
    } else if let Some(code) = comment.strip_prefix("expect-error:") {
        let code = code.trim();
        if code.is_empty() {
            return;
        }
        meta.expected_errors.push(code.to_string());
    } else if let Some(value) = comment.strip_prefix("root:") {
        let value = value.trim();
        if !value.is_empty() {
            meta.extra_args.push("--root".to_string());
            meta.extra_args.push(value.to_string());
        }
    } else if let Some(value) = comment.strip_prefix("bug:") {
        meta.bug = Some(value.trim().to_string());
    } else if let Some(value) = comment.strip_prefix("requires:") {
        let value = value.trim();
        if !value.is_empty() {
            meta.requires = Some(value.to_string());
        }
    } else if let Some(value) = comment.strip_prefix("skip-way:") {
        match parse_skip_way(value) {
            Ok(skips) => meta.skip_ways.extend(skips),
            Err(error) => meta
                .meta_errors
                .push(format!("`// skip-way:{value}`: {error}")),
        }
    } else if let Some(value) = comment.strip_prefix("stdout:") {
        match value.trim() {
            "varies" => meta.stdout_varies = true,
            other => meta
                .meta_errors
                .push(format!("`// stdout: {other}`: the only value is `varies`")),
        }
    } else if let Some(value) = comment.strip_prefix("census-skip:") {
        meta.census_skip = Some(value.trim().to_string());
    } else if let Some(value) = comment.strip_prefix("skip:") {
        let value = value.trim();
        meta.skip = Some(if value.is_empty() {
            "fixture metadata requested skip".to_string()
        } else {
            value.to_string()
        });
    }
}

/// The name of the folder-wide defaults file.
pub const DIRECTORY_FILE: &str = "parity.dir";

/// What the `parity.dir` files around `path` declare for it, outermost first
/// so a nearer folder wins; [`FixtureMeta::default`] where none says anything.
fn directory_defaults(path: &Path) -> FixtureMeta {
    let mut meta = FixtureMeta::default();
    let mut folders: Vec<&Path> = path.ancestors().skip(1).collect();
    // Stop at the repository root, the folder with `Cargo.toml`.
    if let Some(root) = folders
        .iter()
        .position(|dir| dir.join("Cargo.toml").is_file())
    {
        folders.truncate(root + 1);
    }
    for folder in folders.iter().rev() {
        let Ok(text) = std::fs::read_to_string(folder.join(DIRECTORY_FILE)) else {
            continue;
        };
        let relative = path.strip_prefix(folder).ok();
        apply_directory_file(&mut meta, &text, relative);
    }
    meta
}

/// Applies a `parity.dir` file's lines to `meta`, for the fixture at
/// `relative` (its path from the file's folder).
fn apply_directory_file(meta: &mut FixtureMeta, text: &str, relative: Option<&Path>) {
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let scoped = line
            .split_once(".flx:")
            .map(|(name, directive)| (format!("{name}.flx"), directive.trim()));
        match scoped {
            Some((name, directive)) => {
                if relative.is_some_and(|rel| rel == Path::new(&name)) {
                    apply_directive(meta, directive);
                }
            }
            None => apply_directive(meta, line),
        }
    }
}

/// Parses the value of `// skip-way: <way>[, <way>…]: <reason>`.
///
/// The reason must name a known issue (`KI-nnn`): a skip is a recorded,
/// tracked divergence, not a way to make a run quiet.
fn parse_skip_way(value: &str) -> Result<Vec<SkipWay>, String> {
    let Some((ways, reason)) = value.split_once(':') else {
        return Err("expected `<way>[, <way>…]: <reason>`".to_string());
    };
    let reason = reason.trim();
    if !reason.contains("KI-") {
        return Err("the reason must name a known issue, as `KI-nnn`".to_string());
    }
    ways.split(',')
        .map(str::trim)
        .map(|name| {
            Way::parse(name)
                .map(|way| SkipWay {
                    way,
                    reason: reason.to_string(),
                })
                .ok_or_else(|| format!("unknown way `{name}`"))
        })
        .collect()
}

/// Turn an unmet `// requires:` into a skip, leaving an explicit `// skip:` alone.
///
/// An unrecognised platform skips too, and says so. Silently ignoring a typo
/// would run the fixture on the platform it declared it cannot run on, which is
/// the failure this directive exists to prevent; skipping loudly costs coverage
/// on one platform and is visible in the `SKIP` line.
fn apply_platform_requirement(meta: &mut FixtureMeta, allowed: &[String]) {
    let Some(requires) = meta.requires.as_deref() else {
        return;
    };
    if meta.skip.is_some() {
        return;
    }
    meta.skip = match requires {
        "unix" => (!cfg!(unix)).then(|| "fixture requires a unix host".to_string()),
        "windows" => (!cfg!(windows)).then(|| "fixture requires a windows host".to_string()),
        "network" => (!allowed.iter().any(|a| a == "network"))
            .then(|| "fixture requires network access (--allow network)".to_string()),
        other => Some(format!(
            "unrecognised `requires: {other}` (expected `unix`, `windows` or `network`)"
        )),
    };
}

#[derive(Debug, Clone, Copy)]
enum ExpectedBlockKind {
    Stdout,
    Stderr,
}

impl ExpectedBlockKind {
    fn ends_at(self, comment: &str) -> bool {
        match self {
            Self::Stdout => {
                comment == "parity-expected-stdout-end" || comment == "parity-oracle-stdout-end"
            }
            Self::Stderr => comment == "parity-expected-stderr-end",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_full_metadata() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_fixture.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// parity: vm, llvm").unwrap();
        writeln!(f, "// expect: runtime_error").unwrap();
        writeln!(f, "// expect-error: E1004").unwrap();
        writeln!(f, "// bug: division by zero differs across backends").unwrap();
        writeln!(f, "// skip: temporary backend gap").unwrap();
        writeln!(f, "// parity-expected-stdout-begin").unwrap();
        writeln!(f, "// line 1").unwrap();
        writeln!(f, "// line 2").unwrap();
        writeln!(f, "// parity-expected-stdout-end").unwrap();
        writeln!(f, "// parity-expected-stderr-begin").unwrap();
        writeln!(f, "// error[E1004]: boundary violation").unwrap();
        writeln!(f, "//   label").unwrap();
        writeln!(f, "// parity-expected-stderr-end").unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert!(meta.declared_ways);
        assert_eq!(meta.expect, Expect::RuntimeError);
        assert_eq!(meta.expected_errors, vec!["E1004"]);
        assert_eq!(meta.extra_args, Vec::<String>::new());
        assert_eq!(
            meta.bug.as_deref(),
            Some("division by zero differs across backends")
        );
        assert_eq!(meta.skip.as_deref(), Some("temporary backend gap"));
        assert_eq!(meta.expected_stdout.as_deref(), Some("line 1\nline 2"));
        assert_eq!(
            meta.expected_stderr.as_deref(),
            Some("error[E1004]: boundary violation\n  label")
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn requires_matching_platform_does_not_skip() {
        let mut meta = FixtureMeta {
            requires: Some(if cfg!(windows) { "windows" } else { "unix" }.to_string()),
            ..Default::default()
        };
        apply_platform_requirement(&mut meta, &[]);
        assert_eq!(meta.skip, None);
    }

    #[test]
    fn requires_other_platform_skips_with_a_reason() {
        let mut meta = FixtureMeta {
            requires: Some(if cfg!(windows) { "unix" } else { "windows" }.to_string()),
            ..Default::default()
        };
        apply_platform_requirement(&mut meta, &[]);
        assert!(
            meta.skip
                .as_deref()
                .is_some_and(|r| r.contains("requires a")),
            "expected a platform skip reason, got {:?}",
            meta.skip
        );
    }

    #[test]
    fn unrecognised_requires_skips_rather_than_running() {
        // A typo must not run the fixture on the platform it declared it
        // cannot run on. Skipping is visible in the SKIP line; ignoring is not.
        let mut meta = FixtureMeta {
            requires: Some("unixx".to_string()),
            ..Default::default()
        };
        apply_platform_requirement(&mut meta, &[]);
        assert!(
            meta.skip.as_deref().is_some_and(|r| r.contains("unixx")),
            "expected the typo to be named, got {:?}",
            meta.skip
        );
    }

    #[test]
    fn a_directory_file_gives_defaults_and_names_single_files() {
        let mut meta = FixtureMeta::default();
        let text = "# demos\nexpect: any\n\nsub/b.flx: expect: runtime_error\na.flx: census-skip: run by its harness\n";
        apply_directory_file(&mut meta, text, Some(Path::new("a.flx")));
        assert_eq!(meta.expect, Expect::Any);
        assert_eq!(meta.census_skip.as_deref(), Some("run by its harness"));
        let mut meta = FixtureMeta::default();
        apply_directory_file(&mut meta, text, Some(Path::new("sub/b.flx")));
        assert_eq!(meta.expect, Expect::RuntimeError);
        assert_eq!(meta.census_skip, None);
        let mut meta = FixtureMeta::default();
        apply_directory_file(
            &mut meta,
            "x.flx: stdout: varies\ny.flx: stdout: maybe\n",
            Some(Path::new("x.flx")),
        );
        assert!(meta.stdout_varies && meta.meta_errors.is_empty());
    }

    #[test]
    fn a_fixture_header_wins_over_its_folder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_dir_test");
        let _ = std::fs::create_dir_all(dir.join("inner"));
        std::fs::write(dir.join(DIRECTORY_FILE), "expect: any\nskip: outer\n").unwrap();
        std::fs::write(dir.join("inner").join(DIRECTORY_FILE), "skip: inner\n").unwrap();
        let fixture = dir.join("inner").join("x.flx");
        std::fs::write(&fixture, "// expect: compile_error\nfn main() { 1 }\n").unwrap();
        let meta = parse_fixture_meta(&fixture);
        assert_eq!(meta.expect, Expect::CompileError);
        assert_eq!(meta.skip.as_deref(), Some("inner"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_is_skipped_unless_allowed() {
        let network = || FixtureMeta {
            requires: Some("network".to_string()),
            ..Default::default()
        };
        let mut meta = network();
        apply_platform_requirement(&mut meta, &[]);
        assert!(
            meta.skip
                .as_deref()
                .is_some_and(|r| r.contains("--allow network"))
        );
        let mut meta = network();
        apply_platform_requirement(&mut meta, &["network".to_string()]);
        assert_eq!(meta.skip, None);
    }

    #[test]
    fn an_explicit_skip_survives_a_met_requirement() {
        let mut meta = FixtureMeta {
            requires: Some(if cfg!(windows) { "windows" } else { "unix" }.to_string()),
            skip: Some("a real backend gap".to_string()),
            ..Default::default()
        };
        apply_platform_requirement(&mut meta, &[]);
        assert_eq!(meta.skip.as_deref(), Some("a real backend gap"));
    }

    #[test]
    fn defaults_without_metadata() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_no_meta.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert!(!meta.declared_ways);
        assert_eq!(meta.expect, Expect::Success);
        assert_eq!(meta.extra_args, Vec::<String>::new());
        assert!(meta.bug.is_none());
        assert!(meta.expected_stdout.is_none());
        assert!(meta.expected_stderr.is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_expected_stdout_block_outside_header() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_expected_stdout_after_code.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn main() {{").unwrap();
        writeln!(f, "    print(1)").unwrap();
        writeln!(f, "}}").unwrap();
        writeln!(f, "// parity-expected-stdout-begin").unwrap();
        writeln!(f, "// 1").unwrap();
        writeln!(f, "// parity-expected-stdout-end").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.expected_stdout.as_deref(), Some("1"));
        assert_eq!(meta.ways, vec![Way::Vm, Way::Llvm]);
        assert_eq!(meta.expect, Expect::Success);
        assert_eq!(meta.extra_args, Vec::<String>::new());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_expected_stderr_block_outside_header() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_expected_stderr_after_code.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "fn main() {{").unwrap();
        writeln!(f, "    print(1)").unwrap();
        writeln!(f, "}}").unwrap();
        writeln!(f, "// parity-expected-stderr-begin").unwrap();
        writeln!(f, "// error[E300]: type mismatch").unwrap();
        writeln!(f, "//   at line 1").unwrap();
        writeln!(f, "// parity-expected-stderr-end").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(
            meta.expected_stderr.as_deref(),
            Some("error[E300]: type mismatch\n  at line 1")
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn skip_way_names_one_or_more_ways_and_a_ki() {
        assert_eq!(
            parse_skip_way(" llvm: KI-091 float formatting of -0.0"),
            Ok(vec![SkipWay {
                way: Way::Llvm,
                reason: "KI-091 float formatting of -0.0".to_string(),
            }])
        );
        let ways: Vec<Way> = parse_skip_way(" vm, vm_strict : KI-102 x")
            .unwrap()
            .into_iter()
            .map(|skip| skip.way)
            .collect();
        assert_eq!(ways, [Way::Vm, Way::VmStrict]);
    }

    #[test]
    fn skip_way_rejects_an_anonymous_reason_an_unknown_way_and_no_reason() {
        assert!(parse_skip_way(" llvm: flaky").unwrap_err().contains("KI-"));
        assert!(
            parse_skip_way(" llvmm: KI-091 x")
                .unwrap_err()
                .contains("llvmm")
        );
        assert!(parse_skip_way(" llvm").is_err());
    }

    #[test]
    fn skip_way_lines_accumulate_and_bad_ones_are_kept_as_errors() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_skip_way.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// skip-way: llvm: KI-091 a").unwrap();
        writeln!(f, "// skip-way: vm_strict: KI-076 b").unwrap();
        writeln!(f, "// skip-way: vm: no issue named").unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        let ways: Vec<Way> = meta.skip_ways.iter().map(|skip| skip.way).collect();
        assert_eq!(ways, [Way::Llvm, Way::VmStrict]);
        assert_eq!(meta.skip, None);
        assert_eq!(meta.meta_errors.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_fixture_root_into_extra_args() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join("flux_parity_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_root_meta.flx");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "// root: examples").unwrap();
        writeln!(f, "fn main() {{ }}").unwrap();

        let meta = parse_fixture_meta(&path);
        assert_eq!(meta.extra_args, vec!["--root", "examples"]);

        let _ = std::fs::remove_file(&path);
    }
}
