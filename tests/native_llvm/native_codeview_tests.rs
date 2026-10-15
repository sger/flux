//! Native debug builds on Windows (proposal 0188 S11): CodeView in a PDB
//! that Visual Studio, WinDbg and cdb read, for call stacks with Flux names
//! and `.flx` lines.
//!
//! Each fixture in `tests/native_llvm/debug/` is built with
//! `--native -g -o <binary>.exe`. Then:
//! - `llvm-pdbutil` must list the Flux functions and the `.flx` lines;
//! - cdb, stopped where a Flux program panics, must show Flux frames.
//!
//! A machine without llvm-pdbutil or cdb skips those checks with a note.
//! cdb comes with the Windows SDK's Debugging Tools for Windows.

#![cfg(all(feature = "llvm", windows))]

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native_llvm/debug")
}

/// Copies the fixtures into scratch.
fn prepare(name: &str) -> Scratch {
    let scratch = Scratch::new(&format!("native-codeview-{name}"));
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

/// Runs flux in `scratch` with `args` and the scratch's cache, and checks
/// that it succeeded unless `may_fail`.
fn flux(scratch: &Scratch, args: &[&str], may_fail: bool) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(scratch.path())
        .args(args)
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    assert!(
        may_fail || output.status.success(),
        "flux {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Builds `program` with `-g` into `binary` without running it.
fn build(scratch: &Scratch, program: &str, binary: &str) {
    flux(
        scratch,
        &[program, "--native", "-g", "--emit-binary", "-o", binary],
        false,
    );
}

/// `name` from `env`, else from PATH, else the first of `fallbacks` that
/// exists.
fn find_tool(env: &str, name: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(env) {
        return Some(PathBuf::from(path));
    }
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    });
    on_path.or_else(|| {
        fallbacks
            .iter()
            .map(PathBuf::from)
            .find(|candidate| candidate.is_file())
    })
}

fn pdbutil() -> Option<PathBuf> {
    find_tool(
        "FLUX_PDBUTIL",
        "llvm-pdbutil.exe",
        &[r"C:\Program Files\LLVM\bin\llvm-pdbutil.exe"],
    )
}

fn cdb() -> Option<PathBuf> {
    find_tool(
        "FLUX_CDB",
        "cdb.exe",
        &[r"C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe"],
    )
}

fn run_tool(tool: &Path, args: &[&str], cwd: &Path) -> String {
    let output = Command::new(tool)
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|err| panic!("cannot run {}: {err}", tool.display()));
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The lines `llvm-pdbutil dump -l` gives for the source file `file_name`,
/// across every block of code compiled from it.
fn lines_of(dump: &str, file_name: &str) -> Vec<u32> {
    let mut lines = Vec::new();
    let mut in_file = false;
    for row in dump.lines() {
        if !row.starts_with(' ') && !row.trim().is_empty() {
            in_file = row.contains(file_name);
            continue;
        }
        if !in_file || row.contains("line/column/addr entries") {
            continue;
        }
        // `     2 00000520 !    3 0000054A ! `: a line, then its address, and
        // a mark only some entries have.
        let words: Vec<&str> = row.split_whitespace().collect();
        lines.extend(words.windows(2).filter_map(|pair| {
            let is_address = pair[1].len() == 8 && pair[1].chars().all(|c| c.is_ascii_hexdigit());
            is_address.then(|| pair[0].parse::<u32>().ok()).flatten()
        }));
    }
    lines
}

#[test]
fn the_ir_of_a_debug_build_is_codeview() {
    use flux::compiler::Compiler;
    use flux::syntax::{lexer::Lexer, parser::Parser};
    let source = std::fs::read_to_string(fixture_dir().join("calls.flx")).unwrap();
    let mut parser = Parser::new(Lexer::new(&source));
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("app.flx", interner);
    compiler.set_debug_level(flux::debuginfo::DebugLevel::Full);
    let module = compiler
        .lower_to_lir_llvm_module_per_module(&program, false, false, true)
        .expect("the module compiles");
    let ir = flux::llvm::render_module(&module);
    assert!(ir.contains("!\"CodeView\", i32 1"), "{ir}");
    assert!(!ir.contains("Dwarf Version"), "{ir}");
    assert!(ir.contains("DISubprogram(name: \"sum_squares\""), "{ir}");
}

#[test]
fn the_pdb_lists_flux_functions_and_their_lines() {
    let Some(pdbutil) = pdbutil() else {
        eprintln!("note: llvm-pdbutil not found; skipping (set FLUX_PDBUTIL)");
        return;
    };
    let scratch = prepare("calls");
    build(&scratch, "calls.flx", "calls.exe");
    let pdb = scratch.path().join("calls.pdb");
    assert!(pdb.exists(), "no PDB next to calls.exe");
    let pdb = pdb.display().to_string();

    let symbols = run_tool(&pdbutil, &["dump", "-symbols", &pdb], scratch.path());
    for function in ["square", "sum_squares", "main"] {
        assert!(
            symbols
                .lines()
                .any(|row| row.contains("S_GPROC32") && row.ends_with(&format!("`{function}`"))),
            "no S_GPROC32 for {function}"
        );
    }

    let dump = run_tool(&pdbutil, &["dump", "-l", &pdb], scratch.path());
    assert!(
        !dump.contains(r"\\?\"),
        "a verbatim path in the PDB:\n{dump}"
    );
    let lines = lines_of(&dump, "calls.flx");
    for line in [3, 4, 8, 9, 10, 14, 15] {
        assert!(lines.contains(&line), "no calls.flx:{line} in {lines:?}");
    }
}

#[test]
fn each_module_has_its_own_file_in_the_pdb() {
    let Some(pdbutil) = pdbutil() else {
        eprintln!("note: llvm-pdbutil not found; skipping (set FLUX_PDBUTIL)");
        return;
    };
    let scratch = prepare("twomods");
    build(&scratch, "twomods/main.flx", "twomods.exe");
    let pdb = scratch.path().join("twomods.pdb").display().to_string();
    let dump = run_tool(&pdbutil, &["dump", "-l", &pdb], scratch.path());
    assert!(!lines_of(&dump, "main.flx").is_empty(), "{dump}");
    assert!(!lines_of(&dump, "Shapes.flx").is_empty(), "{dump}");
}

#[test]
fn a_cached_debug_run_keeps_its_pdb_and_an_uncached_one_removes_it() {
    let scratch = prepare("cache");
    let cache = scratch.cache_dir();
    assert_eq!(
        flux(&scratch, &["calls.flx", "--native", "-g"], false).trim(),
        "25"
    );
    assert!(cache.join("native/calls.pdb").exists(), "no cached PDB");

    flux(
        &scratch,
        &["calls.flx", "--native", "-g", "--no-cache"],
        false,
    );
    let scratch_builds = cache.join("native/scratch");
    let mut left = Vec::new();
    let mut pending = vec![scratch_builds];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext == "exe" || ext == "pdb")
            {
                left.push(path);
            }
        }
    }
    assert!(left.is_empty(), "an uncached run left {left:?}");
}

#[test]
fn cdb_shows_flux_frames_where_a_program_panics() {
    let Some(cdb) = cdb() else {
        eprintln!(
            "note: cdb not found; skipping (set FLUX_CDB, or install the Debugging Tools for Windows)"
        );
        return;
    };
    let scratch = prepare("panic");
    build(&scratch, "panic.flx", "panic.exe");
    // `flux_panic` ends the process, so stop where the runtime reports it.
    // cdb looks a relative program up on its own search path, not the
    // working directory, so name it in full.
    let session = run_tool(
        &cdb,
        &[
            "-lines",
            "-y",
            &scratch.path().display().to_string(),
            "-c",
            "bp panic!flux_debug_on_panic; g; k; q",
            &scratch.path().join("panic.exe").display().to_string(),
        ],
        scratch.path(),
    );
    // The first runtime error is the division by zero in `ratio`, called
    // from `main` at line 17.
    assert!(
        session.contains("panic!ratio") && session.contains("panic!main"),
        "{session}"
    );
    assert!(
        session.contains("panic.flx @ 11") && session.contains("panic.flx @ 17"),
        "{session}"
    );
}
