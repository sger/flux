//! Path dependencies build and run through the Flux manifest resolver.
//!
//! `Flume.Roots` reads `flux.toml`, walks path dependencies, and hands the
//! compiler one scoped module root per package. These tests exercise that
//! whole path — including the collision the phase exists to fix, where two
//! packages each shipping a `Json` module previously failed as a bare
//! `E027 Duplicate Module`.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dir");
    }
    std::fs::write(path, contents).expect("write file");
}

/// Run `entry` from `cwd`, returning combined output and success.
fn run(entry: &str, cwd: &Path) -> (String, bool) {
    let output = Command::new(flux_bin())
        .current_dir(cwd)
        .arg(entry)
        .arg("--no-cache")
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    let mut text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    text.push_str(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
    (text, output.status.success())
}

/// A package: a manifest, plus modules laid out under its namespace
/// (`src/<Namespace>/<Module>.flx`).
fn write_package(dir: &Path, name: &str, namespace: &str, deps: &str) {
    write(
        &dir.join("flux.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n{deps}"),
    );
    write(
        &dir.join("src").join(namespace).join("Util.flx"),
        &format!(
            "module {namespace}.Util {{\n    public fn tag() -> String {{ \"{name}\" }}\n}}\n"
        ),
    );
}

#[test]
fn a_path_dependency_resolves_through_its_namespace() {
    let scratch = Scratch::new("pkg-path-dep");
    let app = scratch.path().join("app");
    let shared = scratch.path().join("shared");

    write_package(&shared, "shared", "Shared", "");
    write_package(
        &app,
        "app",
        "App",
        "\n[dependencies]\nshared = { path = \"../shared\" }\n",
    );
    write(
        &app.join("src").join("main.flx"),
        "import Shared.Util as Util\n\nfn main() with IO { print(Util.tag()) }\n",
    );

    let (out, ok) = run("src/main.flx", &app);
    assert!(ok, "package build failed:\n{out}");
    assert!(out.contains("shared"), "unexpected output:\n{out}");
}

/// The regression this phase exists to fix: two packages each shipping a
/// module at the same relative path used to collide as `E027`.
#[test]
fn two_packages_claiming_one_namespace_report_a_collision() {
    let scratch = Scratch::new("pkg-collision");
    let app = scratch.path().join("app");

    // Both dependencies override their namespace to `Json`, so the import is
    // genuinely ambiguous and must name the packages.
    for name in ["a", "b"] {
        let dir = scratch.path().join(name);
        write(
            &dir.join("flux.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nnamespace = \"Json\"\n"),
        );
        write(
            &dir.join("src").join("Json").join("Parse.flx"),
            &format!("module Json.Parse {{\n    public fn tag() -> String {{ \"{name}\" }}\n}}\n"),
        );
    }
    write(
        &app.join("flux.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\na = { path = \"../a\" }\nb = { path = \"../b\" }\n",
    );
    write(
        &app.join("src").join("main.flx"),
        "import Json.Parse as P\n\nfn main() with IO { print(P.tag()) }\n",
    );

    let (out, ok) = run("src/main.flx", &app);
    assert!(!ok, "expected the collision to fail the build:\n{out}");
    assert!(out.contains("E469"), "expected E469:\n{out}");
    assert!(
        out.contains("`a`") && out.contains("`b`"),
        "the collision must name both packages:\n{out}"
    );
}

/// Registry dependencies parse but are rejected until Phase 2, rather than
/// being silently ignored and failing later as a missing module.
#[test]
fn a_registry_dependency_is_rejected_until_phase_two() {
    let scratch = Scratch::new("pkg-registry-dep");
    let app = scratch.path().join("app");
    write(
        &app.join("flux.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\njson = \"1.2\"\n",
    );
    write(
        &app.join("src").join("main.flx"),
        "fn main() with IO { print(1) }\n",
    );

    let (out, ok) = run("src/main.flx", &app);
    assert!(!ok, "expected a registry dependency to fail:\n{out}");
    assert!(out.contains("E470"), "expected E470:\n{out}");
    assert!(
        out.contains("registry dependency"),
        "the resolver's own message must survive:\n{out}"
    );
}

/// A manifest error must not bury itself under missing-stdlib cascades: the
/// unscoped roots stay in place so `Flow.*` still resolves.
#[test]
fn a_manifest_error_does_not_hide_behind_missing_stdlib_imports() {
    let scratch = Scratch::new("pkg-error-clarity");
    let app = scratch.path().join("app");
    write(
        &app.join("flux.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
         [dev-dependencies]\ntesting = { path = \"../testing\" }\n",
    );
    write(
        &app.join("src").join("main.flx"),
        "fn main() with IO { print(1) }\n",
    );

    let (out, _) = run("src/main.flx", &app);
    assert!(out.contains("E470"), "expected E470:\n{out}");
    assert!(
        !out.contains("Cannot find module `Flow."),
        "stdlib imports must still resolve:\n{out}"
    );
}

/// Script mode has no manifest and must keep working unchanged.
#[test]
fn script_mode_is_unaffected_by_manifest_resolution() {
    let scratch = Scratch::new("pkg-script-mode");
    write(
        &scratch.path().join("script.flx"),
        "fn main() with IO { print(6 * 7) }\n",
    );

    let (out, ok) = run("script.flx", scratch.path());
    assert!(ok, "script mode failed:\n{out}");
    assert!(out.contains("42"), "unexpected output:\n{out}");
}

/// A package may only declare modules under the namespace it owns, and the
/// error must land at that package's own build with the corrected path.
#[test]
fn a_module_outside_its_package_namespace_is_rejected() {
    let scratch = Scratch::new("pkg-namespace-escape");
    let pkg = scratch.path().join("json");
    write(
        &pkg.join("flux.toml"),
        "[package]\nname = \"json\"\nversion = \"0.1.0\"\n",
    );
    // Package `json` owns `Json`, so a bare `module Utils` escapes it.
    write(
        &pkg.join("src").join("Utils.flx"),
        "module Utils {\n    public fn helper() -> String { \"x\" }\n}\n",
    );
    write(
        &pkg.join("src").join("main.flx"),
        "import Utils as Utils\n\nfn main() with IO { print(Utils.helper()) }\n",
    );

    let (out, ok) = run("src/main.flx", &pkg);
    assert!(!ok, "a namespace escape must fail the build:\n{out}");
    assert!(out.contains("E471"), "expected E471:\n{out}");
    assert!(
        out.contains("`json`") && out.contains("`Json`"),
        "the error must name the package and its namespace:\n{out}"
    );
    assert!(
        out.contains("src/Json/Utils.flx"),
        "the hint must give the corrected path:\n{out}"
    );
}

/// The namespace root module is the package's public face, and a module
/// beneath the namespace is ordinary: neither may be flagged as an escape.
#[test]
fn modules_within_the_package_namespace_are_accepted() {
    let scratch = Scratch::new("pkg-namespace-ok");
    let pkg = scratch.path().join("json");
    write(
        &pkg.join("flux.toml"),
        "[package]\nname = \"json\"\nversion = \"0.1.0\"\n",
    );
    // `src/Json.flx` is the namespace root; `src/Json/Utils.flx` sits beneath it.
    write(
        &pkg.join("src").join("Json.flx"),
        "module Json {\n    public fn tag() -> String { \"root\" }\n}\n",
    );
    write(
        &pkg.join("src").join("Json").join("Utils.flx"),
        "module Json.Utils {\n    public fn helper() -> String { \"nested\" }\n}\n",
    );
    write(
        &pkg.join("src").join("main.flx"),
        "import Json as J\nimport Json.Utils as U\n\n         fn main() with IO { print(J.tag() + U.helper()) }\n",
    );

    let (out, ok) = run("src/main.flx", &pkg);
    assert!(ok, "a correctly namespaced package must build:\n{out}");
    assert!(out.contains("rootnested"), "unexpected output:\n{out}");
}

/// `--root` is the unscoped escape hatch: outside a package, any module name
/// is still legal.
#[test]
fn unscoped_roots_do_not_enforce_a_namespace() {
    let scratch = Scratch::new("pkg-namespace-unscoped");
    write(
        &scratch.path().join("lib").join("Utils.flx"),
        "module Utils {\n    public fn helper() -> String { \"unscoped\" }\n}\n",
    );
    write(
        &scratch.path().join("main.flx"),
        "import Utils as U\n\nfn main() with IO { print(U.helper()) }\n",
    );

    let output = Command::new(flux_bin())
        .current_dir(scratch.path())
        .args(["main.flx", "--root", "lib", "--no-cache"])
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    let text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    assert!(
        output.status.success(),
        "--root must not enforce a namespace:\n{text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("unscoped"), "unexpected output:\n{text}");
}
