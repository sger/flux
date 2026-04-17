//! Regression tests for cache invalidation behavior.
//!
//! Verifies that private changes preserve dependent caches (interface
//! fingerprint unchanged) and public changes invalidate them (interface
//! fingerprint changed).

use flux::{
    bytecode::compiler::{Compiler, module_interface},
    syntax::{lexer::Lexer, parser::Parser},
    types::module_interface::ModuleInterface,
};

fn compile_and_build_interface(source: &str) -> ModuleInterface {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let mut compiler = Compiler::new_with_interner("test.flx".to_string(), interner);
    compiler
        .compile_with_opts(&program, false, false)
        .expect("compilation should succeed");

    let source_hash = flux::bytecode::bytecode_cache::hash_bytes(source.as_bytes());
    let config_hash = module_interface::compute_semantic_config_hash(false, false);

    let core = compiler
        .lower_aether_report_program(&program, false)
        .expect("Core lowering should succeed");
    let exported_runtime_contracts = compiler.exported_runtime_contracts();

    module_interface::build_interface(
        "Test",
        compiler.interner.intern("Test"),
        &source_hash,
        &config_hash,
        core.as_core(),
        compiler.cached_member_schemes(),
        &exported_runtime_contracts,
        &compiler.module_function_visibility,
        Some(compiler.class_env()),
        Vec::new(),
        &compiler.interner,
    )
}

#[test]
fn private_body_change_preserves_interface_fingerprint() {
    let v1 = r#"
        module Test {
            fn helper(x) { x + 1 }
            public fn answer() -> Int { helper(41) }
        }
    "#;
    let v2 = r#"
        module Test {
            fn helper(x) { 40 + 2 }
            public fn answer() -> Int { helper(0) }
        }
    "#;

    let iface1 = compile_and_build_interface(v1);
    let iface2 = compile_and_build_interface(v2);

    assert_eq!(
        iface1.interface_fingerprint, iface2.interface_fingerprint,
        "private body change should not change interface fingerprint"
    );
    assert!(!module_interface::module_interface_changed(
        &iface1, &iface2
    ));
}

#[test]
fn new_public_export_changes_interface_fingerprint() {
    let v1 = r#"
        module Test {
            public fn answer() -> Int { 42 }
        }
    "#;
    let v2 = r#"
        module Test {
            public fn answer() -> Int { 42 }
            public fn bonus() -> Int { 99 }
        }
    "#;

    let iface1 = compile_and_build_interface(v1);
    let iface2 = compile_and_build_interface(v2);

    assert_ne!(
        iface1.interface_fingerprint, iface2.interface_fingerprint,
        "new public export should change interface fingerprint"
    );
    assert!(module_interface::module_interface_changed(&iface1, &iface2));
}

#[test]
fn removed_public_export_changes_interface_fingerprint() {
    let v1 = r#"
        module Test {
            public fn answer() -> Int { 42 }
            public fn bonus() -> Int { 99 }
        }
    "#;
    let v2 = r#"
        module Test {
            public fn answer() -> Int { 42 }
        }
    "#;

    let iface1 = compile_and_build_interface(v1);
    let iface2 = compile_and_build_interface(v2);

    assert_ne!(
        iface1.interface_fingerprint, iface2.interface_fingerprint,
        "removed public export should change interface fingerprint"
    );
    assert!(module_interface::module_interface_changed(&iface1, &iface2));
}

#[test]
fn private_to_public_changes_interface_fingerprint() {
    let v1 = r#"
        module Test {
            fn helper() -> Int { 42 }
            public fn answer() -> Int { 1 }
        }
    "#;
    let v2 = r#"
        module Test {
            public fn helper() -> Int { 42 }
            public fn answer() -> Int { 1 }
        }
    "#;

    let iface1 = compile_and_build_interface(v1);
    let iface2 = compile_and_build_interface(v2);

    assert_ne!(
        iface1.interface_fingerprint, iface2.interface_fingerprint,
        "making a private function public should change interface fingerprint"
    );
}

#[test]
fn comment_only_change_preserves_interface_fingerprint() {
    let v1 = r#"
        module Test {
            public fn answer() -> Int { 42 }
        }
    "#;
    let v2 = r#"
        // This is a comment
        module Test {
            // Another comment
            public fn answer() -> Int { 42 }
        }
    "#;

    let iface1 = compile_and_build_interface(v1);
    let iface2 = compile_and_build_interface(v2);

    assert_eq!(
        iface1.interface_fingerprint, iface2.interface_fingerprint,
        "comment-only changes should not change interface fingerprint"
    );
}

#[test]
fn private_helper_added_preserves_interface_fingerprint() {
    let v1 = r#"
        module Test {
            public fn answer() -> Int { 42 }
        }
    "#;
    let v2 = r#"
        module Test {
            fn unused_helper() -> Int { 99 }
            public fn answer() -> Int { 42 }
        }
    "#;

    let iface1 = compile_and_build_interface(v1);
    let iface2 = compile_and_build_interface(v2);

    assert_eq!(
        iface1.interface_fingerprint, iface2.interface_fingerprint,
        "adding a private helper should not change interface fingerprint"
    );
}
