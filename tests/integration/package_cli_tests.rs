//! `flux init` and `flux new` scaffold packages.
//!
//! Both commands forward to `Flume.Cli`, which owns every packaging decision:
//! the manifest template, the namespace derivation, and whether a package gets
//! `src/main.flx` or a namespace root module. These tests exercise the whole
//! path through the real CLI and then build what it produced.

use std::path::Path;
use std::process::{Command, Output};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::{Scratch, cache_args_for, workspace_root};

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn flux(args: &[&str], cwd: &Path) -> Output {
    Command::new(flux_bin())
        .current_dir(cwd)
        .args(args)
        // Keep package tests on the project-local target cache. In
        // particular, never derive a repository-level `.flux-cache` from the
        // process working directory.
        .args(cache_args_for(&cwd.join("target").join("flux")))
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux")
}

fn combined(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    text.push_str(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
    text
}

#[test]
fn package_tests_do_not_leave_legacy_repository_caches() {
    let root = workspace_root();
    assert!(
        !root.join(".flux-cache").exists(),
        "package tests must not create a repository .flux-cache"
    );
    assert!(
        !root.join(".flux").join("cache").exists(),
        "package tests must not create a repository .flux/cache"
    );
}

#[test]
fn init_scaffolds_a_runnable_binary_package() {
    let scratch = Scratch::new("cli-init-bin");
    let pkg = scratch.path().join("mypkg");
    std::fs::create_dir_all(&pkg).expect("create package dir");

    let out = flux(&["init"], &pkg);
    assert!(out.status.success(), "init failed:\n{}", combined(&out));

    assert!(pkg.join("flux.toml").is_file(), "no manifest was written");
    let manifest = std::fs::read_to_string(pkg.join("flux.toml")).expect("read manifest");
    // The package is named for its directory when no name is given.
    assert!(manifest.contains("name = \"mypkg\""), "{manifest}");

    // What init produced must actually build and run.
    let run = flux(&["src/main.flx"], &pkg);
    assert!(
        run.status.success(),
        "scaffold failed to run:\n{}",
        combined(&run)
    );
    assert!(
        combined(&run).contains("Hello from Flux"),
        "unexpected output:\n{}",
        combined(&run)
    );
}

/// `--lib` produces the namespace root module that is the package's public
/// face, at `src/<Namespace>.flx`.
#[test]
fn new_lib_derives_a_namespace_from_the_package_name() {
    let scratch = Scratch::new("cli-new-lib");

    let out = flux(&["new", "http-client", "--lib"], scratch.path());
    assert!(out.status.success(), "new failed:\n{}", combined(&out));

    let pkg = scratch.path().join("http-client");
    // `http-client` derives the namespace `HttpClient`.
    let module = pkg.join("src").join("HttpClient.flx");
    assert!(
        module.is_file(),
        "expected src/HttpClient.flx, found: {:?}",
        std::fs::read_dir(pkg.join("src"))
            .map(|e| e
                .filter_map(Result::ok)
                .map(|e| e.path())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
    let source = std::fs::read_to_string(&module).expect("read module");
    assert!(source.contains("module HttpClient"), "{source}");
}

#[test]
fn init_refuses_to_overwrite_an_existing_package() {
    let scratch = Scratch::new("cli-init-twice");
    let pkg = scratch.path().join("pkg");
    std::fs::create_dir_all(&pkg).expect("create package dir");

    assert!(flux(&["init"], &pkg).status.success(), "first init failed");

    let second = flux(&["init"], &pkg);
    assert!(
        !second.status.success(),
        "a second init must fail:\n{}",
        combined(&second)
    );
    assert!(
        combined(&second).contains("already exists"),
        "unexpected message:\n{}",
        combined(&second)
    );
}

#[test]
fn new_requires_a_package_name() {
    let scratch = Scratch::new("cli-new-no-name");
    let out = flux(&["new"], scratch.path());
    assert!(!out.status.success(), "expected a usage error");
    assert!(
        combined(&out).contains("Usage: flux new"),
        "unexpected message:\n{}",
        combined(&out)
    );
}

/// A scaffolded package is a real project: its path dependencies resolve, so
/// `init` composes with the manifest root resolution from earlier in the phase.
#[test]
fn a_scaffolded_package_can_depend_on_another() {
    let scratch = Scratch::new("cli-init-deps");

    assert!(
        flux(&["new", "shared", "--lib"], scratch.path())
            .status
            .success(),
        "creating the dependency failed"
    );
    assert!(
        flux(&["new", "app"], scratch.path()).status.success(),
        "creating the app failed"
    );

    let app = scratch.path().join("app");
    let manifest = std::fs::read_to_string(app.join("flux.toml")).expect("read manifest");
    std::fs::write(
        app.join("flux.toml"),
        format!("{manifest}\n[dependencies]\nshared = {{ path = \"../shared\" }}\n"),
    )
    .expect("write manifest");
    std::fs::write(
        app.join("src").join("main.flx"),
        "import Shared as Shared\n\nfn main() with IO { print(Shared.greet()) }\n",
    )
    .expect("write main");

    let run = flux(&["src/main.flx"], &app);
    assert!(
        run.status.success(),
        "cross-package build failed:\n{}",
        combined(&run)
    );
    assert!(
        combined(&run).contains("Hello from Shared"),
        "unexpected output:\n{}",
        combined(&run)
    );
}

/// KI-019: a command that reports an error must exit non-zero, or scripts and
/// CI cannot detect the failure.
#[test]
fn failing_commands_exit_non_zero() {
    let scratch = Scratch::new("cli-exit-codes");
    let missing = "does-not-exist.flx";

    for command in [
        vec![missing],
        vec!["fmt", missing],
        vec!["tokens", missing],
        vec!["bytecode", missing],
        vec!["lint", missing],
        vec!["cache-info", missing],
        vec!["module-cache-info", missing],
        vec!["native-cache-info", missing],
        vec!["interface-info", missing],
    ] {
        let out = flux(&command, scratch.path());
        assert!(
            !out.status.success(),
            "`flux {}` exited 0 despite failing:\n{}",
            command.join(" "),
            combined(&out)
        );
    }
}

/// The other half of KI-019: fixing the failure path must not make successful
/// commands exit non-zero.
#[test]
fn succeeding_commands_still_exit_zero() {
    let scratch = Scratch::new("cli-exit-codes-ok");
    std::fs::write(
        scratch.path().join("ok.flx"),
        "fn main() with IO { print(\"ok\") }\n",
    )
    .expect("write source");

    for command in [
        vec!["ok.flx"],
        vec!["tokens", "ok.flx"],
        vec!["lint", "ok.flx"],
        vec!["cache-info", "ok.flx"],
        vec!["module-cache-info", "ok.flx"],
    ] {
        let out = flux(&command, scratch.path());
        assert!(
            out.status.success(),
            "`flux {}` failed unexpectedly:\n{}",
            command.join(" "),
            combined(&out)
        );
    }
}

/// `flux build` / `run` / `test` / `check` operate on the current package,
/// with the entry point chosen by `Flume.Cli`.
#[test]
fn package_commands_operate_on_the_current_package() {
    let scratch = Scratch::new("cli-package-cmds");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");

    std::fs::write(
        pkg.join("src").join("main.flx"),
        "fn main() with IO { print(\"ran\") }\n\nfn test_arith() { assert_eq(1 + 1, 2) }\n",
    )
    .expect("write main");

    let run = flux(&["run"], &pkg);
    assert!(run.status.success(), "run failed:\n{}", combined(&run));
    assert!(combined(&run).contains("ran"), "{}", combined(&run));

    // build and check compile without executing.
    for command in ["build", "check"] {
        let out = flux(&[command], &pkg);
        assert!(
            out.status.success(),
            "{command} failed:\n{}",
            combined(&out)
        );
        assert!(
            !combined(&out).contains("ran"),
            "{command} must not run the program:\n{}",
            combined(&out)
        );
    }

    let test = flux(&["test"], &pkg);
    assert!(test.status.success(), "test failed:\n{}", combined(&test));
    assert!(
        combined(&test).contains("1 passed"),
        "unexpected test summary:\n{}",
        combined(&test)
    );
}

#[test]
fn package_metadata_reports_the_selected_profile() {
    let scratch = Scratch::new("cli-package-profile-metadata");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");
    std::fs::write(
        pkg.join("flux.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[profile.release]\noptimize = false\n",
    )
    .expect("write manifest");

    let out = flux(
        &["metadata", "--profile", "release", "--format", "json"],
        &pkg,
    );
    assert!(out.status.success(), "metadata failed:\n{}", combined(&out));
    let value: serde_json::Value = serde_json::from_str(&combined(&out)).expect("metadata json");
    assert_eq!(value["targets"]["backend"], "native");
    assert_eq!(value["targets"]["profile"], "release");
    assert_eq!(value["targets"]["optimize"], false);
}

#[test]
fn build_plan_reports_backend_and_profile() {
    let scratch = Scratch::new("cli-package-plan-profile");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");

    let out = flux(&["build", "--plan"], &pkg);
    assert!(
        out.status.success(),
        "build plan failed:\n{}",
        combined(&out)
    );
    let value: serde_json::Value = serde_json::from_str(&combined(&out)).expect("plan json");
    assert_eq!(value["format_version"], 1);
    assert_eq!(value["units"][0]["backend"], "vm");
    assert_eq!(value["units"][0]["profile"], "dev");
}

#[test]
fn profile_is_rejected_for_standalone_source_files() {
    let scratch = Scratch::new("cli-profile-source");
    let source = scratch.write("main.flx", "fn main() with IO { print(\"ok\") }\n");
    let out = flux(
        &[
            source.file_name().unwrap().to_str().unwrap(),
            "--profile",
            "release",
        ],
        scratch.path(),
    );
    assert!(!out.status.success(), "profile should be package-only");
    assert!(
        combined(&out).contains("--profile applies to package commands"),
        "unexpected output:\n{}",
        combined(&out)
    );
}

#[test]
fn unknown_package_profile_fails_before_building() {
    let scratch = Scratch::new("cli-profile-unknown");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");

    let out = flux(&["build", "--profile", "shipping"], &pkg);
    assert!(!out.status.success(), "unknown profile should fail");
    assert!(
        combined(&out).contains("unknown profile `shipping`")
            && combined(&out).contains("expected `dev` or `release`"),
        "unexpected output:\n{}",
        combined(&out)
    );
}

/// A compile error must fail `build` and `check`, or CI cannot use them.
#[test]
fn build_and_check_fail_on_a_compile_error() {
    let scratch = Scratch::new("cli-package-broken");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");
    std::fs::write(
        pkg.join("src").join("main.flx"),
        "fn main() with IO { print(1 + \"x\") }\n",
    )
    .expect("write main");

    for command in ["build", "check"] {
        let out = flux(&[command], &pkg);
        assert!(
            !out.status.success(),
            "{command} exited 0 despite a type error:\n{}",
            combined(&out)
        );
    }
}

/// `--bin` selects a named target; a bare `run` keeps using `src/main.flx`
/// even when other binaries are declared.
#[test]
fn run_bin_selects_a_named_target() {
    let scratch = Scratch::new("cli-package-bin");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");

    std::fs::write(
        pkg.join("src").join("main.flx"),
        "fn main() with IO { print(\"default-target\") }\n",
    )
    .expect("write main");
    std::fs::create_dir_all(pkg.join("src").join("bin")).expect("create bin dir");
    std::fs::write(
        pkg.join("src").join("bin").join("tool.flx"),
        "fn main() with IO { print(\"named-target\") }\n",
    )
    .expect("write tool");
    let manifest = std::fs::read_to_string(pkg.join("flux.toml")).expect("read manifest");
    std::fs::write(
        pkg.join("flux.toml"),
        format!("{manifest}\n[[bin]]\nname = \"tool\"\npath = \"src/bin/tool.flx\"\n"),
    )
    .expect("write manifest");

    let named = flux(&["run", "--bin", "tool"], &pkg);
    assert!(
        named.status.success(),
        "--bin failed:\n{}",
        combined(&named)
    );
    assert!(
        combined(&named).contains("named-target"),
        "{}",
        combined(&named)
    );

    // A declared [[bin]] must not displace the conventional entry point.
    let default = flux(&["run"], &pkg);
    assert!(
        combined(&default).contains("default-target"),
        "bare run picked the wrong target:\n{}",
        combined(&default)
    );

    let missing = flux(&["run", "--bin", "nope"], &pkg);
    assert!(!missing.status.success(), "an unknown --bin must fail");
}

/// Outside a project these commands have nothing to act on and must say so.
#[test]
fn package_commands_require_a_manifest() {
    let scratch = Scratch::new("cli-package-no-manifest");
    let out = flux(&["build"], scratch.path());
    assert!(
        !out.status.success(),
        "expected a failure outside a project"
    );
    assert!(
        combined(&out).contains("flux.toml"),
        "the error must name the missing manifest:\n{}",
        combined(&out)
    );
}

/// KI-021: the test path resolved modules with unscoped roots, so a package
/// that built and ran failed to compile under `flux test`.
#[test]
fn test_sees_path_dependencies() {
    let scratch = Scratch::new("cli-test-deps");

    assert!(
        flux(&["new", "shared", "--lib"], scratch.path())
            .status
            .success(),
        "creating the dependency failed"
    );
    assert!(
        flux(&["new", "app"], scratch.path()).status.success(),
        "creating the app failed"
    );

    let app = scratch.path().join("app");
    let manifest = std::fs::read_to_string(app.join("flux.toml")).expect("read manifest");
    std::fs::write(
        app.join("flux.toml"),
        format!("{manifest}\n[dependencies]\nshared = {{ path = \"../shared\" }}\n"),
    )
    .expect("write manifest");
    std::fs::write(
        app.join("src").join("main.flx"),
        "import Shared as Shared\n\n\
         fn main() with IO { print(Shared.greet()) }\n\n\
         fn test_dependency_is_visible() {\n\
         \u{20}   assert_eq(Shared.greet(), \"Hello from Shared!\")\n\
         }\n",
    )
    .expect("write main");

    // The run path already worked; the test path is what regressed.
    let run = flux(&["run"], &app);
    assert!(run.status.success(), "run failed:\n{}", combined(&run));

    let test = flux(&["test"], &app);
    assert!(
        test.status.success(),
        "test must see the dependency:\n{}",
        combined(&test)
    );
    assert!(
        combined(&test).contains("1 passed"),
        "unexpected test summary:\n{}",
        combined(&test)
    );
}

/// KI-020: `flux test` collected tests only from the entry file, so tests in a
/// package's other modules were silently never run.
#[test]
fn test_discovers_tests_in_every_module() {
    let scratch = Scratch::new("cli-test-discovery");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");

    std::fs::write(
        pkg.join("src").join("main.flx"),
        "import App.Extra as Extra\n\n\
         fn main() with IO { print(Extra.helper()) }\n\n\
         fn test_in_entry() { assert_eq(1, 1) }\n",
    )
    .expect("write main");
    std::fs::create_dir_all(pkg.join("src").join("App")).expect("create module dir");
    std::fs::write(
        pkg.join("src").join("App").join("Extra.flx"),
        "module App.Extra {\n\
         \u{20}   public fn helper() -> Int { 42 }\n\
         \u{20}   public fn test_in_module() { assert_eq(helper(), 42) }\n\
         }\n",
    )
    .expect("write module");

    let out = flux(&["test"], &pkg);
    assert!(out.status.success(), "test failed:\n{}", combined(&out));
    assert!(
        combined(&out).contains("2 tests"),
        "both modules' tests must run:\n{}",
        combined(&out)
    );
    assert!(
        combined(&out).contains("App.Extra.test_in_module"),
        "a module test must be reported by its qualified name:\n{}",
        combined(&out)
    );
}

/// `flux build` and `flux run` must be freely interleavable. `build` stops
/// before execution and so compiles serially, while `run` takes the parallel
/// VM fast path; a build that wrote module artifacts the run could not consume
/// failed with "missing global mapping for local index".
#[test]
fn build_and_check_do_not_poison_a_later_run() {
    let scratch = Scratch::new("cli-build-then-run");
    let pkg = scratch.path().join("app");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    assert!(flux(&["init"], &pkg).status.success(), "init failed");

    for first in ["build", "check", "test"] {
        let pre = flux(&[first], &pkg);
        assert!(pre.status.success(), "{first} failed:\n{}", combined(&pre));

        let run = flux(&["run"], &pkg);
        assert!(
            run.status.success(),
            "run after {first} failed:\n{}",
            combined(&run)
        );
        assert!(
            combined(&run).contains("Hello from Flux"),
            "run after {first} produced no output:\n{}",
            combined(&run)
        );
    }
}

/// Resolved package roots are cached against every manifest that produced
/// them, so editing a dependency's manifest is picked up rather than serving a
/// stale answer.
#[test]
fn editing_a_manifest_invalidates_the_cached_roots() {
    let scratch = Scratch::new("cli-roots-cache");

    assert!(
        flux(&["new", "shared", "--lib"], scratch.path())
            .status
            .success(),
        "creating the dependency failed"
    );
    assert!(
        flux(&["new", "app"], scratch.path()).status.success(),
        "creating the app failed"
    );

    let app = scratch.path().join("app");
    let manifest = std::fs::read_to_string(app.join("flux.toml")).expect("read manifest");
    std::fs::write(
        app.join("flux.toml"),
        format!("{manifest}\n[dependencies]\nshared = {{ path = \"../shared\" }}\n"),
    )
    .expect("write manifest");
    std::fs::write(
        app.join("src").join("main.flx"),
        "import Shared as Shared\n\nfn main() with IO { print(Shared.greet()) }\n",
    )
    .expect("write main");

    let first = flux(&["run"], &app);
    assert!(
        first.status.success(),
        "first run failed:\n{}",
        combined(&first)
    );

    // Rename the dependency's namespace; the cached roots must not survive it.
    std::fs::write(
        scratch.path().join("shared").join("flux.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nnamespace = \"Renamed\"\n",
    )
    .expect("rewrite dependency manifest");

    let second = flux(&["run"], &app);
    assert!(
        !second.status.success(),
        "the stale namespace must no longer resolve:\n{}",
        combined(&second)
    );
}
