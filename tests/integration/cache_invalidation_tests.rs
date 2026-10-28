//! Regression tests for cache invalidation behavior.
//!
//! Verifies that private changes preserve dependent caches (interface
//! fingerprint unchanged) and public changes invalidate them (interface
//! fingerprint changed).

use std::path::Path;
use std::process::Command;

#[path = "../support/mod.rs"]
mod support;
use crate::support::scratch;
use scratch::Scratch;

use flux::{
    bytecode::bytecode_cache::hash_bytes,
    compiler::{Compiler, module_interface},
    syntax::{lexer::Lexer, parser::Parser},
    types::module_interface::ModuleInterface,
};

#[test]
fn semantic_config_hash_is_default_eliding_and_optimize_aware() {
    let default = module_interface::compute_semantic_config_hash(false, false);
    let default_document = hash_bytes(b"");
    let optimized = module_interface::compute_semantic_config_hash(false, true);
    let strict = module_interface::compute_semantic_config_hash(true, false);

    assert_eq!(default, default_document);
    assert_ne!(default, optimized);
    assert_ne!(default, strict);
    assert_ne!(optimized, strict);
}

/// `-g` keys its own compiled artifacts (proposal 0188), while a run without
/// it keeps the keys every existing cache was written under. Interfaces do
/// not depend on the debug level.
#[test]
fn artifact_config_hash_keys_each_debug_level() {
    use flux::debuginfo::DebugLevel;
    let default = module_interface::compute_artifact_config_hash(false, false, DebugLevel::None);
    let lines = module_interface::compute_artifact_config_hash(false, false, DebugLevel::Lines);
    let full = module_interface::compute_artifact_config_hash(false, false, DebugLevel::Full);
    let optimized_full =
        module_interface::compute_artifact_config_hash(false, true, DebugLevel::Full);

    assert_eq!(default, hash_bytes(b""));
    assert_eq!(
        module_interface::compute_artifact_config_hash(true, true, DebugLevel::None),
        module_interface::compute_semantic_config_hash(true, true),
        "without -g the artifact and semantic hashes must agree"
    );
    assert_ne!(default, lines);
    assert_ne!(default, full);
    assert_ne!(lines, full);
    assert_ne!(full, optimized_full);
}

fn compile_and_build_interface(source: &str) -> ModuleInterface {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let mut compiler = Compiler::new_with_interner("test.flx".to_string(), interner);
    compiler.compile_with_opts(&program, false, false).expect("compilation should succeed");

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
        Some(&program),
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
    assert!(!module_interface::module_interface_changed(&iface1, &iface2));
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

/// A cache entry written by one build of the compiler must not be reused by a
/// different build of the same version — see `docs/known_issues.md#ki-079`.
///
/// `FLUX_BUILD_ID` stands in for the executable fingerprint the compiler
/// normally derives from its own binary: a test cannot rebuild the compiler,
/// but pinning the identity exercises the same key.
#[test]
fn a_debug_build_does_not_reuse_modules_cached_without_debug() {
    let scratch = Scratch::new("debug-level-cache");
    let program = scratch.write("main.flx", "fn main() with IO { print(\"hi\") }\n");
    let home = scratch.join("flux-home");

    let run = |extra: &[&str]| -> String {
        let output = Command::new(Path::new(env!("CARGO_BIN_EXE_flux")))
            .arg(&program)
            .args(extra)
            .args(scratch.cache_args())
            .env("FLUX_HOME", &home)
            .output()
            .expect("run flux");
        assert!(
            output.status.success(),
            "flux {extra:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    };

    run(&[]);
    let debug = run(&["-g"]);
    assert!(
        debug.contains("Compiling  main"),
        "a -g run must not reuse a module cached without -g, got:\n{debug}"
    );
    let debug_again = run(&["-g"]);
    assert!(
        debug_again.contains("Cached     main"),
        "a -g run should reuse its own cache, got:\n{debug_again}"
    );
    // Not asserted: that a plain run after a -g run reuses `main`. The -g run
    // recompiles the dependencies, and an interface fingerprint is not stable
    // across rebuilds of the same source, so `main` sees its dependencies'
    // interfaces as changed. That instability predates the debug level.
}

#[test]
fn a_different_compiler_build_does_not_reuse_cached_modules() {
    let scratch = Scratch::new("ki079-build-id");
    let program = scratch.write("main.flx", "fn main() with IO { print(\"hi\") }\n");
    let home = scratch.join("flux-home");

    let run = |build_id: &str| -> String {
        let output = Command::new(Path::new(env!("CARGO_BIN_EXE_flux")))
            .arg(&program)
            .args(scratch.cache_args())
            .env("FLUX_BUILD_ID", build_id)
            .env("FLUX_HOME", &home)
            .output()
            .expect("run flux");
        assert!(
            output.status.success(),
            "flux failed under build {build_id}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    };

    run("build-a");
    let warm = run("build-a");
    assert!(
        warm.contains("Cached     main"),
        "the same build should reuse its own cache, got:\n{warm}"
    );

    let other_build = run("build-b");
    assert!(
        other_build.contains("Compiling  main"),
        "a different build must recompile rather than reuse, got:\n{other_build}"
    );
}
