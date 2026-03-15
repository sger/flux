#![cfg(feature = "jit")]

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::bytecode::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
use flux::jit::{JitOptions, jit_compile_and_run};
use flux::runtime::{value::Value, vm::VM};
use flux::syntax::{lexer::Lexer, parser::Parser};

fn run_vm(input: &str) -> Result<Value, String> {
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

fn run_jit(input: &str) -> Result<Value, String> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    if !parser.errors.is_empty() {
        return Err(render_diagnostics(&parser.errors, Some(input), None));
    }
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner.clone());
    if let Err(diags) = compiler.compile(&program) {
        return Err(render_diagnostics(&diags, Some(input), None));
    }
    jit_compile_and_run(&program, &interner, &JitOptions::default())
        .map(|(value, _)| value)
        .map_err(|err| err.to_string())
}

fn assert_vm_jit_value(input: &str) {
    let vm_value = run_vm(input).unwrap_or_else(|err| panic!("VM failed unexpectedly: {}", err));
    let jit_value = run_jit(input).unwrap_or_else(|err| panic!("JIT failed unexpectedly: {}", err));
    assert_eq!(
        vm_value, jit_value,
        "VM/JIT value mismatch for input:\n{}",
        input
    );
}

fn assert_vm_jit_error_contains(input: &str, needle: &str) {
    let vm_err = run_vm(input).expect_err("VM should fail");
    let jit_err = run_jit(input).expect_err("JIT should fail");
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

#[test]
fn vm_and_jit_match_numeric_primop_value() {
    assert_vm_jit_value("iadd(imul(6, 7), 0)");
}

#[test]
fn vm_and_jit_match_numeric_primop_error() {
    assert_vm_jit_error_contains("idiv(42, 0)", "division by zero");
}

#[test]
fn vm_and_jit_match_map_primop_value() {
    assert_vm_jit_value(
        r#"
let m = {}
map_get(map_set(m, "k", 7), "k")
"#,
    );
}

#[test]
fn vm_and_jit_match_string_primop_value() {
    assert_vm_jit_value(r#"string_slice(string_concat("Flux", "Lang"), 0, 4)"#);
}

#[test]
fn vm_and_jit_match_phase2_collection_primop_values() {
    assert_vm_jit_value("first(#[1, 2, 3])");
    assert_vm_jit_value("last(#[1, 2, 3])");
    assert_vm_jit_value("rest(#[1, 2, 3])");
    assert_vm_jit_value("contains(#[1, 2, 3], 2)");
    assert_vm_jit_value("slice(#[1, 2, 3], 0, 2)");
    assert_vm_jit_value("concat(#[1, 2], #[3, 4])");
}

#[test]
fn vm_and_jit_match_phase2_string_primop_values() {
    assert_vm_jit_value(r#"trim("  hi  ")"#);
    assert_vm_jit_value(r#"upper("hi")"#);
    assert_vm_jit_value(r#"lower("HI")"#);
    assert_vm_jit_value(r#"starts_with("hello", "he")"#);
    assert_vm_jit_value(r#"ends_with("hello", "lo")"#);
    assert_vm_jit_value(r#"replace("banana", "na", "X")"#);
    assert_vm_jit_value(r#"chars("ab")"#);
}

#[test]
fn vm_and_jit_match_phase2_map_primop_values() {
    assert_vm_jit_value(r#"len(keys(put({}, "a", 1)))"#);
    assert_vm_jit_value(r#"len(values(put({}, "a", 1)))"#);
    assert_vm_jit_value(r#"is_map(merge({}, put({}, "a", 1)))"#);
    assert_vm_jit_value(r#"get(delete(put({}, "a", 1), "a"), "a")"#);
}

#[test]
fn vm_and_jit_match_phase2_parse_primop_values() {
    assert_vm_jit_value(r#"parse_int(" 123 ")"#);
    assert_vm_jit_value(r#"parse_ints(#["1", "2", "3"])"#);
    assert_vm_jit_value(r#"split_ints("1,2,3", ",")"#);
}

#[test]
fn vm_and_jit_match_string_length_contract_for_non_ascii() {
    let vm_len = run_vm(r#"len("é")"#).expect("VM len should succeed");
    let jit_len = run_jit(r#"len("é")"#).expect("JIT len should succeed");
    assert_eq!(vm_len, Value::Integer(2));
    assert_eq!(jit_len, Value::Integer(2));

    let vm_string_len = run_vm(r#"string_len("é")"#).expect("VM string_len should succeed");
    let jit_string_len = run_jit(r#"string_len("é")"#).expect("JIT string_len should succeed");
    assert_eq!(vm_string_len, Value::Integer(2));
    assert_eq!(jit_string_len, Value::Integer(2));
}

#[test]
fn vm_and_jit_match_phase2_primop_errors() {
    assert_vm_jit_error_contains(r#"contains("oops", 1)"#, "first argument");
    assert_vm_jit_error_contains("concat(1, #[2])", "concat");
    assert_vm_jit_error_contains("concat(#[1], 2)", "concat");
    assert_vm_jit_error_contains(r#"parse_int("12x")"#, "could not parse");
    assert_vm_jit_error_contains(r#"split_ints("1,a,3", ",")"#, "could not parse");
    assert_vm_jit_error_contains(r#"delete({}, [])"#, "hashable");
}

#[test]
fn jit_primop_type_errors_render_e1004_diagnostics() {
    for input in ["array_len(1)", "string_len(1)"] {
        let jit_err = run_jit(input).expect_err("JIT should fail");
        assert!(
            jit_err.contains("error[E1004]: primop"),
            "expected rendered E1004 diagnostic for `{input}`; got:\n{jit_err}"
        );
        assert!(
            jit_err.contains("primop"),
            "expected primop-specific detail for `{input}`; got:\n{jit_err}"
        );
        assert!(
            !jit_err
                .trim()
                .eq("primop array_len expected Array, got Int")
                && !jit_err
                    .trim()
                    .eq("primop string_len expected String, got Int"),
            "expected formatted diagnostic instead of raw helper string for `{input}`; got:\n{jit_err}"
        );
    }
}

#[test]
fn jit_base_runtime_errors_render_diagnostics() {
    for (input, expected_code, expected_title) in [
        (
            "map(#[1], concat)",
            "error[E1000]: map: callback error at index 0: wrong number of arguments",
            "map: callback error at index 0: wrong number of arguments",
        ),
        (
            "flat_map(#[1], \\x -> x)",
            "error[E1009]: flat_map: callback must return an Array when input is Array, got Int",
            "flat_map: callback must return an Array when input is Array, got Int",
        ),
    ] {
        let jit_err = run_jit(input).expect_err("JIT should fail");
        assert!(
            jit_err.contains(expected_code),
            "expected rendered diagnostic for `{input}`; got:\n{jit_err}"
        );
        assert!(
            jit_err.contains(expected_title),
            "expected Base-specific title for `{input}`; got:\n{jit_err}"
        );
        assert!(
            jit_err.contains("<jit>:1:1"),
            "expected source location for `{input}`; got:\n{jit_err}"
        );
        assert!(
            !jit_err.trim().eq(expected_title),
            "expected formatted diagnostic instead of raw Base error for `{input}`; got:\n{jit_err}"
        );
    }
}

#[test]
fn jit_indirect_call_runtime_errors_render_diagnostics() {
    for (input, expected_header, expected_message) in [
        (
            "fn add(a, b) { a + b }\nfn main() {\n    let f = add\n    let ignored = f(1)\n}\n",
            "error[E1000]: wrong number of arguments: want=2, got=1",
            "wrong number of arguments: want=2, got=1",
        ),
        (
            "fn main() {\n    let f = 1\n    let ignored = f(2)\n}\n",
            "error[E1001]: Not A Function",
            "Cannot call non-function value (got Int).",
        ),
    ] {
        let jit_err = run_jit(input).expect_err("JIT should fail");
        assert!(
            jit_err.contains(expected_header),
            "expected rendered indirect-call diagnostic for `{input}`; got:\n{jit_err}"
        );
        assert!(
            jit_err.contains(expected_message),
            "expected indirect-call detail for `{input}`; got:\n{jit_err}"
        );
        assert!(
            !jit_err.trim().eq(expected_message),
            "expected formatted diagnostic instead of raw indirect-call error for `{input}`; got:\n{jit_err}"
        );
    }
}

#[test]
fn vm_and_jit_match_effectful_read_file_primop_value() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic")
        .as_nanos();
    let path: PathBuf = std::env::temp_dir().join(format!("flux_primop_parity_{}.txt", unique));
    fs::write(&path, "primop parity file").expect("should write temp file");

    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let program = format!(r#"read_file("{}")"#, escaped);
    assert_vm_jit_error_contains(&program, "Top-Level Effect");

    let _ = fs::remove_file(path);
}

#[test]
fn vm_and_jit_match_control_primop_error() {
    assert_vm_jit_error_contains(
        r#"panic("primop parity panic")"#,
        "panic: primop parity panic",
    );
}

#[test]
fn vm_and_jit_match_base_except_with_qualified_access() {
    assert_vm_jit_value(
        r#"
import Base except [print]
len([1, 2, 3]) + Base.len([1, 2, 3])
"#,
    );
    assert_vm_jit_value(
        r#"
import Base except [print]
to_string(7)
"#,
    );
    assert_vm_jit_value(
        r#"
import Base except [print]
Base.to_string(7)
"#,
    );
}

#[test]
fn vm_and_jit_match_base_qualified_call_under_shadowing() {
    assert_vm_jit_value(
        r#"
import Base except [print]
fn demo() {
  let len = fn(x) { 123 };
  [len([1, 2, 3]), Base.len([1, 2, 3])]
}
demo()
"#,
    );
}

#[test]
fn vm_and_jit_match_base_qualified_allowlisted_base_call() {
    assert_vm_jit_value(
        r#"
import Base except [print]
Base.map([1, 2, 3], fn(x) { x + 1 })
"#,
    );
}
