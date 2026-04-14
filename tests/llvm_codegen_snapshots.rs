#![cfg(feature = "llvm")]

use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::llvm::{LlvmModule, emit_prelude_and_arith, render_module};

#[test]
fn emitted_prelude_and_arith_snapshot() {
    let mut module = LlvmModule {
        source_filename: Some("helpers.flx".into()),
        target_triple: Some("x86_64-unknown-linux-gnu".into()),
        data_layout: Some("e-m:e-p:64:64-i64:64-n8:16:32:64-S128".into()),
        ..LlvmModule::new()
    };
    emit_prelude_and_arith(&mut module);
    emit_prelude_and_arith(&mut module);

    insta::with_settings!({
        snapshot_path => "snapshots/llvm",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_snapshot!("emitted_prelude_and_arith", render_module(&module));
    });
}

#[test]
fn emitted_prelude_and_arith_opt_verify_if_available() {
    if Command::new("opt").arg("--version").output().is_err() {
        return;
    }

    let mut module = LlvmModule::new();
    emit_prelude_and_arith(&mut module);
    let ll = render_module(&module);
    let temp = std::env::temp_dir().join(format!(
        "llvm_phase2_{}.ll",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos()
    ));

    fs::write(&temp, ll).expect("write ll file");
    let output = Command::new("opt")
        .arg("--disable-output")
        .arg("-passes=verify")
        .arg(&temp)
        .output()
        .expect("run opt");
    let _ = fs::remove_file(&temp);

    assert!(
        output.status.success(),
        "opt verification failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
