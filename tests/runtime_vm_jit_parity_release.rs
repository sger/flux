#![cfg(feature = "jit")]

use std::path::Path;
use std::process::Command;

use flux::bytecode::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
use flux::jit::{JitOptions, jit_compile_and_run};
use flux::runtime::{value::Value, vm::VM};
use flux::syntax::{lexer::Lexer, parser::Parser};

fn run_vm_program(input: &str) -> Result<Value, String> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    match vm.run() {
        Ok(()) => Ok(vm.last_popped_stack_elem().clone()),
        Err(err) => Err(err),
    }
}

fn run_jit_program(input: &str) -> Result<Value, String> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    let interner = parser.take_interner();
    jit_compile_and_run(&program, &interner, &JitOptions::default()).map(|(value, _)| value)
}

fn assert_vm_jit_value(input: &str) {
    let vm_value =
        run_vm_program(input).unwrap_or_else(|err| panic!("VM failed unexpectedly: {err}"));
    let jit_value =
        run_jit_program(input).unwrap_or_else(|err| panic!("JIT failed unexpectedly: {err}"));
    assert_eq!(
        vm_value, jit_value,
        "VM/JIT value mismatch for program:\n{}",
        input
    );
}

fn assert_vm_jit_error_signature_contains(input: &str, needle: &str) {
    let vm_err = run_vm_program(input).expect_err("VM should fail");
    let jit_err = run_jit_program(input).expect_err("JIT should fail");
    assert!(
        vm_err.contains(needle),
        "VM error missing {:?}:\n{}",
        needle,
        vm_err
    );
    assert!(
        jit_err.contains(needle),
        "JIT error missing {:?}:\n{}",
        needle,
        jit_err
    );
}

fn normalize_text(s: &str) -> String {
    s.replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_flux_file(
    workspace_root: &Path,
    flux_bin: &Path,
    file: &str,
    roots: &[&str],
    jit: bool,
) -> (i32, String, String) {
    let mut args = vec!["--no-cache".to_string()];
    for root in roots {
        args.push("--root".to_string());
        args.push((*root).to_string());
    }
    args.push(file.to_string());
    if jit {
        args.push("--jit".to_string());
    }

    let output = Command::new(flux_bin)
        .current_dir(workspace_root)
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux for `{file}` (jit={jit}): {e}"));

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (status, normalize_text(&stdout), normalize_text(&stderr))
}

fn error_signature(stderr: &str) -> String {
    let mut lines = Vec::new();
    for line in stderr.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("Stack trace:") {
            break;
        }
        lines.push(trimmed.to_string());
    }
    lines.join("\n")
}

fn assert_file_cli_outcome_parity(file: &str, roots: &[&str]) {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let flux_bin = Path::new(env!("CARGO_BIN_EXE_flux"));

    let (vm_status, vm_stdout, vm_stderr) =
        run_flux_file(workspace_root, flux_bin, file, roots, false);
    let (jit_status, jit_stdout, jit_stderr) =
        run_flux_file(workspace_root, flux_bin, file, roots, true);

    assert_eq!(
        vm_status, jit_status,
        "VM/JIT exit-code mismatch for `{file}`\nVM status={}\nJIT status={}\nVM stderr:\n{}\nJIT stderr:\n{}",
        vm_status, jit_status, vm_stderr, jit_stderr
    );
    if vm_status == 0 {
        assert_eq!(
            vm_stdout, jit_stdout,
            "VM/JIT stdout mismatch for `{file}`\nVM:\n{}\nJIT:\n{}",
            vm_stdout, jit_stdout
        );
    } else {
        assert_eq!(
            error_signature(&vm_stderr),
            error_signature(&jit_stderr),
            "VM/JIT error-signature mismatch for `{file}`\nVM:\n{}\nJIT:\n{}",
            vm_stderr,
            jit_stderr
        );
    }
}

#[test]
fn release_runtime_parity_module_qualified_typed_flow() {
    assert_vm_jit_value(
        r#"
module MathOps {
  public fn inc(x: Int) -> Int { x + 1 }
}
let result: Int = MathOps.inc(41);
result
"#,
    );
}

#[test]
fn release_runtime_parity_adt_flow() {
    assert_vm_jit_value(
        r#"
type Result<T, E> = Ok(T) | Err(E)
let r: Result<Int, String> = Ok(7)
match r {
  Ok(v) -> v + 1,
  Err(_e) -> 0
}
"#,
    );
}

#[test]
fn release_runtime_parity_tail_recursive_countdown() {
    assert_vm_jit_value(
        r#"
fn countdown(n) {
  if n == 0 {
    0
  } else {
    countdown(n - 1)
  }
}
countdown(100000)
"#,
    );
}

#[test]
fn release_runtime_parity_effectful_error_signature() {
    assert_vm_jit_error_signature_contains(
        r#"panic("release parity panic")"#,
        "panic: release parity panic",
    );
}

#[test]
fn release_runtime_parity_aoc_day04_output() {
    assert_file_cli_outcome_parity("examples/aoc/2024/day04.flx", &["examples/aoc/2024"]);
}

#[test]
fn release_runtime_parity_aoc_day05_output() {
    assert_file_cli_outcome_parity(
        "examples/aoc/2024/day05_part1_test.flx",
        &["lib", "examples/aoc/2024"],
    );
}
