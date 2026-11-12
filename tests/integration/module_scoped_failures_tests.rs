//! Proposal 0203: a module that fails to parse is skipped together with the
//! modules that import it, and every other module is still type-checked. Only
//! a broken module graph stops the whole compile. Each test writes a small
//! program of several modules and runs the real `flux` CLI on it, because the
//! snapshot harnesses keep their own copy of the driver's loop.

use crate::support::flux_runner;
use crate::support::scratch;
use flux_runner::{Ran, flux_file};
use scratch::Scratch;

const TYPE_BAD: &str = "module TypeBad {
    public fn bad() -> Int {
        let x: Bool = 42
        1
    }
}
";

const PARSE_BAD: &str = "module ParseBad {
    public fn p() -> Int {
        let s = \"unterminated
    }
}
";

const USES_PARSE_BAD: &str = "import ParseBad

module UsesParseBad {
    public fn u() -> Int {
        let y: Bool = 7
        1
    }
}
";

/// Writes `modules` and `entry` into one scratch directory and runs `flux`
/// on the entry with that directory as the module root.
fn run(tag: &str, modules: &[(&str, &str)], entry: &str, native: bool) -> Ran {
    let scratch = Scratch::new(tag);
    for (name, source) in modules {
        scratch.write(name, source);
    }
    let entry_path = scratch.write("main.flx", entry);
    let entry_path = entry_path.to_string_lossy().into_owned();
    let root = scratch.path().to_string_lossy().into_owned();
    let run = flux_file(&entry_path).args(&["--root", &root]).env("NO_COLOR", "1");
    if native { run.native().run() } else { run.run() }
}

/// The error codes in `stderr`, each with the file its diagnostic names,
/// sorted: the aggregator orders files by path, which these tests don't check.
fn reported(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut code: Option<String> = None;
    for line in stderr.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("error[") {
            code = rest.split(']').next().map(str::to_string);
        } else if let Some(c) = code.take() {
            match line.split(".flx:").next() {
                Some(file) if line.contains(".flx:") => {
                    let file = file.rsplit('/').next().unwrap_or(file);
                    out.push(format!("{c} {file}"));
                }
                _ => code = Some(c),
            }
        }
    }
    out.sort();
    out
}

fn assert_failed_without_running(ran: &Ran) {
    assert!(!ran.success, "the compile should fail:\n{}", ran.stderr);
    assert!(!ran.stdout.contains("ran"), "the program ran:\n{}", ran.stdout);
}

#[test]
fn an_entry_parse_error_does_not_hide_an_independent_import_type_error() {
    let ran = run(
        "msf-entry-parse",
        &[("TypeBad.flx", TYPE_BAD)],
        "import TypeBad\n\nfn main() with IO {\n    let s = \"unterminated\n}\n",
        false,
    );
    assert_failed_without_running(&ran);
    assert_eq!(reported(&ran.stderr), ["E071 main", "E300 TypeBad"], "{}", ran.stderr);
}

#[test]
fn an_import_parse_error_does_not_hide_an_independent_import_type_error() {
    let ran = run(
        "msf-import-parse",
        &[("ParseBad.flx", PARSE_BAD), ("TypeBad.flx", TYPE_BAD)],
        "import ParseBad\nimport TypeBad\n\nfn main() with IO {\n    print(\"ran\")\n}\n",
        false,
    );
    assert_failed_without_running(&ran);
    assert_eq!(reported(&ran.stderr), ["E071 ParseBad", "E300 TypeBad"], "{}", ran.stderr);
    assert!(ran.stderr.contains("Module Skipped"), "{}", ran.stderr);
}

#[test]
fn a_module_importing_one_with_a_parse_error_is_skipped_unchecked() {
    let ran = run(
        "msf-dependent",
        &[("ParseBad.flx", PARSE_BAD), ("UsesParseBad.flx", USES_PARSE_BAD)],
        "import UsesParseBad\n\nfn main() with IO {\n    print(\"ran\")\n}\n",
        false,
    );
    assert_failed_without_running(&ran);
    // UsesParseBad's own E300 is not looked for: its import failed.
    assert_eq!(reported(&ran.stderr), ["E071 ParseBad"], "{}", ran.stderr);
    assert!(
        ran.stderr.contains("because its dependency `ParseBad` already has errors"),
        "{}",
        ran.stderr
    );
}

#[test]
fn an_entry_importing_flow_primops_does_not_hide_an_independent_import_type_error() {
    let ran = run(
        "msf-entry-primops",
        &[("TypeBad.flx", TYPE_BAD)],
        "import Flow.Primops\nimport TypeBad\n\nfn main() with IO {\n    print(\"ran\")\n}\n",
        false,
    );
    assert_failed_without_running(&ran);
    assert_eq!(reported(&ran.stderr), ["E083 main", "E300 TypeBad"], "{}", ran.stderr);
}

#[test]
fn an_unresolved_import_still_stops_the_whole_compile() {
    let ran = run(
        "msf-unresolved",
        &[("TypeBad.flx", TYPE_BAD)],
        "import NoSuchModule\nimport TypeBad\n\nfn main() with IO {\n    print(\"ran\")\n}\n",
        false,
    );
    assert_failed_without_running(&ran);
    let codes = reported(&ran.stderr);
    assert!(codes.iter().any(|c| c.starts_with("E018")), "{}", ran.stderr);
    assert!(!codes.iter().any(|c| c.starts_with("E300")), "{}", ran.stderr);
}

#[cfg(feature = "llvm")]
#[test]
fn natively_an_import_parse_error_does_not_hide_an_independent_import_type_error() {
    let ran = run(
        "msf-import-parse-native",
        &[("ParseBad.flx", PARSE_BAD), ("TypeBad.flx", TYPE_BAD)],
        "import ParseBad\nimport TypeBad\n\nfn main() with IO {\n    print(\"ran\")\n}\n",
        true,
    );
    assert_failed_without_running(&ran);
    assert_eq!(reported(&ran.stderr), ["E071 ParseBad", "E300 TypeBad"], "{}", ran.stderr);
}
