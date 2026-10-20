//! Native debug builds (proposal 0188 S5 and S6): DWARF that lldb and Xcode
//! read, and `tools/lldb/flux_lldb.py`, which shows Flux values.
//!
//! Each fixture in `tests/native_llvm/debug/` is built with
//! `--native -g -o <binary>`. Then:
//! - `llvm-dwarfdump --verify` must find no errors in the Flux debug
//!   information: the `.dSYM` on macOS, elsewhere the Flux objects the build
//!   cached;
//! - lldb, run in batch mode, must break at a Flux line, show Flux functions
//!   in the backtrace, and step;
//! - with the script loaded, `flux locals`, `flux print` and `flux bt` must
//!   show variables and frames as `flux debug` does on the VM.
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

/// Builds `program` with `-g` into `binary`, without running it: for a
/// program that ends with a runtime error.
/// Native `-g` builds this suite runs at once. Each links the Rust runtime
/// and runs `dsymutil`; cargo runs the tests on every core.
const BUILDS_AT_ONCE: usize = 2;

/// Waits for one of the suite's build slots; the slot is free again when
/// the guard is dropped.
fn build_slot() -> impl Drop {
    use std::sync::{Condvar, Mutex};
    static RUNNING: Mutex<usize> = Mutex::new(0);
    static FREED: Condvar = Condvar::new();
    struct Slot;
    impl Drop for Slot {
        fn drop(&mut self) {
            *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            FREED.notify_one();
        }
    }
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    while *running >= BUILDS_AT_ONCE {
        running = FREED.wait(running).unwrap_or_else(|e| e.into_inner());
    }
    *running += 1;
    Slot
}

fn build_only(scratch: &Scratch, program: &str, binary: &str) {
    let _slot = build_slot();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(scratch.path())
        .args([program, "--native", "-g", "--emit-binary", "-o", binary])
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    assert!(
        output.status.success(),
        "flux --native -g --emit-binary failed for {program}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Builds `program` with `-g` into `binary` and returns the build's output.
fn build(scratch: &Scratch, program: &str, binary: &str) -> Output {
    let _slot = build_slot();
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
    "values.flx",
    "twomods/main.flx",
    "adtmods/main.flx",
    "handlers.flx",
    "overflow.flx",
    "fibers.flx",
    "root_panic.flx",
    "inlined.flx",
    "refs.flx",
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
    // Verifying a dSYM also walks the runtime's own DWARF, which takes a
    // while: the fixtures run two at a time. One thread per fixture ran
    // every build and `dsymutil` at once, which exhausted a 24 GB machine.
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|threads| {
        for _ in 0..2 {
            let (dwarfdump, next) = (&dwarfdump, &next);
            threads.spawn(move || {
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(fixture) = FIXTURES.get(index) else {
                        break;
                    };
                    check_dwarf(dwarfdump, fixture);
                }
            });
        }
    });
}

/// The objects a build of `scratch` cached: the Flux modules it compiled,
/// each with its own compile unit, and the program's entry object.
fn flux_objects(scratch: &Scratch) -> Vec<PathBuf> {
    fn collect(dir: &Path, objects: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, objects);
            } else if path.extension().is_some_and(|ext| ext == "o") {
                objects.push(path);
            }
        }
    }
    let mut objects = Vec::new();
    collect(&scratch.cache_dir().join("native"), &mut objects);
    objects.sort();
    assert!(
        !objects.is_empty(),
        "no cached objects under {}",
        scratch.cache_dir().display()
    );
    objects
}

fn check_dwarf(dwarfdump: &[String], fixture: &str) {
    let scratch = prepare(fixture);
    // Built, not run: some fixtures end with a runtime error on purpose.
    build_only(&scratch, fixture, "app");
    let debug = debug_file(&scratch, "app");
    let debug = debug.to_string_lossy();
    // Outside macOS the linked binary carries the runtime's DWARF too, tens
    // of megabytes that `--verify` takes the better part of an hour over
    // and `flux_unit_errors` ignores: verify the Flux objects instead.
    let verified = if cfg!(target_os = "macos") {
        vec![PathBuf::from(&*debug)]
    } else {
        flux_objects(&scratch)
    };
    let mut errors = Vec::new();
    for path in &verified {
        let verify = run_tool(
            dwarfdump,
            &["--verify", &path.to_string_lossy()],
            scratch.path(),
        );
        let name = path.file_name().unwrap().to_string_lossy();
        errors.extend(
            flux_unit_errors(&verify)
                .into_iter()
                .map(|error| format!("{name}: {error}")),
        );
    }
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

/// Asserts the program ended with `status`. lldb on Linux at times loses a
/// threaded program as it exits, under load more often than not, and says
/// so instead of its status; then what the program printed as it ended,
/// `last_words`, shows how it did.
fn assert_exited(session: &str, status: i32, last_words: &str) {
    let lost = "exited with status = -1 (0xffffffff) lost connection";
    assert!(
        session.contains(&format!("exited with status = {status}"))
            || (session.contains(lost) && session.lines().any(|line| line == last_words)),
        "{session}"
    );
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

/// Without `-g`, a build has a line table alone, for its own panic traces
/// (0188 S10a): no variables, no types, and code as without it.
#[test]
fn without_a_debug_level_the_ir_has_lines_only() {
    let source = std::fs::read_to_string(fixture_dir().join("calls.flx")).unwrap();
    let ir = module_ir(&source, flux::debuginfo::DebugLevel::None);
    assert!(ir.contains("emissionKind: LineTablesOnly"), "{ir}");
    assert!(ir.contains("DISubprogram(name: \"square\""), "{ir}");
    assert!(!ir.contains("DILocalVariable"), "{ir}");
    assert!(!ir.contains("llvm.dbg.declare"), "{ir}");
    assert!(!ir.contains("FluxCtor"), "{ir}");
    assert!(!ir.contains("frame-pointer"), "{ir}");
}

/// `flux_lldb.py`, as the repository has it.
fn lldb_script() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tools/lldb/flux_lldb.py")
        .display()
        .to_string()
}

/// Runs lldb with `flux_lldb.py` loaded, stopped at `file:line`, then runs
/// `commands`. `None`, with a note, when this lldb runs no Python scripts.
fn flux_session(
    lldb: &[String],
    scratch: &Scratch,
    binary: &str,
    stop_at: (&str, u32),
    commands: &[&str],
) -> Option<String> {
    let import = format!("command script import {}", lldb_script());
    let breakpoint = format!("breakpoint set --file {} --line {}", stop_at.0, stop_at.1);
    let mut all = vec![import.as_str(), breakpoint.as_str(), "run"];
    all.extend_from_slice(commands);
    let session = lldb_session(lldb, scratch, binary, &all);
    if session.contains("script interpreter") || session.contains("scripting not supported") {
        eprintln!("note: this lldb runs no Python scripts; skipping");
        return None;
    }
    Some(session)
}

/// The output lines of the session's `flux` commands, without lldb's echo.
fn flux_lines(session: &str) -> Vec<String> {
    session
        .lines()
        .filter(|line| !line.starts_with("(lldb)"))
        .map(|line| line.trim_end().to_string())
        .collect()
}

fn assert_lines(session: &str, expected: &[&str]) {
    let lines = flux_lines(session);
    for line in expected {
        assert!(
            lines.iter().any(|shown| shown == line),
            "missing `{line}`:\n{session}"
        );
    }
}

/// `flux refs` reads a value's reference count from its header (0188
/// S10b). Natively `main` has passed `xs` on, so `total` holds its three
/// references: `xs`, `ys` and the tuple's.
#[test]
fn lldb_counts_references() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("refs");
    build(&scratch, "refs.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("refs.flx", 13),
        &[
            "flux refs xs",
            "flux refs s",
            "flux refs n",
            "flux refs nope",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "xs: 1 reference (unique: can be reused in place)",
            "s: 1 reference (unique: can be reused in place)",
            "n: not reference counted (Int)",
            "no variable `nope` here",
        ],
    );
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("refs.flx", 6),
        &[
            "flux refs xs",
            "flux refs ys",
            "flux refs pair",
            "flux refs pair.0",
            "flux refs pair.5",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "xs: 2 references (shared)",
            "ys: 2 references (shared)",
            "pair: 1 reference (unique: can be reused in place)",
            "pair.0: 2 references (shared)",
            "`pair.5` has no part `5`",
        ],
    );
}

#[test]
fn lldb_shows_flux_values() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("values");
    build(&scratch, "values.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("values.flx", 14),
        &[
            "flux locals",
            "flux print p",
            "flux print xs.2",
            "frame variable shape",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "n = 7",
            "name = \"flux\"",
            "flag = true",
            "ratio = 2.5",
            "xs = [1, 2, 3]",
            "pair = (9, \"x\")",
            "opt = Some(4)",
            "arr = [|1, 2|]",
            "shape = Square(3)",
            "p = Point(1, 2)",
            "label = \"flux!\"",
            "  .0 = 1",
            "  .1 = 2",
            "xs.2 = 3",
        ],
    );
    // lldb's own view of a variable uses the script's summary.
    assert!(
        session.contains("(FluxValue) shape = Square(3)"),
        "{session}"
    );
}

#[test]
fn lldb_shows_variables_once_they_are_bound() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("calls");
    build(&scratch, "calls.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("calls.flx", 8),
        &["flux locals", "thread step-over", "flux locals"],
    ) else {
        return;
    };
    // At line 8, `sa` is not bound yet; one line on, it is.
    let (before, after) = session
        .split_once("(lldb) thread step-over")
        .expect("the session stepped");
    let shown = |part: &str, line: &str| flux_lines(part).iter().any(|shown| shown == line);
    let named = |part: &str, name: &str| {
        flux_lines(part)
            .iter()
            .any(|shown| shown.starts_with(&format!("{name} = ")))
    };
    assert!(
        shown(before, "a = 3") && shown(before, "b = 4"),
        "{session}"
    );
    assert!(!named(before, "sa"), "{session}");
    assert!(shown(after, "sa = 9"), "{session}");
}

#[test]
fn lldb_shows_captured_variables_and_local_function_names() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("lambdas");
    build(&scratch, "lambdas.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("lambdas.flx", 5),
        &["flux locals", "flux bt"],
    ) else {
        return;
    };
    assert_lines(&session, &["x = 1", "offset = 10 (captured)"]);
    let lines = flux_lines(&session);
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("#0 <lambda lambdas.flx:4:34> at lambdas.flx:5:")),
        "{session}"
    );
    // A local recursive function has its own name, as on the VM.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("#1 map_acc at List.flx:")),
        "{session}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("#2 main at lambdas.flx:4:")),
        "{session}"
    );
}

#[test]
fn lldb_shows_the_binders_of_the_arm_that_matched() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("matches");
    build(&scratch, "matches.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("matches.flx", 10),
        &["flux locals"],
    ) else {
        return;
    };
    // Only the `Rect` arm stops here, not the `Circle` arm before it.
    assert_lines(&session, &["shape = Rect(3, 4)", "w = 3", "h = 4"]);
    assert!(!session.contains("r = "), "{session}");
}

#[test]
fn lldb_names_constructors_another_module_declares() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("adtmods");
    build(&scratch, "adtmods/main.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("main.flx", 6),
        &["flux locals", "flux bt"],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "shape = Rect(3, 4)",
            "w = 3",
            "#0 size at main.flx:6:23",
            "#1 main at main.flx:13:11",
        ],
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_dsym_carries_the_lldb_script() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("values");
    build(&scratch, "values.flx", "app");
    let script = scratch
        .path()
        .join("app.dSYM/Contents/Resources/Python/app.py");
    assert_eq!(
        std::fs::read_to_string(&script).expect("the dSYM has the script"),
        flux::debuginfo::LLDB_SCRIPT
    );
    // lldb loads it from there once allowed to, before it loads the target.
    let session = run_tool(
        &lldb,
        &[
            "-b",
            "--no-lldbinit",
            "-O",
            "settings set target.load-script-from-symbol-file true",
            "-o",
            "breakpoint set --file values.flx --line 14",
            "-o",
            "run",
            "-o",
            "flux locals",
            "./app",
        ],
        scratch.path(),
    );
    assert_lines(&session, &["shape = Square(3)"]);
}

#[test]
fn flux_debug_prints_the_lldb_script() {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(["debug", "--lldb-script"])
        .output()
        .expect("run flux");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        flux::debuginfo::LLDB_SCRIPT
    );
}

#[test]
fn the_lldb_script_is_valid_python() {
    let compiled = Command::new("python3")
        .args(["-m", "py_compile", &lldb_script()])
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output();
    match compiled {
        Ok(output) => assert!(output.status.success(), "{output:?}"),
        Err(_) => eprintln!("note: python3 not found; skipping"),
    }
}

#[test]
fn a_debug_build_describes_variables_and_constructors() {
    let source = std::fs::read_to_string(fixture_dir().join("values.flx")).unwrap();
    let ir = module_ir(&source, flux::debuginfo::DebugLevel::Full);
    assert!(ir.contains("emissionKind: FullDebug"), "{ir}");
    assert!(ir.contains("call void @llvm.dbg.declare("), "{ir}");
    assert!(
        ir.contains("!DILocalVariable(name: \"ratio\", arg: 4,"),
        "{ir}"
    );
    assert!(ir.contains("!DILocalVariable(name: \"label\","), "{ir}");
    assert!(ir.contains("!DIEnumerator(name: \"Square\""), "{ir}");
    assert!(ir.contains("name: \"FluxCtor.Point\""), "{ir}");
    assert!(
        ir.contains("!DIDerivedType(tag: DW_TAG_member, name: \"x\""),
        "{ir}"
    );
    // No store goes between a tail call and its return: the call would
    // stop being a tail call.
    let lines: Vec<&str> = ir.lines().collect();
    for pair in lines.windows(2) {
        if pair[0].contains(" = tail call ") {
            assert!(
                !pair[1].trim_start().starts_with("store"),
                "a store follows a tail call:\n{}\n{}",
                pair[0],
                pair[1]
            );
        }
    }
}

// ── Effects and runtime errors (0188 S7) ─────────────────────────────────

#[test]
fn a_handler_arm_shows_the_frames_it_suspended_and_the_handlers() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("handlers");
    build(&scratch, "handlers.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("handlers.flx", 25),
        &["flux bt", "flux handlers", "flux where"],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "#0 <handler Flip.flip> at handlers.flx:25:29",
            "   \u{27c2} handling Flip.flip — performed at handlers.flx:17:13",
            "#1 decide at handlers.flx:17:13   (suspended)",
            "#2 main at handlers.flx:24:9",
            "#0 Flip",
            "#1 Counter state = 5",
            "stopped: breakpoint at handlers.flx:25:29 in <handler Flip.flip>",
        ],
    );
}

#[test]
fn more_frames_than_the_runtime_keeps_in_one_continuation() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("overflow");
    build(&scratch, "overflow.flx", "app");
    let Some(session) = flux_session(&lldb, &scratch, "app", ("overflow.flx", 49), &["flux bt"])
    else {
        return;
    };
    // `resume` holds a composed closure: an array of continuations, the
    // first eight composed into one.
    let suspended: Vec<String> = flux_lines(&session)
        .into_iter()
        .filter(|line| line.ends_with("(suspended)"))
        .collect();
    let names: Vec<&str> = suspended
        .iter()
        .filter_map(|line| line.split(' ').nth(1))
        .collect();
    assert_eq!(
        names,
        [
            "level10", "level9", "level8", "level7", "level6", "level5", "level4", "level3",
            "level2", "level1"
        ],
        "{session}"
    );
}

#[test]
fn stepping_across_a_perform_follows_the_handler() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("handlers");
    build(&scratch, "handlers.flx", "app");
    // Over a perform: the next line of the same call, after the handler
    // resumed it.
    let Some(over) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("handlers.flx", 17),
        &["flux next", "flux where"],
    ) else {
        return;
    };
    assert_lines(&over, &["stopped: step at handlers.flx:18:13 in decide"]);
    // Into a perform: the handler's arm; then into `resume`: the frame it
    // resumes, above the arm.
    let Some(into) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("handlers.flx", 17),
        &["flux step", "flux where", "flux step", "flux bt"],
    ) else {
        return;
    };
    assert_lines(
        &into,
        &[
            "stopped: step at handlers.flx:25:29 in <handler Flip.flip>",
            "#0 decide at handlers.flx:18:13",
            "#1 <handler Flip.flip> at handlers.flx:25:29",
        ],
    );
}

/// Runs `commands` in lldb on `binary` with the lldb script, every fiber
/// on one worker as under `flux debug`; None when lldb runs no Python.
fn fiber_session(
    lldb: &[String],
    scratch: &Scratch,
    binary: &str,
    commands: &[&str],
) -> Option<String> {
    let import = format!("command script import {}", lldb_script());
    let mut all = vec![
        import.as_str(),
        "settings set target.env-vars FLUX_WORKERS=1",
    ];
    all.extend_from_slice(commands);
    let session = lldb_session(lldb, scratch, binary, &all);
    if session.contains("script interpreter") || session.contains("scripting not supported") {
        eprintln!("note: this lldb runs no Python scripts; skipping");
        return None;
    }
    Some(session)
}

#[test]
fn flux_threads_lists_the_fibers_and_what_each_waits_for() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("fibers");
    build_only(&scratch, "fibers.flx", "app");
    let Some(session) = fiber_session(
        &lldb,
        &scratch,
        "app",
        &[
            "breakpoint set --file fibers.flx --line 13",
            "run",
            "flux where",
            "flux threads",
            "flux bt",
            "flux thread 2",
            "flux bt",
            "flux thread 1",
            "flux bt",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "stopped: breakpoint at fibers.flx:13:5 in fast (thread 3)",
            "  1 main [waiting both (threads 2, 3)] at fibers.flx:31:16",
            "  2 slow [waiting sleep 20 ms] at fibers.flx:6:5",
            "* 3 fast [stopped] at fibers.flx:13:5",
            "thread 2: slow [waiting sleep 20 ms]",
            "#0 slow at fibers.flx:6:5   (suspended)",
            "thread 1: main [waiting both (threads 2, 3)]",
            "#0 body at fibers.flx:31:16   (suspended)",
        ],
    );
    // A child fiber's stack ends at its function; the root fiber's goes on
    // down to `main`.
    let lines = flux_lines(&session);
    let fast = lines
        .iter()
        .position(|line| line == "#0 fast at fibers.flx:13:5");
    assert!(
        fast.is_some_and(|at| !lines[at + 1].starts_with("#1")),
        "{session}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("#") && line.contains(" main at fibers.flx:37:")),
        "{session}"
    );
}

#[test]
fn flux_catch_fibers_stops_where_a_fiber_fails() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("fibers");
    build_only(&scratch, "fibers.flx", "app");
    let Some(session) = fiber_session(
        &lldb,
        &scratch,
        "app",
        &[
            "flux catch fibers",
            "run",
            "flux where",
            "flux threads",
            "continue",
        ],
    ) else {
        return;
    };
    let lines = flux_lines(&session);
    let stop = lines
        .iter()
        .find(|line| line.starts_with("stopped: exception"))
        .unwrap_or_else(|| panic!("no exception stop:\n{session}"));
    assert!(
        stop.starts_with(
            r#"stopped: exception (fiber failure: ProtocolError(503, "too many")) at "#
        ) && stop.ends_with("(thread 4)"),
        "{session}"
    );
    assert_lines(
        &session,
        &["  1 main [waiting try (thread 4)] at fibers.flx:32:30"],
    );
    // `try` turns the failure into a value: the program goes on and ends.
    assert_exited(&session, 0, "56");
}

#[test]
fn a_panicking_root_fiber_stops_once() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("root_panic");
    build_only(&scratch, "root_panic.flx", "app");
    let Some(session) = fiber_session(
        &lldb,
        &scratch,
        "app",
        &[
            "flux catch panic",
            "run",
            "flux where",
            "continue",
            "flux where",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &["stopped: exception (panic: root gave up) at root_panic.flx:8:9 in body"],
    );
    assert_eq!(
        session.matches("stopped: exception").count(),
        1,
        "{session}"
    );
    assert_exited(&session, 1, "panic: root gave up");
}

#[test]
fn flux_catch_stops_at_runtime_errors_in_flux_frames() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("panic");
    build_only(&scratch, "panic.flx", "app");
    let import = format!("command script import {}", lldb_script());
    let session = lldb_session(
        &lldb,
        &scratch,
        "app",
        &[&import, "flux catch panic", "run", "flux where", "flux bt"],
    );
    if session.contains("script interpreter") {
        eprintln!("note: this lldb runs no Python scripts; skipping");
        return;
    }
    // Division by zero is not one of the kinds asked for: the program ends
    // with it, without stopping.
    assert_exited(&session, 1, "panic: Division by zero @11:5");
    assert!(!session.contains("stopped: exception"), "{session}");

    let session = lldb_session(
        &lldb,
        &scratch,
        "app",
        &[
            &import,
            "flux catch division",
            "run",
            "flux where",
            "flux bt",
            "flux catch",
        ],
    );
    assert_lines(
        &session,
        &[
            "stopped: exception (division by zero) at panic.flx:11:5 in ratio",
            "#0 ratio at panic.flx:11:5",
            "#1 main at panic.flx:17:21",
            "stopping at: division by zero",
        ],
    );
}

#[test]
fn a_caught_panic_stops_at_the_call_to_panic() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("boom");
    scratch.write(
        "boom.flx",
        "fn check(n: Int) -> Int {\n    if n > 2 {\n        panic(\"too big\")\n    } else {\n        n\n    }\n}\n\nfn main() with IO {\n    print(to_string(check(3)))\n}\n",
    );
    build_only(&scratch, "boom.flx", "boom");
    let import = format!("command script import {}", lldb_script());
    let session = lldb_session(
        &lldb,
        &scratch,
        "boom",
        &[&import, "flux catch panic", "run", "flux where", "flux bt"],
    );
    if session.contains("script interpreter") {
        eprintln!("note: this lldb runs no Python scripts; skipping");
        return;
    }
    assert_lines(
        &session,
        &[
            "stopped: exception (panic: too big) at boom.flx:3:9 in check",
            "#0 check at boom.flx:3:9",
        ],
    );
}

/// Runs the binary with `FLUX_TAPE_RECORD` set, as an editor does: the
/// program writes the tape on its own.
fn record_tape(scratch: &Scratch, binary: &str, tape: &str) {
    let output = Command::new(scratch.join(binary))
        .current_dir(scratch.path())
        .env("FLUX_TAPE_RECORD", scratch.join(tape))
        .output()
        .expect("run the program");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs `commands` in lldb with the script loaded and the program set to
/// run with `env`; None when lldb runs no Python.
fn tape_session(
    lldb: &[String],
    scratch: &Scratch,
    binary: &str,
    env: &str,
    commands: &[&str],
) -> Option<String> {
    let import = format!("command script import {}", lldb_script());
    let env = format!("settings set target.env-vars FLUX_WORKERS=1 {env}");
    let mut all = vec![import.as_str(), env.as_str()];
    all.extend_from_slice(commands);
    let session = lldb_session(lldb, scratch, binary, &all);
    if session.contains("script interpreter") || session.contains("scripting not supported") {
        eprintln!("note: this lldb runs no Python scripts; skipping");
        return None;
    }
    Some(session)
}

#[test]
fn lldb_runs_a_replay_to_an_event_and_back() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("tape");
    build_only(&scratch, "tape.flx", "app");
    record_tape(&scratch, "app", "run.jsonl");
    let replay = format!("FLUX_TAPE_REPLAY={}", scratch.join("run.jsonl").display());
    let Some(session) = tape_session(
        &lldb,
        &scratch,
        "app",
        &replay,
        &[
            "flux event 1",
            "flux locals",
            "flux events",
            "flux event 3",
            "flux event 0",
            "flux event 9",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "stopped: event 1 (clock_now()) at tape.flx:5:18 in main",
            "#0 clock_now() (done)",
            "#1 clock_now() (done)",
            "#3 stdout(\"true\\n\") (ahead)",
            "#4 a normal exit (ahead)",
            "stopped: event 3 (stdout(\"true\\n\")) at tape.flx:7:5 in main",
            // Back, by running the replay again.
            "stopped: event 0 (clock_now()) at tape.flx:4:17 in main",
            "no event #9 on the tape; `flux events` lists them",
        ],
    );
    let locals = flux_lines(&session);
    assert!(
        locals.iter().any(|line| line.starts_with("first = ")),
        "{session}"
    );
}

#[test]
fn lldb_goes_forward_only_in_a_recording() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("tape");
    build_only(&scratch, "tape.flx", "app");
    let record = format!("FLUX_TAPE_RECORD={}", scratch.join("run.jsonl").display());
    let Some(session) = tape_session(
        &lldb,
        &scratch,
        "app",
        &record,
        &["flux event 2", "flux events", "flux event 0"],
    ) else {
        return;
    };
    // Event 2 prints the clock's reading, whatever it was.
    let lines = flux_lines(&session);
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("stopped: event 2 (stdout(")
                && line.ends_with(") at tape.flx:6:5 in main")),
        "{session}"
    );
    assert_lines(
        &session,
        &["event #0 has happened; going back to it needs a replay of this tape"],
    );
}

#[test]
fn lldb_says_when_a_run_has_no_tape() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("tape");
    build_only(&scratch, "tape.flx", "app");
    let Some(session) = tape_session(
        &lldb,
        &scratch,
        "app",
        "",
        &[
            "breakpoint set --file tape.flx --line 6",
            "run",
            "flux events",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &["this run has no tape: launch it with FLUX_TAPE_REPLAY or FLUX_TAPE_RECORD set"],
    );
}

#[test]
fn an_inlined_lambda_is_described_as_inlined() {
    // 0188 S9c: Core inlines a lambda applied where it is written; its code
    // keeps the lambda's places, inlined at the call.
    let source = std::fs::read_to_string(fixture_dir().join("inlined.flx")).unwrap();
    let ir = module_ir(&source, flux::debuginfo::DebugLevel::Full);
    assert!(
        ir.contains("distinct !DISubprogram(name: \"<lambda app.flx:4:14>\""),
        "{ir}"
    );
    assert!(ir.contains(", inlinedAt: !"), "{ir}");
}

#[test]
fn lldb_shows_an_inlined_lambda_as_a_frame() {
    let Some(lldb) = lldb() else {
        eprintln!("note: lldb not found; skipping (set FLUX_LLDB)");
        return;
    };
    let scratch = prepare("inlined");
    build_only(&scratch, "inlined.flx", "app");
    let Some(session) = flux_session(
        &lldb,
        &scratch,
        "app",
        ("inlined.flx", 6),
        &[
            "flux bt",
            "flux locals",
            "flux next",
            "flux bt",
            "flux locals",
        ],
    ) else {
        return;
    };
    assert_lines(
        &session,
        &[
            "#0 <lambda inlined.flx:4:14> at inlined.flx:6:9   [inlined]",
            "#1 main at inlined.flx:4:14",
            "y = 40",
            "#0 main at inlined.flx:8:14",
            "n = 41",
        ],
    );
}
