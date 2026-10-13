//! The backend-neutral debug-info model (proposal 0188, stage S0).
//!
//! `--dump-debug-info` snapshots cover what the model records: functions and
//! lambdas, variables with their types, data types, effects, compiler-made
//! code marked artificial, and one entry per module with its own file. The
//! library tests check that building it leaves the generated code alone.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use flux::{
    compiler::Compiler,
    debuginfo::DebugLevel,
    syntax::{lexer::Lexer, parser::Parser},
};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Runs `flux --dump-debug-info` on an example and returns its stdout with
/// the workspace path replaced, so snapshots hold on every checkout.
fn dump_debug_info(example: &str, extra: &[&str]) -> String {
    let scratch = Scratch::new("debuginfo-dump");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(scratch.cache_args())
        .args(extra)
        .arg(workspace_root().join("examples").join(example))
        .arg("--dump-debug-info")
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    assert!(
        output.status.success(),
        "flux --dump-debug-info failed for {example}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = workspace_root().to_string_lossy().into_owned();
    String::from_utf8_lossy(&output.stdout).replace(&root, "<root>")
}

fn assert_dump_snapshot(name: &str, example: &str, extra: &[&str]) {
    let dump = dump_debug_info(example, extra);
    insta::with_settings!({
        snapshot_path => "../snapshots/debuginfo",
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!(name, dump);
    });
}

#[test]
fn dump_plain_functions() {
    assert_dump_snapshot("plain_functions", "guide/factorial.flx", &[]);
}

#[test]
fn dump_lambdas_and_closures() {
    assert_dump_snapshot("lambdas_and_closures", "functions/closure.flx", &[]);
}

#[test]
fn dump_named_field_data_and_record_patterns() {
    assert_dump_snapshot("record_patterns", "guide/record_patterns.flx", &[]);
}

#[test]
fn dump_effect_and_handler() {
    assert_dump_snapshot(
        "effect_handler",
        "effects/07_custom_effect_handler.flx",
        &[],
    );
}

#[test]
fn dump_marks_dictionaries_artificial() {
    assert_dump_snapshot(
        "contextual_dictionary",
        "type_classes/contextual_dictionary.flx",
        &[],
    );
}

#[test]
fn dump_pattern_binders() {
    assert_dump_snapshot("list_patterns", "patterns/list_patterns.flx", &[]);
}

#[test]
fn dump_gives_each_module_its_own_file() {
    assert_dump_snapshot(
        "two_modules",
        "imports/test_import_math.flx",
        &["--root", "examples"],
    );
}

#[test]
fn native_dump_matches_vm_dump() {
    // Both backends build the model from the same Core, before their own
    // lowering, so they must agree.
    let vm = dump_debug_info("functions/closure.flx", &[]);
    let native = dump_debug_info("functions/closure.flx", &["--native"]);
    assert_eq!(vm, native);
}

const PROGRAM: &str = r#"
data Shape {
    Circle { radius: Float },
    Rect { w: Float, h: Float },
}

fn area(s: Shape) -> Float {
    match s {
        Circle { radius } -> 3.14 * radius * radius,
        Rect { w, h } -> w * h,
    }
}

fn main() {
    let shape = Rect { w: 2.0, h: 3.0 }
    let measure = fn(s) { area(s) }
    measure(shape)
}
"#;

/// Compiles `PROGRAM` at `level` and returns the compiler.
fn compile_at(level: DebugLevel) -> Compiler {
    let mut parser = Parser::new(Lexer::new(PROGRAM));
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("shapes.flx", interner);
    compiler.set_debug_level(level);
    compiler
        .compile_with_opts(&program, false, false)
        .expect("compile");
    compiler
}

#[test]
fn no_debug_info_without_a_debug_level() {
    assert!(compile_at(DebugLevel::None).debug_infos().is_empty());
}

#[test]
fn debug_level_records_one_module_with_its_functions() {
    let compiler = compile_at(DebugLevel::Full);
    let infos = compiler.debug_infos();
    assert_eq!(infos.len(), 1);
    let info = &infos[0];
    assert_eq!(info.files[0].path, "shapes.flx");
    let names: Vec<_> = info
        .functions
        .iter()
        .filter(|function| !function.artificial)
        .map(|function| function.source_name.as_str())
        .collect();
    assert_eq!(names[..2], ["area", "main"]);
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(
        names[2].starts_with("<lambda shapes.flx:16:"),
        "the lambda is named by where it is written: {names:?}"
    );
}

#[test]
fn debug_level_leaves_the_bytecode_unchanged() {
    let plain = compile_at(DebugLevel::None).bytecode();
    let debug = compile_at(DebugLevel::Full).bytecode();
    assert_eq!(
        format!("{:?}", plain.instructions),
        format!("{:?}", debug.instructions)
    );
    assert_eq!(
        format!("{:?}", plain.constants),
        format!("{:?}", debug.constants)
    );
}
