#![cfg(all(feature = "jit", feature = "llvm"))]

use std::path::Path;
use std::process::Command;

use flux::bytecode::compiler::Compiler;
use flux::bytecode::vm::VM;
use flux::diagnostics::render_diagnostics;
use flux::jit::{JitOptions, jit_compile_and_run};
use flux::llvm::{LlvmOptions, llvm_compile_and_run};
use flux::runtime::value::Value;
use flux::syntax::{lexer::Lexer, parser::Parser};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn normalize(output: &str) -> String {
    output
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_flux(file: &str, extra_flag: Option<&str>) -> (i32, String, String) {
    let mut args = vec!["--no-cache", file];
    if let Some(flag) = extra_flag {
        args.push(flag);
    }

    let output = Command::new(flux_bin())
        .current_dir(workspace_root())
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux for `{file}` ({extra_flag:?}): {e}"));

    let status = output.status.code().unwrap_or(-1);
    let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
    let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
    (status, stdout, stderr)
}

fn assert_backend_parity(file: &str) {
    let (vm_status, vm_stdout, vm_stderr) = run_flux(file, None);
    let (jit_status, jit_stdout, jit_stderr) = run_flux(file, Some("--jit"));
    let (llvm_status, llvm_stdout, llvm_stderr) = run_flux(file, Some("--llvm"));

    assert_eq!(
        vm_status, jit_status,
        "{file}: VM/Cranelift exit-code mismatch\nVM stderr:\n{vm_stderr}\nCranelift stderr:\n{jit_stderr}"
    );
    assert_eq!(
        vm_status, llvm_status,
        "{file}: VM/LLVM exit-code mismatch\nVM stderr:\n{vm_stderr}\nLLVM stderr:\n{llvm_stderr}"
    );
    assert_eq!(
        vm_stdout, jit_stdout,
        "{file}: VM/Cranelift stdout mismatch\nVM:\n{vm_stdout}\nCranelift:\n{jit_stdout}"
    );
    assert_eq!(
        vm_stdout, llvm_stdout,
        "{file}: VM/LLVM stdout mismatch\nVM:\n{vm_stdout}\nLLVM:\n{llvm_stdout}"
    );
}

fn run_vm_program(input: &str) -> Result<Value, String> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    if !parser.errors.is_empty() {
        return Err(render_diagnostics(&parser.errors, Some(input), None));
    }

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    if let Err(diags) = compiler.compile(&program) {
        return Err(render_diagnostics(&diags, Some(input), None));
    }

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
    if !parser.errors.is_empty() {
        return Err(render_diagnostics(&parser.errors, Some(input), None));
    }

    let interner = parser.take_interner();
    jit_compile_and_run(&program, &interner, &JitOptions::default())
        .map(|(value, _)| value)
        .map_err(|err| err.to_string())
}

fn run_llvm_program(input: &str) -> Result<Value, String> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    if !parser.errors.is_empty() {
        return Err(render_diagnostics(&parser.errors, Some(input), None));
    }

    let interner = parser.take_interner();
    llvm_compile_and_run(
        &program,
        &interner,
        &LlvmOptions {
            source_file: Some("<test>".to_string()),
            source_text: Some(input.to_string()),
            opt_level: 0,
        },
    )
    .map(|(value, _)| value)
    .map_err(|err| err.to_string())
}

fn assert_backend_value_parity(input: &str) {
    let vm_value =
        run_vm_program(input).unwrap_or_else(|err| panic!("VM failed unexpectedly: {err}"));
    let jit_value =
        run_jit_program(input).unwrap_or_else(|err| panic!("Cranelift failed unexpectedly: {err}"));
    let llvm_value =
        run_llvm_program(input).unwrap_or_else(|err| panic!("LLVM failed unexpectedly: {err}"));

    assert_eq!(
        vm_value, jit_value,
        "VM/Cranelift value mismatch for program:\n{input}"
    );
    assert_eq!(
        vm_value, llvm_value,
        "VM/LLVM value mismatch for program:\n{input}"
    );
}

#[test]
fn aether_verify_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/verify_aether.flx");
}

#[test]
fn aether_bench_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/bench_reuse.flx");
}

#[test]
fn aether_bench_reuse_enabled_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/bench_reuse_enabled.flx");
}

#[test]
fn aether_bench_reuse_blocked_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/bench_reuse_blocked.flx");
}

#[test]
fn aether_hof_recursive_suite_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/hof_recursive_suite.flx");
}

#[test]
fn aether_tree_updates_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/tree_updates.flx");
}

#[test]
fn aether_queue_workload_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/queue_workload.flx");
}

#[test]
fn aether_forwarded_wrapper_reuse_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/forwarded_wrapper_reuse.flx");
}

#[test]
fn aether_opt_corpus_positive_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/opt_corpus_positive.flx");
}

#[test]
fn aether_opt_corpus_negative_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/opt_corpus_negative.flx");
}

#[test]
fn aether_fbip_success_cases_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/fbip_success_cases.flx");
}

#[test]
fn aether_borrow_calls_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/borrow_calls.flx");
}

#[test]
fn aether_reuse_alias_spines_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/reuse_alias_spines.flx");
}

#[test]
fn aether_reuse_specialization_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/reuse_specialization.flx");
}

#[test]
fn aether_drop_spec_branchy_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/drop_spec_branchy.flx");
}

#[test]
fn aether_drop_spec_recursive_fixture_matches_vm_cranelift_and_llvm() {
    assert_backend_parity("examples/aether/drop_spec_recursive.flx");
}

#[test]
fn aether_list_rebuild_reuse_matches_vm_cranelift_and_llvm() {
    let input = r#"
fn rebuild(xs) {
    match xs {
        [h | t] -> [h | t],
        _ -> [],
    }
}

fn main() {
    rebuild([1, 2, 3])
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_option_wrapper_reuse_matches_vm_cranelift_and_llvm() {
    let input = r#"
fn option_map(opt, f) {
    match opt {
        Some(x) -> Some(f(x)),
        _ -> None,
    }
}

fn main() {
    option_map(Some(42), \x -> x + 1)
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_borrowed_list_length_matches_vm_cranelift_and_llvm() {
    let input = r#"
fn my_len(xs) {
    match xs {
        [_ | t] -> 1 + my_len(t),
        _ -> 0,
    }
}

fn main() {
    let xs = [1, 2, 3, 4]
    my_len(xs) + my_len(xs)
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_borrowed_call_chain_matches_vm_cranelift_and_llvm() {
    let input = r#"
fn my_len(xs) {
    match xs {
        [_ | t] -> 1 + my_len(t),
        _ -> 0,
    }
}

fn len_twice(xs) {
    my_len(xs) + my_len(xs)
}

fn main() {
    len_twice([1, 2, 3])
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_branchy_filter_matches_vm_cranelift_and_llvm() {
    let input = r#"
fn my_filter(xs, f) {
    match xs {
        [h | t] -> if f(h) { [h | my_filter(t, f)] } else { my_filter(t, f) },
        _ -> [],
    }
}

fn main() {
    my_filter([1, 2, 3, 4, 5, 6], \x -> x % 2 == 0)
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_named_adt_update_matches_vm_cranelift_and_llvm() {
    let input = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)

fn set_black(t) {
    match t {
        Node(_, left, key, right) -> Node(Black, left, key, right),
        _ -> t,
    }
}

fn main() {
    set_black(Node(Red, Leaf, 5, Leaf))
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_drop_spec_named_adt_multiuse_field_matches_vm_cranelift_and_llvm() {
    let input = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)

fn dup_left(t) {
    match t {
        Node(color, left, key, right) -> Node(color, left, key, left),
        _ -> t,
    }
}

fn main() {
    dup_left(Node(Red, Leaf, 5, Leaf))
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_drop_spec_branchy_named_adt_matches_vm_cranelift_and_llvm() {
    let input = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)

fn keep_or_dup_left(t, keep) {
    match t {
        Node(color, left, key, right) -> if keep { Node(color, left, key, right) } else { Node(color, left, key, left) },
        _ -> t,
    }
}

fn main() {
    keep_or_dup_left(Node(Red, Leaf, 5, Leaf), false)
}
"#;

    assert_backend_value_parity(input);
}

#[test]
fn aether_drop_spec_list_multiuse_field_matches_vm_cranelift_and_llvm() {
    let input = r#"
fn copy_head(xs) {
    match xs {
        [h | t] -> [h | [h | t]],
        _ -> [],
    }
}

fn main() {
    copy_head([1, 2, 3])
}
"#;

    assert_backend_value_parity(input);
}
