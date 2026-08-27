//! A qualified call `Module.f(..)` must reach the module's own function, and
//! dispatch as a class method only when the qualifier names the class.
//!
//! The two cases pull in opposite directions:
//!
//!   * `Foldable.fold(..)` / `Comparable.same(..)` are class-method calls
//!     written through the module that declares the class. They must keep
//!     dispatching to the selected instance, effects and all.
//!   * `Stream.append(..)` is an ordinary module function whose name collides
//!     with the built-in `Semigroup` method. It must reach `Flow.Stream`.
//!
//! Before the fix, any qualified call whose member name matched a class
//! method was routed to the class — so `Stream.append` was unreachable,
//! failing with `E444: No instance for Semigroup<Stream<Int>>`.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(name: &str, source: &str) -> (String, String, bool) {
    // Own scratch dir per run: a literal filename in one shared directory let
    // concurrent test binaries overwrite each other (KI-010).
    let scratch = Scratch::new(name.trim_end_matches(".flx"));
    let file = scratch.write(name, source);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .args(scratch.cache_args())
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {name}: {e}"));
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

/// The regression: a module function whose name matches a built-in class
/// method must still be callable.
#[test]
fn a_module_function_shadowing_a_builtin_class_method_is_reachable() {
    let (stdout, stderr, success) = run_source(
        "qualified_stream_append.flx",
        r#"
import Flow.Async exposing (..)
import Flow.Stream as Stream

fn body() -> Int with Async {
    let joined = Stream.append(Stream.from_array([|1, 2|]), Stream.from_array([|3|]))
    Stream.count(joined)
}

fn main() -> Unit with Console {
    print(to_string(run_async(body)))
}
"#,
    );
    assert!(
        success,
        "Stream.append must reach Flow.Stream, not Semigroup:\n{stdout}\n{stderr}"
    );
    assert!(
        !format!("{stdout}{stderr}").contains("Semigroup"),
        "the call must not be routed to the Semigroup class:\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains('3'), "expected 3 elements, got:\n{stdout}");
}

/// The other direction: genuine qualified class-method calls must keep
/// dispatching. These examples call `Comparable.same` and `Matchable.same`
/// through the module that declares the class, which is exactly the shape the
/// fix had to preserve — an over-broad rule here breaks instance selection.
#[test]
fn qualified_class_method_calls_still_dispatch_to_their_instances() {
    for example in [
        "examples/type_classes/eq_auditlog_example.flx",
        "examples/type_classes/class_matchable_effects.flx",
    ] {
        let path = workspace_root().join(example);
        if !path.exists() {
            continue;
        }
        let scratch = Scratch::new("qualified-class-method-example");
        let output = Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .args(["run", path.to_str().unwrap(), "--no-cache"])
            .args(scratch.cache_args())
            .output()
            .unwrap_or_else(|e| panic!("failed to run {example}: {e}"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{example} must still run:\n{stdout}\n{stderr}"
        );
        assert!(
            !format!("{stdout}{stderr}").contains("No instance"),
            "{example} lost its instance dispatch:\n{stdout}\n{stderr}"
        );
    }
}
