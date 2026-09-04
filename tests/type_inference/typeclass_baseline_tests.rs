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
    build_interface_for("TypeclassMetadata")
}

fn build_interface_for(module: &str) -> ModuleInterface {
    let file = format!("{module}.flx");
    let source = std::fs::read_to_string(fixture_path(&file)).expect("read interface fixture");
    let (program, mut compiler) = parse_source(&source, &file);
    compiler
        .compile_with_opts(&program, false, false)
        .expect("interface fixture should compile");
    let aether = compiler
        .lower_aether_report_program(&program, false)
        .expect("interface fixture should lower");
    let source_hash = hash_bytes(source.as_bytes());
    let config_hash = module_interface::compute_semantic_config_hash(false, false);
    let module_sym = compiler.interner.intern(module);
    module_interface::build_interface(
        module,
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
        "unsupported_deriving_diagnostic.flx",
        "derived_eq.flx",
        "derived_parameterized_eq.flx",
        "derived_show.flx",
        "derived_encode.flx",
        "derived_decode.flx",
        "structural_container_dictionary.flx",
        "TypeclassMetadata.flx",
        "typeclass_backend_parity.flx",
        "multiple_class_obligations.flx",
        "superclass_instance_validation.flx",
        "superclass_order_independent.flx",
        "associated_type_declaration.flx",
        "associated_type_reduction.flx",
        "stuck_associated_type.flx",
        "associated_type_interface_roundtrip.flx",
        "two_dictionaries_one_class.flx",
        "two_dictionaries_superclass.flx",
        "SuperclassMetadata.flx",
        "superclass_across_modules.flx",
        "superclass_method_call.flx",
        "superclass_evidence_without_context.flx",
        "sibling_method_call_in_instance.flx",
        "contextual_superclass_evidence.flx",
        "transitive_superclass.flx",
        "kind_valid.flx",
        "hkt_instance_positive.flx",
        "structured_predicate.flx",
        "interface_roundtrip.flx",
        "contextual_dictionary.flx",
        "no_partial_resolution.flx",
        "where_constraint.flx",
        "solved_constraint.flx",
        "generalized_constraint.flx",
        "stuck_constraint.flx",
        "diagnosed_constraint.flx",
        "generalized_structured_constraint.flx",
        "multi_parameter_resolution.flx",
        "result_directed_resolution.flx",
        "where_constraint_multi_param.flx",
        "contextual_dictionary_wrapper.flx",
        "qualified_class_id.flx",
        "eq_ord.flx",
        "semigroup_monoid.flx",
        "mempty_result_dispatch.flx",
        "functor_applicative_monad.flx",
        "return_directed_pure.flx",
        "effectful_fmap.flx",
        "option_instances.flx",
        "list_instances.flx",
        "array_instances.flx",
        "either_instances.flx",
        "module_member_shadows_stub.flx",
        "let_annotation_rigid_param.flx",
        "result_directed_two_dictionaries.flx",
        "syntax_tour.flx",
    ];
    for fixture in fixtures {
        let source = std::fs::read_to_string(fixture_path(fixture)).expect("read fixture");
        assert!(
            {
                let source = source.to_ascii_lowercase();
                [
                    "baseline", "stage 1", "stage 2", "stage 3", "stage 4", "stage 5", "stage 6",
                    "stage 7", "stage 8",
                ]
                .iter()
                .any(|contract| source.contains(contract))
            },
            "{fixture} needs a baseline or a stage 1-8 contract"
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
        ("dictionary_call_arity.flx", "42\n10"),
        ("generalized_constraint_obligation.flx", "true"),
        ("multiple_class_obligations.flx", "\"7\""),
        ("superclass_instance_validation.flx", "5\n500"),
        (
            "eq_ord.flx",
            "\"same\"\n\"less\"\n\"more\"\n\"same\"\n\"more\"",
        ),
        (
            "semigroup_monoid.flx",
            "\"abc\"\n[1, 2, 3]\n[|1, 2|]\nSome(\"ab\")",
        ),
        ("mempty_result_dispatch.flx", "\"\"\n[]\n[|0|]\nNone\n0"),
        ("let_annotation_rigid_param.flx", "5\n7\n\"s\""),
        (
            "result_directed_two_dictionaries.flx",
            "7\n\"int\"\n\"str\"",
        ),
    ] {
        let output = run_fixture(fixture).unwrap_or_else(|error| panic!("{}", error));
        assert_eq!(output.stdout, expected, "unexpected output for {fixture}");
    }
}

#[test]
fn kind_checked_typeclass_fixtures_have_expected_output() {
    for (fixture, expected) in [
        ("kind_valid.flx", "42"),
        ("hkt_instance_positive.flx", "42"),
        ("structured_predicate.flx", "7"),
        ("interface_roundtrip.flx", "1"),
    ] {
        let output = run_fixture(fixture).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(output.stdout, expected, "unexpected output for {fixture}");
    }
}

/// Proposal 0179 Stage 2: a contextual instance lowers to a dictionary
/// *constructor* rather than a method tuple. Reaching it through a
/// constrained function requires that constructor to be initialised at module
/// load time; before Stage 2 the global was declared but never stored, and the
/// call failed with `E1001 Cannot call non-function value (got None)`.
/// Proposal 0179 Stage 2: a marker class (no methods) carries no dictionary,
/// so it must not add a parameter to a constrained function. Three phases
/// previously counted dictionaries with three different filters, giving the
/// callee a phantom parameter that call sites never passed — `E1000 wrong
/// number of arguments` on the VM, and an unchecked ABI mismatch natively.
#[test]
fn marker_class_constraints_add_no_dictionary_parameter() {
    let output = run_fixture("no_partial_resolution.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "7\n1");
}

#[test]
fn contextual_dictionary_is_initialised_for_constrained_calls() {
    let output = run_fixture("contextual_dictionary.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "false\nfalse\ntrue");
}

#[test]
fn structured_predicates_survive_scheme_construction() {
    let source = std::fs::read_to_string(fixture_path("structured_predicate.flx"))
        .expect("read structured predicate fixture");
    let (program, mut compiler) = parse_source(&source, "structured_predicate.flx");
    compiler
        .compile(&program)
        .expect("structured predicate fixture compiles");

    let core = compiler
        .dump_core_with_opts(
            &program,
            false,
            flux::core::display::CoreDisplayMode::Readable,
        )
        .expect("structured predicate should lower");
    assert!(
        core.contains("List") || core.contains("list_size"),
        "structured predicate must remain visible in the lowered contract"
    );
}

#[test]
fn result_directed_lookup_fixture_locks_current_baseline() {
    let output = run_fixture("result_directed_method_lookup.flx")
        .expect("current concrete multi-parameter dispatch should run");
    assert_eq!(output.stdout, "\"42\"");
}

/// Stage 4: every parameter of a multi-parameter class is read from the
/// position the class declaration puts it in, so two instances sharing no
/// first argument are still told apart.
#[test]
fn multi_parameter_classes_resolve_on_the_complete_predicate() {
    let output =
        run_fixture("multi_parameter_resolution.flx").expect("multi-parameter dispatch should run");
    assert_eq!(output.stdout, "\"42\"\ntrue");
}

/// Stage 4 (KI-052): a generic function forwarding to a contextual instance
/// gets the dictionary that instance's context needs, rather than one that
/// shadowed it.
#[test]
fn a_generic_wrapper_forwards_the_right_contextual_dictionary() {
    let output = run_fixture("contextual_dictionary_wrapper.flx")
        .expect("contextual dictionary forwarding should run");
    assert_eq!(output.stdout, "\"5\"\n\"5\"");
}

/// Stage 4: a `where` bound written with explicit arguments keeps all of them.
/// `where Convert<a, b>` used to emit the arity-1 predicate `Convert<a>`,
/// which no two-parameter instance head could match.
#[test]
fn a_where_bound_keeps_every_declared_type_argument() {
    let output = run_fixture("where_constraint_multi_param.flx")
        .expect("multi-parameter where bound should run");
    assert_eq!(output.stdout, "\"42\"\n3");
}

/// Stage 4: a class parameter occurring only in the return type is fixed by
/// the expected result, through both a `let` annotation and a function return
/// type. Before Stage 4 the predicate was built from the argument instead.
#[test]
fn a_class_parameter_in_the_return_type_is_fixed_by_the_expected_result() {
    let output =
        run_fixture("result_directed_resolution.flx").expect("result-directed dispatch should run");
    assert_eq!(output.stdout, "7\ntrue\n7");
}

/// Proposal 0179 Stage 7: a derived instance is reachable both by name and
/// through a dictionary. Pinning both routes is the point — methods that exist
/// but no dictionary to project them out of is the shape this stage removes.
#[test]
fn derived_instances_are_callable_and_carry_evidence() {
    for (fixture, expected) in [
        ("derived_eq.flx", "true\nfalse\ntrue\nfalse\ntrue"),
        (
            "derived_parameterized_eq.flx",
            "true\nfalse\ntrue\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\nfalse",
        ),
        ("derived_show.flx", "\"Red\"\n\"Blue\"\n\"Green\""),
        ("derived_decode.flx", "\"pair\"\n2"),
        (
            "structural_container_dictionary.flx",
            "true\nfalse\ntrue\nfalse\ntrue\ntrue",
        ),
    ] {
        let output =
            run_fixture(fixture).unwrap_or_else(|error| panic!("{fixture} should run: {error}"));
        assert_eq!(output.stdout, expected, "{fixture}");
    }
}

/// Proposal 0179 Stage 7: deriving a class with no derivation rule is an
/// error at the clause. Before Stage 7 this fixture compiled and printed `1`,
/// leaving a `Functor<Box>` instance whose only method was never generated.
#[test]
fn unsupported_deriving_is_rejected_at_the_clause() {
    let fixture = "unsupported_deriving_diagnostic.flx";
    let Err(error) = run_fixture(fixture) else {
        panic!("{fixture} must not compile: deriving `Functor` has no derivation rule");
    };
    assert!(
        error.contains("E486"),
        "{fixture} should report E486, got:\n{error}"
    );
}

/// The rejection is what keeps the evidence honest: no `Functor` dictionary is
/// fabricated, because the clause never gets as far as producing one.
#[test]
fn unsupported_deriving_does_not_fabricate_a_dictionary() {
    let source = std::fs::read_to_string(fixture_path("unsupported_deriving_diagnostic.flx"))
        .expect("read deriving fixture");
    let (program, mut compiler) = parse_source(&source, "unsupported_deriving_diagnostic.flx");
    let errors = compiler
        .compile(&program)
        .expect_err("underivable deriving must not compile");
    assert!(
        errors.iter().any(|diag| diag.code() == Some("E486")),
        "unsupported deriving must be reported, got: {errors:?}"
    );
}

/// A class method declares its effects on the class, where no contract check
/// could see them, so a caller with no `with` clause could call one and
/// perform IO. The identical call to an ordinary effectful function was
/// rejected; both must be now.
#[test]
fn a_class_method_effect_row_reaches_its_caller() {
    let source = r#"
class Flash<a> {
    fn flash(x: a) -> Int with IO
}

instance Flash<Int> {
    fn flash(x) with IO { x }
}

fn leaky(x: Int) -> Int { flash(x) }

fn main() { leaky(1) }
"#;
    let (program, mut compiler) = parse_source(source, "effect_leak_through_class_method.flx");
    let errors = compiler
        .compile(&program)
        .expect_err("a function calling an effectful class method must declare the effect");
    assert!(
        errors.iter().any(|diag| diag.code() == Some("E400")),
        "the class method's effect row must reach its caller, got: {errors:?}"
    );
}

/// An instance method may call a sibling method of its own class on its own
/// head type. The evidence is the dictionary being defined, so the predicate
/// must reduce to the instance's context rather than becoming a second
/// dictionary parameter (KI-078).
#[test]
fn an_instance_method_may_call_a_sibling_on_its_own_head() {
    let fixture = "sibling_method_call_in_instance.flx";
    let output = run_fixture(fixture).expect("fixture should compile and run");
    assert_eq!(output.stdout, "true\nfalse");
}

/// Superclass evidence discharged by a *contextual* instance: `Mid<List<a>>`
/// owes `Base<List<a>>`, and its context supplies `Base<a>`. Matching the
/// context on class identity alone put the wrong dictionary in the slot, and
/// the duplicated parameters it came with failed on arity (KI-077).
#[test]
fn superclass_evidence_comes_from_a_contextual_instance() {
    let fixture = "contextual_superclass_evidence.flx";
    let output = run_fixture(fixture).expect("fixture should compile and run");
    assert_eq!(output.stdout, "9");
}

/// Superclass evidence has two spellings, and both must dispatch the same way.
/// `superclass_method_call.flx` writes the context out, so its dictionary is a
/// constructor; this instance omits it, so the dictionary is a plain tuple whose
/// leading slot names the `Sizeable<Int>` dictionary directly. That tuple was
/// not recognised as an already-elaborated dictionary argument, so the call was
/// re-elaborated on every recompilation until the compiler ran out of stack.
#[test]
fn superclass_evidence_is_supplied_without_an_explicit_context() {
    let fixture = "superclass_evidence_without_context.flx";
    let output = run_fixture(fixture).expect("fixture should compile and run");
    assert_eq!(output.stdout, "505");
}

/// An instance context that grows its type argument at every step —
/// `Foo<a>` needing `Foo<List<a>>` — has no finite evidence tree. The search
/// never repeats a predicate, so the cycle check cannot see it; only the depth
/// budget stops it. Before that budget existed this overflowed the compiler's
/// stack and aborted the process.
///
/// Proposal 0183 R3 gave exhaustion its own code: the search was *abandoned*,
/// which is not the same fact as no instance existing, and reporting E444 sent
/// the reader looking for an instance that may well be there.
#[test]
fn a_growing_instance_context_terminates() {
    let source = r#"
class Foo<a> {
    fn foo(x: a) -> Int
}

instance Foo<List<a>> => Foo<a> {
    fn foo(x) { 1 }
}

fn use_it<a: Foo>(x: a) -> Int { foo(x) }

fn main() { use_it(1) }
"#;
    let (program, mut compiler) = parse_source(source, "growing_instance_context.flx");
    let errors = compiler
        .compile(&program)
        .expect_err("a context with no finite evidence tree must not resolve");
    assert!(
        errors.iter().any(|diag| diag.code() == Some("E488")),
        "a non-terminating context should report an exhausted search, got: {errors:?}"
    );
}

/// Two instances whose contexts name each other close a cycle in the
/// instance-context graph. The solver treats a repeat on the current path as
/// satisfied, so this compiles; what matters here is that resolution
/// terminates at all, rather than recursing until the stack is gone.
#[test]
fn mutually_recursive_instance_contexts_terminate() {
    let source = r#"
class Ay<a> { fn ay(x: a) -> Int }
class Bee<a> { fn bee(x: a) -> Int }

instance Bee<a> => Ay<a> {
    fn ay(x) { 1 }
}
instance Ay<a> => Bee<a> {
    fn bee(x) { 2 }
}

fn main() { ay(1) }
"#;
    let (program, mut compiler) = parse_source(source, "mutual_instance_contexts.flx");
    // Either outcome is acceptable; a stack overflow is not, and is what this
    // guards against.
    let _ = compiler.compile(&program);
}

/// An instance that omits a required method with no default is rejected at the
/// instance head, and `Compiler::compile` is where that must be visible.
/// E442 lived in the warning half of the `collect_class_declarations`
/// partition, so this returned `Ok` and left the missing method to
/// `generate_polymorphic_stub`; a later pipeline stage still rejected the
/// program, which is why the escape went unnoticed.
#[test]
fn missing_instance_method_is_rejected() {
    let source = r#"
class Describable<a> {
    fn name(x: a) -> Int
    fn value(x: a) -> Int
}

instance Describable<Int> {
    fn name(x) { x }
}

fn main() { 42 }
"#;
    let (program, mut compiler) = parse_source(source, "missing_instance_method.flx");
    let errors = compiler
        .compile(&program)
        .expect_err("an instance missing a required method must not compile");
    assert!(
        errors.iter().any(|diag| diag.code() == Some("E442")),
        "missing instance method must be reported, got: {errors:?}"
    );
}

/// The counterpart: a method the class defaults may be omitted, and doing so
/// must stay a clean compile. E442 fires on the absence of *both* an
/// implementation and a default.
#[test]
fn omitting_a_defaulted_method_is_accepted() {
    let source = r#"
class Describable<a> {
    fn name(x: a) -> Int
    fn value(x: a) -> Int { 0 }
}

instance Describable<Int> {
    fn name(x) { x }
}

fn main() { 42 }
"#;
    let (program, mut compiler) = parse_source(source, "defaulted_instance_method.flx");
    compiler
        .compile(&program)
        .expect("omitting a method that has a default must compile");
}

#[test]
fn public_typeclass_metadata_survives_interface_serialization_roundtrip() {
    let interface = build_interface_from_fixture();
    assert_eq!(interface.public_classes.len(), 1);
    assert_eq!(interface.public_instances.len(), 1);
    assert_eq!(interface.public_classes[0].name, "Sizeable");
    assert_eq!(
        interface.public_classes[0].parameter_kinds,
        [flux::types::kind::Kind::Type]
    );
    assert_eq!(interface.public_instances[0].class_name, "Sizeable");
    assert_eq!(interface.public_instances[0].head_type_repr, "Int");
    assert_eq!(
        interface.public_instances[0].head_kinds,
        [flux::types::kind::Kind::Type]
    );

    let encoded = serde_json::to_vec(&interface).expect("serialize interface");
    let decoded: ModuleInterface = serde_json::from_slice(&encoded).expect("reload interface");
    assert_eq!(decoded, interface);
    assert_eq!(
        decoded.interface_fingerprint,
        interface.interface_fingerprint
    );
}

/// Proposal 0179 Stage 5: a public class's superclasses reach a consumer with
/// their owning modules, so the `ClassId`s — and therefore the dictionary slot
/// layout — are identical on the warm path and the cold one.
#[test]
fn public_superclass_metadata_survives_interface_serialization_roundtrip() {
    let interface = build_interface_for("SuperclassMetadata");

    let measurable = interface
        .public_classes
        .iter()
        .find(|entry| entry.name == "Measurable")
        .expect("Measurable is public");
    assert_eq!(measurable.superclasses.len(), 1);
    // Parallel to `superclasses`, and the same length: the count decides how
    // many evidence slots the dictionary leads with.
    assert_eq!(
        measurable.superclass_class_modules,
        ["SuperclassMetadata"],
        "each superclass must carry its owning module"
    );

    let encoded = serde_json::to_vec(&interface).expect("serialize interface");
    let decoded: ModuleInterface = serde_json::from_slice(&encoded).expect("reload interface");
    assert_eq!(decoded, interface);
}

/// Proposal 0179 Stage 5: an interface that cannot supply every superclass
/// identity is rejected with E478, naming the class and what is missing.
///
/// A partially rebuilt class is worse than an absent one — the superclass
/// count decides how many evidence slots its dictionaries lead with, so one
/// rebuilt a slot short has every method read at the wrong index. Skipping it
/// silently was worse still: the failure resurfaced as an unrelated
/// duplicate-instance error about the instances that referenced the class.
#[test]
fn an_interface_missing_superclass_identities_is_reported_not_skipped() {
    let mut interface = build_interface_for("SuperclassMetadata");
    let measurable = interface
        .public_classes
        .iter_mut()
        .find(|entry| entry.name == "Measurable")
        .expect("Measurable is public");
    assert_eq!(measurable.superclasses.len(), 1);
    // What an interface written before the owning modules were recorded, or
    // truncated by a `#[serde(default)]` field, would look like.
    measurable.superclass_class_modules.clear();

    let mut compiler = Compiler::new_with_file_path("consumer.flx");
    compiler.preload_module_interface(&interface);

    let reported = compiler
        .errors
        .iter()
        .find(|diagnostic| diagnostic.code() == Some("E478"))
        .unwrap_or_else(|| {
            panic!(
                "expected E478, got: {:?}",
                compiler.errors.iter().map(|d| d.code()).collect::<Vec<_>>()
            )
        });
    let message = reported.message().unwrap_or_default();
    assert!(
        message.contains("Measurable"),
        "diagnostic should name the class: {message}"
    );
    assert!(
        message.contains("SuperclassMetadata"),
        "diagnostic should name the module: {message}"
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

/// Proposal 0179 Stage 3: `where Eq<a>` and `<a: Eq>` are two spellings of one
/// obligation, so both must produce the same behavior.
#[test]
fn where_and_bound_constraint_spellings_agree() {
    let output = run_fixture("where_constraint.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "true\nfalse");
}

/// Proposal 0179 Stage 3: a concrete predicate is discharged against an
/// instance.
#[test]
fn a_concrete_predicate_is_solved_against_its_instance() {
    let output = run_fixture("solved_constraint.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "21");
}

/// A predicate over a quantified variable is retained on the scheme and
/// discharged at the call site — the retained half of constraint splitting.
#[test]
fn a_quantified_predicate_is_generalized_onto_the_scheme() {
    let output =
        run_fixture("generalized_constraint.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "42");
}

/// A predicate mentioning an enclosing binding's variable is deferred outward
/// rather than dropped — the deferred half of `split`.
#[test]
fn an_outer_scope_predicate_is_deferred_not_dropped() {
    let output = run_fixture("stuck_constraint.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "\"5\"");
}

/// Numeric defaulting discharges the otherwise-ambiguous `Num` obligation.
#[test]
fn numeric_defaulting_discharges_an_ambiguous_num_obligation() {
    let output = run_fixture("diagnosed_constraint.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "3\n21");
}

/// Structured predicates keep their full arguments through generalization, and
/// two over the same variable no longer collide in deduplication.
#[test]
fn structured_predicates_survive_generalization_distinctly() {
    let output = run_fixture("generalized_structured_constraint.flx")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "\"7\"\n\"9\"");
}

/// Proposal 0179 Stage 5: a dictionary carries evidence for its superclasses,
/// so a function constrained on a subclass can call inherited methods —
/// through one projection for a direct superclass, two for a transitive one.
#[test]
fn superclass_methods_are_reachable_from_a_subclass_dictionary() {
    for (fixture, expected) in [
        ("superclass_method_call.flx", "505"),
        ("transitive_superclass.flx", "111"),
    ] {
        let output = run_fixture(fixture).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(output.stdout, expected, "unexpected output for {fixture}");
    }
}

/// Proposal 0179 Stage 5: an inherited method dispatches identically whether
/// the defining module was just compiled or reloaded from its interface — the
/// warm path must rebuild the same superclass identities, because they decide
/// how many evidence slots the dictionary leads with.
#[test]
fn superclass_dispatch_is_the_same_cold_and_warm() {
    let scratch = parity::scratch::Scratch::new("typeclass-superclass-cache");
    let fixture = fixture_path("superclass_across_modules.flx");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .args([fixture.to_str().expect("fixture path is UTF-8")])
            .args(scratch.cache_args())
            .output()
            .expect("run cross-module superclass fixture")
    };

    let cold = run();
    assert!(cold.status.success(), "{}", normalize_output(&cold.stderr));
    assert_eq!(normalize_output(&cold.stdout), "505");

    let warm = run();
    assert!(warm.status.success(), "{}", normalize_output(&warm.stderr));
    assert_eq!(
        normalize_output(&warm.stdout),
        normalize_output(&cold.stdout),
        "cached run disagrees with the cold one"
    );
}

/// KI-057: a function holding two dictionaries for one class dispatches each
/// call through the dictionary its argument belongs to, not through whichever
/// was recorded first.
///
/// The superclass case is included because Stage 5 gave every inherited method
/// a second route to the same implementation, which is exactly what a
/// name-keyed map collapses.
#[test]
fn two_dictionaries_for_one_class_are_told_apart() {
    for (fixture, expected) in [
        ("two_dictionaries_one_class.flx", "12"),
        ("two_dictionaries_superclass.flx", "507"),
    ] {
        let output = run_fixture(fixture).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(output.stdout, expected, "unexpected output for {fixture}");
    }
}

/// Proposal 0179 Stage 6: an imported class's associated types reduce the same
/// way whether the defining module was just compiled or reloaded from its
/// interface. The declaration and the equations travel separately, so the warm
/// path has to rebuild both or the application silently stays stuck.
#[test]
fn associated_type_reduction_is_the_same_cold_and_warm() {
    let scratch = parity::scratch::Scratch::new("typeclass-associated-type-cache");
    let fixture = fixture_path("associated_type_interface_roundtrip.flx");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .args([fixture.to_str().expect("fixture path is UTF-8")])
            .args(scratch.cache_args())
            .output()
            .expect("run cross-module associated type fixture")
    };

    let cold = run();
    assert!(cold.status.success(), "{}", normalize_output(&cold.stderr));
    assert_eq!(normalize_output(&cold.stdout), "7\n\"s\"");

    let warm = run();
    assert!(warm.status.success(), "{}", normalize_output(&warm.stderr));
    assert_eq!(
        normalize_output(&warm.stdout),
        normalize_output(&cold.stdout),
        "cached run disagrees with the cold one"
    );
}

/// Proposal 0179 Stage 6: an application whose arguments select an instance
/// reduces to that equation's body, and one that cannot reduce is preserved
/// until a call site fixes it.
#[test]
fn associated_types_reduce_when_selected_and_stay_stuck_otherwise() {
    for (fixture, expected) in [
        ("associated_type_reduction.flx", "7"),
        ("stuck_associated_type.flx", "7\n\"s\""),
    ] {
        let output = run_fixture(fixture).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(output.stdout, expected, "unexpected output for {fixture}");
    }
}

/// Proposal 0179 Stage 6: a stuck associated type must not unify with an
/// unrelated concrete type.
///
/// `first_of` returns `Element<c>`, so a function declaring `-> String` is
/// wrong for every instance whose `Element` is not `String`. The E300 guard
/// only reports when both sides are settled, and a stuck application over this
/// signature's own rigid variables counts as settled — without that this
/// compiled and returned whatever the selected instance produced.
#[test]
fn a_stuck_associated_type_does_not_unify_with_a_concrete_type() {
    let source = r#"
class Collection<c> {
    type Element<c>
    fn first_of(xs: c) -> Element<c>
}

instance Collection<List<Int>> {
    type Element<List<Int>> = Int
    fn first_of(xs) { 7 }
}

fn wrong<c: Collection>(xs: c) -> String {
    first_of(xs)
}

fn main() with IO {
    print(wrong([1, 2]))
}
"#;
    let (program, mut compiler) = parse_source(source, "stuck_mismatch.flx");
    let result = compiler.compile(&program);
    assert!(
        result.is_err() || !compiler.errors.is_empty(),
        "declaring `-> String` for a body of type `Element<c>` must be rejected"
    );
}

/// Proposal 0179 Stage 6: an associated type declared by a class and defined by
/// an instance reaches `ClassEnv`, so later steps have something to reduce.
///
/// Asserted on the environment rather than only through a passing fixture: a
/// parser that dropped the declarations would still compile this program,
/// because Step 1 adds no semantics that depend on them.
#[test]
fn associated_type_declarations_and_equations_reach_the_class_environment() {
    let source = std::fs::read_to_string(fixture_path("associated_type_declaration.flx"))
        .expect("read associated type fixture");
    let (program, mut compiler) = parse_source(&source, "associated_type_declaration.flx");
    compiler
        .compile(&program)
        .expect("associated type fixture compiles");

    let class = compiler
        .class_env()
        .classes
        .values()
        .find(|class| compiler.interner.resolve(class.name) == "Collection")
        .expect("Collection is collected");
    assert_eq!(class.associated_types.len(), 1);
    assert_eq!(
        compiler.interner.resolve(class.associated_types[0].name),
        "Element"
    );
    // Indexed by the class parameter it is declared over.
    assert_eq!(class.associated_types[0].params.len(), 1);

    let instance = compiler
        .class_env()
        .instances
        .iter()
        .find(|instance| compiler.interner.resolve(instance.class_name) == "Collection")
        .expect("the Collection instance is collected");
    assert_eq!(instance.associated_types.len(), 1);
    let equation = &instance.associated_types[0];
    assert_eq!(compiler.interner.resolve(equation.name), "Element");
    // The head repeats the instance head, and the body is what it reduces to.
    assert_eq!(equation.head.len(), 1);
    assert_eq!(equation.body.display_with(&compiler.interner), "a");
}

/// Proposal 0179 Stage 5: a superclass obligation is checked against the whole
/// program, so declaring the subclass instance above the superclass instance it
/// requires compiles exactly like the other order.
///
/// The check used to run inline while instances were collected, seeing only
/// what preceded it.
#[test]
fn a_superclass_obligation_does_not_depend_on_declaration_order() {
    let output =
        run_fixture("superclass_order_independent.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "500\n5");
}

/// Proposal 0183: a scheme keeps only the predicates nothing else implies.
///
/// `mconcat<a: Monoid>` raises `Semigroup<a>` in its body, and every `Monoid`
/// dictionary already carries `Semigroup` evidence in a superclass slot. Before
/// GHC's `mkMinimalBySCs` was applied the scheme retained both, so the function
/// took two dictionary parameters and every caller passed evidence it was
/// already handing over.
#[test]
fn a_scheme_drops_a_predicate_its_superclass_already_supplies() {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            "--dump-core",
            fixture_path("semigroup_monoid.flx")
                .to_str()
                .expect("fixture path is UTF-8"),
            "--no-cache",
        ])
        .output()
        .expect("dump core for the Monoid fixture");
    let dump = String::from_utf8_lossy(&output.stdout);

    let signature = dump
        .lines()
        .skip_while(|line| !line.starts_with("letrec mconcat"))
        .nth(1)
        .expect("mconcat is lowered with a parameter list");

    assert!(
        signature.contains("_Monoid"),
        "mconcat keeps the context it declared: {signature}"
    );
    assert!(
        !signature.contains("_Semigroup"),
        "Semigroup is implied by Monoid and must not become a second \
         dictionary parameter: {signature}"
    );
}

/// Proposal 0183: a signature that names its context owns it.
///
/// `cmp` promises `MyEq<a>` and its body calls a `MyOrd` method. Inferring the
/// missing predicate onto the scheme made the signature a suggestion and moved
/// the error to whichever caller used a type without a `MyOrd` instance —
/// a file the author of the mistake may never open. GHC reports it here, as
/// case (P2) of Note [Constraints in partial type signatures].
#[test]
fn a_body_may_not_need_more_than_its_signature_declares() {
    let source = r#"
class MyEq<a> {
    fn meq(x: a, y: a) -> Bool
}

class MyOrd<a> {
    fn mlt(x: a, y: a) -> Bool
}

instance MyEq<Int> { fn meq(x, y) { x == y } }
instance MyOrd<Int> { fn mlt(x, y) { x < y } }

fn cmp<a: MyEq>(x: a, y: a) -> Bool {
    mlt(x, y)
}

fn main() with IO { print(cmp(1, 2)) }
"#;
    let (program, mut compiler) = parse_source(source, "undeclared_context.flx");
    let errors = compiler
        .compile(&program)
        .expect_err("a body needing more than its signature grants must not compile");

    let deduce = errors
        .iter()
        .find(|diag| diag.code() == Some("E489"))
        .unwrap_or_else(|| panic!("expected E489, got: {errors:?}"));
    let message = deduce.message().unwrap_or_default();
    assert!(
        message.contains("MyOrd<a>") && message.contains("MyEq<a>"),
        "the message names both the predicate and the context it was checked \
         against, with the signature's own variable: {message}"
    );
}

/// The other half of the same rule: a signature that names *no* bound has not
/// specified a context, so inference still supplies one. Without this,
/// `fn list_size<a>(value: List<a>)` would stop compiling.
#[test]
fn a_signature_without_bounds_still_infers_its_context() {
    let output = run_fixture("structured_predicate.flx").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.stdout, "7");
}

/// An instance context discharges a subgoal that is not ground.
///
/// `Dec<List<a>>`'s body decodes through a `Box`, so it needs `Dec<Box<a>>`,
/// which reduces to the `Dec<a>` its own context grants. The instance search
/// used to abandon any subgoal carrying a type variable — right only while
/// there was no context to appeal to, and the reason `Flow.Json`'s
/// `Decode<List<a>>` (which decodes through an array) stopped compiling when
/// the residue was first reported.
#[test]
fn an_instance_context_discharges_a_non_ground_subgoal() {
    let source = r#"
class Dec<a> {
    fn dec(x: Int) -> a
}

data Box<a> { Box(a) }

instance Dec<Int> {
    fn dec(x) { x }
}

instance Dec<a> => Dec<Box<a>> {
    fn dec(x) { Box(dec(x)) }
}

instance Dec<a> => Dec<List<a>> {
    fn dec(x) {
        match dec(x) {
            Box(v) -> [v]
        }
    }
}

fn main() with IO {
    let xs: List<Int> = dec(1)
    print(len(xs))
}
"#;
    let (program, mut compiler) = parse_source(source, "non_ground_subgoal.flx");
    compiler
        .compile(&program)
        .expect("a subgoal the instance context grants must resolve");
}

/// `+` resolves at `String` even when the operand's type arrives later.
///
/// In a lambda handed to a higher-order function both operands are still
/// variables when the operator is inferred, so the obligation is emitted over
/// a variable and only afterwards resolved to `String`. While `+` was a `Num`
/// method this reported `Num<String>` as a missing instance; `Flow.Add` gives
/// it a real instance to match, so the type it acquires later is one the
/// solver can discharge.
#[test]
fn addition_over_strings_survives_a_deferred_operand() {
    let source = r#"
fn apply(f, a, b) { f(a, b) }

fn main() with IO {
    print(apply(fn(x, y) { x + y }, "a", "b"))
}
"#;
    let (program, mut compiler) = parse_source(source, "deferred_string_add.flx");
    compiler
        .compile(&program)
        .expect("`+` over strings is a built-in rule, not a missing `Num` instance");
}

/// The other half: `String` has `Add` and not `Num`, so it gains `+` without
/// gaining the rest of arithmetic. `-` emits a `Num` predicate, which no
/// `String` instance satisfies, so a deferred subtraction stays an error.
#[test]
fn subtraction_over_strings_is_still_rejected() {
    let source = r#"
fn sub(a, b) { a - b }

fn main() with IO {
    print(sub("a", "b"))
}
"#;
    let (program, mut compiler) = parse_source(source, "deferred_string_sub.flx");
    assert!(
        compiler.compile(&program).is_err(),
        "only `+` is overloaded over `String`"
    );
}

/// A pattern variable keeps the scrutinee's element type, so an obligation
/// raised on it is one the enclosing signature's context can discharge.
///
/// `match xs { [h | t] -> ..., _ -> ... }` mixes a constraining arm with a
/// non-constraining one. Arm isolation used to hand every arm a fresh variable
/// whenever the scrutinee was not *fully* concrete, and `List<a>` inside
/// `fn f<a: C>(..)` is not — so `h` lost its connection to `a` and the `C<h>`
/// obligation could not be matched against the `C<a>` the signature grants
/// (docs/known_issues.md#ki-080). Only the head constructor decides a pattern
/// family, so `List<a>` settles it as well as `List<Int>` does.
#[test]
fn a_pattern_variable_keeps_the_scrutinees_element_type() {
    let source = r#"
class MyEq<a> {
    fn meq(x: a, y: a) -> Bool
}

instance MyEq<Int> {
    fn meq(x, y) { x == y }
}

fn viapat<a: MyEq>(xs: List<a>) -> Bool {
    match xs {
        [h | t] -> meq(h, h),
        _ -> true
    }
}

fn main() with IO {
    print(viapat([1, 2]))
}
"#;
    let (program, mut compiler) = parse_source(source, "pattern_element_type.flx");
    let result = compiler.compile(&program);
    assert!(
        result.is_ok(),
        "the signature's own context must discharge the obligation: {:?}",
        result.err()
    );
}

/// The other half: isolating an arm must still not hide a real mismatch. A
/// pattern variable bound from a `List<Int>` scrutinee is an `Int`, and using
/// it as a `String` is an error whichever arms accompany it.
#[test]
fn a_pattern_variable_from_a_concrete_scrutinee_is_still_checked() {
    let source = r#"
fn f(xs: List<Int>) -> Int {
    match xs {
        [h | t] -> h + "oops",
        _ -> 0
    }
}

fn main() with IO {
    print(f([1, 2]))
}
"#;
    let (program, mut compiler) = parse_source(source, "pattern_mismatch.flx");
    assert!(
        compiler.compile(&program).is_err(),
        "an `Int` element used as a `String` is a mismatch"
    );
}
