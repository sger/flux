//! Native debug builds (proposal 0188 S5): DWARF line tables that lldb and
//! Xcode read.
//!
//! Each fixture in `tests/native_llvm/debug/` is built with
//! `--native -g -o <binary>`. Then:
//! - `llvm-dwarfdump --verify` must find no errors in the binary's debug
//!   information (the `.dSYM` on macOS);
//! - lldb, run in batch mode, must break at a Flux line, show Flux functions
//!   in the backtrace, and step.
//!
//! lldb's exact output differs between versions, so the tests look for the
//! lines that matter rather than snapshot whole sessions. A machine without
//! lldb or llvm-dwarfdump skips those checks with a note; CI installs both.

#![cfg(all(feature = "llvm", unix))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native_llvm/debug")
}

/// Copies fixture `name` (a file, or a directory of modules) into scratch.
fn prepare(name: &str) -> Scratch {
    let scratch = Scratch::new(&format!("native-debug-{}", name.replace('/', "-")));
    copy_dir(&fixture_dir(), scratch.path());
    scratch
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Builds `program` with `-g` into `binary` and returns the build's output.
fn build(scratch: &Scratch, program: &str, binary: &str) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(scratch.path())
        .args([program, "--native", "-g", "-o", binary])
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    assert!(
        output.status.success(),
        "flux --native -g failed for {program}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// A tool from `env`, else the first of `names` on PATH, else (on macOS)
/// through `xcrun`.
fn find_tool(env: &str, names: &[&str]) -> Option<Vec<String>> {
    if let Some(path) = std::env::var_os(env) {
        return Some(vec![path.to_string_lossy().into_owned()]);
    }
    for name in names {
        if Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Some(vec![name.to_string()]);
        }
    }
    if cfg!(target_os = "macos") {
        let name = names[0];
        let found = Command::new("xcrun")
            .args(["--find", name])
            .output()
            .is_ok_and(|o| o.status.success());
        if found {
            return Some(vec!["xcrun".to_string(), name.to_string()]);
        }
    }
    None
}

fn dwarfdump() -> Option<Vec<String>> {
    find_tool(
        "FLUX_DWARFDUMP",
        &["llvm-dwarfdump", "llvm-dwarfdump-18", "llvm-dwarfdump-19"],
    )
}

fn lldb() -> Option<Vec<String>> {
    find_tool("FLUX_LLDB", &["lldb", "lldb-18", "lldb-19"])
}

fn run_tool(tool: &[String], args: &[&str], cwd: &Path) -> String {
    let output = Command::new(&tool[0])
        .args(&tool[1..])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|err| panic!("cannot run {tool:?}: {err}"));
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Where the debug information of `binary` is: its `.dSYM` on macOS.
fn debug_file(scratch: &Scratch, binary: &str) -> PathBuf {
    let binary = scratch.path().join(binary);
    if cfg!(target_os = "macos") {
        let dsym = binary.with_extension("dSYM");
        let dsym = if dsym.exists() {
            dsym
        } else {
            PathBuf::from(format!("{}.dSYM", binary.display()))
        };
        assert!(dsym.exists(), "no .dSYM next to {}", binary.display());
        dsym
    } else {
        binary
    }
}

const FIXTURES: &[&str] = &[
    "calls.flx",
    "lambdas.flx",
    "matches.flx",
    "effects.flx",
    "classes.flx",
    "async_nap.flx",
    "deep.flx",
    "twomods/main.flx",
];

/// The errors `llvm-dwarfdump --verify` reports in Flux's compile units.
///
/// A binary also carries the DWARF of the runtime it links, C and Rust,
/// which the Flux compiler does not produce and these tests do not judge:
/// rustc's DWARF for the runtime can have overlapping ranges of its own.
/// The verifier names the unit it is checking before its errors, so each
/// error is charged to the unit last named.
fn flux_unit_errors(verify: &str) -> Vec<String> {
    let mut unit = String::new();
    let mut errors = Vec::new();
    for line in verify.lines() {
        if let Some(rest) = line.strip_prefix("Verifying unit: ") {
            unit = rest.to_string();
        } else if line.starts_with("Verifying ") {
            unit.clear();
        } else if line.starts_with("error:")
            && !line.contains("Aggregated error counts")
            && !line.contains("occurred")
            && (unit.contains(".flx") || line.contains(".flx"))
        {
            errors.push(format!("{unit}: {line}"));
        }
    }
    errors
}

#[test]
fn debug_builds_have_valid_dwarf_with_flux_lines() {
    let Some(dwarfdump) = dwarfdump() else {
        eprintln!("note: llvm-dwarfdump not found; skipping (set FLUX_DWARFDUMP)");
        return;
    };
    // Verifying also walks the runtime's own DWARF, which takes a while:
    // the fixtures run side by side.
    std::thread::scope(|threads| {
        for fixture in FIXTURES {
            let dwarfdump = &dwarfdump;
            threads.spawn(move || check_dwarf(dwarfdump, fixture));
        }
    });
}

fn check_dwarf(dwarfdump: &[String], fixture: &str) {
    let scratch = prepare(fixture);
    build(&scratch, fixture, "app");
    let debug = debug_file(&scratch, "app");
    let debug = debug.to_string_lossy();
    let verify = run_tool(dwarfdump, &["--verify", &debug], scratch.path());
    let errors = flux_unit_errors(&verify);
    assert!(
        errors.is_empty(),
        "llvm-dwarfdump --verify found errors in the Flux units of {fixture}:\n{}",
        errors.join("\n")
    );
    let lines = run_tool(dwarfdump, &["--debug-line", &debug], scratch.path());
    let file = Path::new(fixture).file_name().unwrap().to_string_lossy();
    assert!(
        lines.contains(&*file),
        "{fixture} is not in the line table:\n{}",
        &lines[..lines.len().min(4000)]
    );
}

#[test]
fn every_module_gets_its_own_compile_unit() {
    let Some(dwarfdump) = dwarfdump() else {
        eprintln!("note: llvm-dwarfdump not found; skipping (set FLUX_DWARFDUMP)");
        return;
    };
    let scratch = prepare("twomods");
    build(&scratch, "twomods/main.flx", "app");
    let debug = debug_file(&scratch, "app");
    let info = run_tool(
        &dwarfdump,
        &["--debug-info", &debug.to_string_lossy()],
        scratch.path(),
    );
    assert!(
        info.contains("\"main.flx\""),
        "{}",
        &info[..info.len().min(4000)]
    );
    assert!(
        info.contains("\"Shapes.flx\""),
        "{}",
        &info[..info.len().min(4000)]
    );
    assert!(
        info.contains("\"double\""),
        "{}",
        &info[..info.len().min(4000)]
    );
}

/// Runs lldb in batch mode on `binary` with `commands`.
fn lldb_session(lldb: &[String], scratch: &Scratch, binary: &str, commands: &[&str]) -> String {
    let mut args: Vec<String> = vec!["-b".to_string(), "--no-lldbinit".to_string()];
    for command in commands {
        args.push("-o".to_string());
        args.push(command.to_string());
    }
    args.push(format!("./{binary}"));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_tool(lldb, &args, scratch.path())
}

/// The `frame #N: … <function> at <file>:<line>` lines of a session.
fn frames(session: &str) -> Vec<String> {
    session
        .lines()
        .filter(|line| line.contains("frame #"))
        .map(|line| line.trim().to_string())
        .collect()
}

#[test]
fn lldb_breaks_at_a_flux_line_and_shows_flux_frames() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("calls");
    build(&scratch, "calls.flx", "app");
    let session = lldb_session(
        &lldb,
        &scratch,
        "app",
        &["breakpoint set --file calls.flx --line 3", "run", "bt"],
    );
    let frames = frames(&session);
    let shown = |function: &str, line: &str| {
        frames
            .iter()
            .any(|frame| frame.contains(function) && frame.contains(line))
    };
    assert!(shown("square", "calls.flx:3"), "{session}");
    assert!(shown("sum_squares", "calls.flx:8"), "{session}");
    assert!(shown("main", "calls.flx:14"), "{session}");
}

#[test]
fn lldb_steps_over_and_into_flux_lines() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("calls");
    build(&scratch, "calls.flx", "app");
    let session = lldb_session(
        &lldb,
        &scratch,
        "app",
        &[
            "breakpoint set --file calls.flx --line 8",
            "run",
            "thread step-over",
            "frame info",
            "thread step-in",
            "frame info",
        ],
    );
    let infos: Vec<&str> = session
        .lines()
        .filter(|line| line.starts_with("frame #0"))
        .collect();
    assert_eq!(infos.len(), 2, "{session}");
    assert!(
        infos[0].contains("sum_squares") && infos[0].contains("calls.flx:9"),
        "step-over should move to the next line:\n{session}"
    );
    assert!(
        infos[1].contains("square") && infos[1].contains("calls.flx:"),
        "step-in should enter `square`:\n{session}"
    );
}

#[test]
fn lldb_names_a_lambda_by_where_it_is() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("lambdas");
    build(&scratch, "lambdas.flx", "app");
    let session = lldb_session(
        &lldb,
        &scratch,
        "app",
        &["breakpoint set --file lambdas.flx --line 5", "run", "bt"],
    );
    assert!(
        frames(&session)
            .iter()
            .any(|frame| frame.contains("<lambda lambdas.flx:4:")),
        "{session}"
    );
}

#[test]
fn deep_tail_recursion_runs_in_a_debug_build() {
    let scratch = prepare("deep");
    build(&scratch, "deep.flx", "app");
    let output = Command::new(scratch.path().join("app"))
        .output()
        .expect("run the debug build");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1000000");
}

/// The LLVM IR of a module, compiled at `level`.
fn module_ir(source: &str, level: flux::debuginfo::DebugLevel) -> String {
    use flux::compiler::Compiler;
    use flux::syntax::{lexer::Lexer, parser::Parser};
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("app.flx", interner);
    compiler.set_debug_level(level);
    let module = compiler
        .lower_to_lir_llvm_module_per_module(&program, false, false, true)
        .expect("the module compiles");
    flux::llvm::render_module(&module)
}

#[test]
fn the_ir_of_a_debug_build_locates_every_instruction() {
    let source = std::fs::read_to_string(fixture_dir().join("calls.flx")).unwrap();
    let ir = module_ir(&source, flux::debuginfo::DebugLevel::Full);
    assert!(ir.contains("!llvm.dbg.cu = !{!0}"), "{ir}");
    assert!(ir.contains("source_filename = \"app.flx\""), "{ir}");
    assert!(ir.contains("DISubprogram(name: \"square\""), "{ir}");
    assert!(ir.contains("DISubprogram(name: \"sum_squares\""), "{ir}");
    // In a function with a subprogram, every instruction has a location:
    // LLVM rejects a call without one.
    let mut in_debug_function = false;
    for line in ir.lines() {
        if line.starts_with("define ") {
            in_debug_function = line.contains("!dbg !");
        } else if line == "}" {
            in_debug_function = false;
        } else if in_debug_function && line.starts_with("  ") && !line.trim().is_empty() {
            assert!(line.contains(", !dbg !"), "no location on: {line}");
        }
    }
}

#[test]
fn without_a_debug_level_the_ir_has_no_debug_information() {
    let source = std::fs::read_to_string(fixture_dir().join("calls.flx")).unwrap();
    let ir = module_ir(&source, flux::debuginfo::DebugLevel::None);
    assert!(!ir.contains("!dbg"), "{ir}");
    assert!(!ir.contains("llvm.dbg.cu"), "{ir}");
    assert!(!ir.contains("frame-pointer"), "{ir}");
}
