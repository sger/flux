//! Natively, the C compiler is asked what it is once per `flux` process.
//! Each `cc --version` costs about 25 ms, and a build used to ask once per
//! module compiled and again for the runtime.

#![cfg(all(feature = "llvm", unix))]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::support::flux_runner::{flux_source, scratch::Scratch};

/// The `cc` that `PATH` finds, so the stand-in can pass every call on to it.
fn real_cc() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&path)
        .map(|dir| dir.join("cc"))
        .find(|cc| cc.is_file())
        .expect("a cc on PATH")
}

#[test]
fn a_native_build_asks_cc_for_its_version_once() {
    let scratch = Scratch::new("cc-probe");
    let log = scratch.join("cc.log");
    let shim = scratch.write(
        "bin/cc",
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real_cc().display()
        ),
    );
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
        .expect("make the stand-in executable");
    let path = format!(
        "{}:{}",
        scratch.join("bin").display(),
        std::env::var("PATH").expect("PATH is set")
    );

    // `--no-cache` compiles every stdlib module, each a chance to ask again.
    // `CC=cc` keeps a `CC` from the environment from bypassing the stand-in.
    let ran = flux_source("fn main() with IO {\n    print(1)\n}\n", "cc-probe")
        .native()
        .no_cache()
        .env("CC", "cc")
        .env("PATH", &path)
        .run();
    assert!(ran.success, "the build failed:\n{}", ran.stderr);
    assert_eq!(ran.stdout.trim(), "1");

    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    let probes = calls.lines().filter(|line| line.trim() == "--version").count();
    assert_eq!(probes, 1, "cc was called with:\n{calls}");
}
