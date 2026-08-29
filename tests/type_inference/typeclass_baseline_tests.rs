//! Baseline contract for the currently supported Flux typeclass surface.
//!
//! Each behavior is represented by a descriptive Flux fixture under
//! `examples/type_classes/`. This keeps the roadmap executable:
//! the Rust assertions validate compiler metadata and the fixtures validate
//! Flux runtime behavior and diagnostics.

use std::{path::Path, process::Command};

use flux::{
    bytecode::bytecode_cache::hash_bytes,
    compiler::{Compiler, module_interface},
    syntax::{lexer::Lexer, parser::Parser},
    types::module_interface::ModuleInterface,
};

#[path = "../support/primop_parity.rs"]
mod parity;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    workspace_root().join("examples/type_classes").join(name)
}

struct FixtureOutput {
    stdout: String,
}

fn normalize_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .trim()
        .to_string()
}

fn parse_source(source: &str, file: &str) -> (flux::syntax::program::Program, Compiler) {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{file} parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, Compiler::new_with_interner(file, interner))
}

fn run_fixture(name: &str) -> Result<FixtureOutput, String> {
    let scratch = parity::scratch::Scratch::new(&format!("typeclass-baseline-{name}"));
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            fixture_path(name).to_str().expect("fixture path is UTF-8"),
            "--no-cache",
        ])
        .args(scratch.cache_args())
        .output()
        .expect("run Flux fixture");
    let stdout = normalize_output(&output.stdout);
    let stderr = normalize_output(&output.stderr);
    if !output.status.success() {
        return Err(format!("{name} failed:\n{stderr}"));
    }
    Ok(FixtureOutput { stdout })
}

fn build_interface_from_fixture() -> ModuleInterface {
    let source = std::fs::read_to_string(fixture_path("interface_metadata_roundtrip.flx"))
        .expect("read interface fixture");
    let (program, mut compiler) = parse_source(&source, "TypeclassMetadata.flx");
    compiler
        .compile_with_opts(&program, false, false)
        .expect("interface fixture should compile");
    let aether = compiler
        .lower_aether_report_program(&program, false)
        .expect("interface fixture should lower");
    let source_hash = hash_bytes(source.as_bytes());
    let config_hash = module_interface::compute_semantic_config_hash(false, false);
    let module_sym = compiler.interner.intern("TypeclassMetadata");
    module_interface::build_interface(
        "TypeclassMetadata",
        module_sym,
        &source_hash,
        &config_hash,
        aether.as_core(),
        compiler.cached_member_schemes(),
        &compiler.exported_runtime_contracts(),
        &compiler.module_function_visibility,
        Some(compiler.class_env()),
        Vec::new(),
        &compiler.interner,
        Some(&program),
    )
}

#[test]
fn typeclass_fixtures_have_descriptive_contracts_and_parse() {
    let fixtures = [
        "dictionary_call_arity.flx",
        "generalized_constraint_obligation.flx",
        "result_directed_method_lookup.flx",
        "invalid_higher_kind.flx",
        "unsupported_deriving_diagnostic.flx",
        "interface_metadata_roundtrip.flx",
        "typeclass_backend_parity.flx",
        "multiple_class_obligations.flx",
        "superclass_instance_validation.flx",
    ];
    for fixture in fixtures {
        let source = std::fs::read_to_string(fixture_path(fixture)).expect("read fixture");
        assert!(
            source.to_ascii_lowercase().contains("baseline"),
            "{fixture} needs a baseline contract"
        );
        assert!(
            source.contains("Expected"),
            "{fixture} needs an expected result"
        );
        let _ = parse_source(&source, fixture);
    }
}

#[test]
fn class_and_instance_collection_is_present_in_compiler_environment() {
    let source = std::fs::read_to_string(fixture_path("dictionary_call_arity.flx"))
        .expect("read dictionary fixture");
    let (program, mut compiler) = parse_source(&source, "dictionary_call_arity.flx");
    compiler
        .compile(&program)
        .expect("dictionary fixture compiles");

    assert!(
        compiler
            .class_env()
            .classes
            .values()
            .any(|class| compiler.interner.resolve(class.name) == "Sizeable")
    );
    assert!(compiler.class_env().instances.iter().any(|instance| {
        compiler.interner.resolve(instance.class_name) == "Sizeable"
            && instance.type_args.len() == 1
    }));
}

#[test]
fn concrete_and_polymorphic_dictionary_calls_have_exact_runtime_arity() {
    for (fixture, expected) in [
        ("dictionary_call_arity.flx", "42"),
        ("generalized_constraint_obligation.flx", "true"),
        ("multiple_class_obligations.flx", "\"7\""),
        ("superclass_instance_validation.flx", "5\n500"),
    ] {
        let output = run_fixture(fixture).unwrap_or_else(|error| panic!("{}", error));
        assert_eq!(output.stdout, expected, "unexpected output for {fixture}");
    }
}

#[test]
fn result_directed_lookup_fixture_locks_current_baseline() {
    let output = run_fixture("result_directed_method_lookup.flx")
        .expect("current concrete multi-parameter dispatch should run");
    assert_eq!(output.stdout, "\"42\"");
}

#[test]
fn unsupported_features_preserve_the_current_baseline() {
    for fixture in [
        "invalid_higher_kind.flx",
        "unsupported_deriving_diagnostic.flx",
    ] {
        run_fixture(fixture)
            .unwrap_or_else(|error| panic!("{fixture} changed the baseline: {error}"));
    }
}

#[test]
fn unsupported_deriving_does_not_fabricate_a_dictionary() {
    let source = std::fs::read_to_string(fixture_path("unsupported_deriving_diagnostic.flx"))
        .expect("read deriving fixture");
    let (program, mut compiler) = parse_source(&source, "unsupported_deriving_diagnostic.flx");
    compiler
        .compile(&program)
        .expect("deriving baseline compiles");
    let core = compiler
        .dump_core_with_opts(
            &program,
            false,
            flux::core::display::CoreDisplayMode::Readable,
        )
        .expect("deriving baseline should lower");
    assert!(
        !core.contains("__dict_Functor_Box"),
        "unsupported deriving must not fabricate Functor evidence"
    );
}

#[test]
fn public_typeclass_metadata_survives_interface_serialization_roundtrip() {
    let interface = build_interface_from_fixture();
    assert_eq!(interface.public_classes.len(), 1);
    assert_eq!(interface.public_instances.len(), 1);
    assert_eq!(interface.public_classes[0].name, "Sizeable");
    assert_eq!(interface.public_instances[0].class_name, "Sizeable");
    assert_eq!(interface.public_instances[0].head_type_repr, "Int");

    let encoded = serde_json::to_vec(&interface).expect("serialize interface");
    let decoded: ModuleInterface = serde_json::from_slice(&encoded).expect("reload interface");
    assert_eq!(decoded, interface);
    assert_eq!(
        decoded.interface_fingerprint,
        interface.interface_fingerprint
    );
}

#[test]
fn cold_and_warm_compilation_reuse_the_same_typeclass_cache_contract() {
    let scratch = parity::scratch::Scratch::new("typeclass-baseline-cache");
    let fixture = fixture_path("dictionary_call_arity.flx");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .args([
                fixture.to_str().expect("fixture path is UTF-8"),
                "--verbose",
            ])
            .args(scratch.cache_args())
            .output()
            .expect("run cached fixture")
    };
    let cold = run();
    let warm = run();
    assert!(cold.status.success(), "cold compile failed");
    assert!(warm.status.success(), "warm compile failed");
    assert_eq!(
        String::from_utf8_lossy(&cold.stdout),
        String::from_utf8_lossy(&warm.stdout),
        "cache reuse changed runtime output"
    );
}

#[cfg(feature = "llvm")]
#[test]
fn supported_typeclass_fixture_has_vm_native_parity() {
    parity::assert_vm_native_parity("typeclass_dictionary_smoke.flx", "typeclass baseline smoke");
}

#[test]
fn multiple_obligations_are_present_in_lowered_dictionary_contract() {
    let source = std::fs::read_to_string(fixture_path("multiple_class_obligations.flx"))
        .expect("read obligations fixture");
    let (program, mut compiler) = parse_source(&source, "multiple_class_obligations.flx");
    compiler
        .compile(&program)
        .expect("obligations fixture compiles");
    let core = compiler
        .dump_core_with_opts(
            &program,
            false,
            flux::core::display::CoreDisplayMode::Readable,
        )
        .expect("obligations fixture should lower");
    assert!(core.contains("__dict_Equal_Int"));
    assert!(core.contains("__dict_Render_Int"));
}
