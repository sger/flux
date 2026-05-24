//! `flux eval "<expr>"` — evaluate a single expression on the VM and print its value.
//!
//! The expression is wrapped in a synthetic `fn main` and run through the normal
//! program pipeline (VM backend). This powers the LSP's "evaluate `>>>` snippets in
//! doc comments" code lens: the editor spawns this as a subprocess, so arbitrary
//! user code runs isolated from the language server — its stdout, and any infinite
//! loop, can never corrupt or hang the LSP's stdio channel.

use std::path::PathBuf;

use crate::driver::{
    flags::DriverFlags,
    pipeline::program::{RunProgramRequest, run_from_source},
    session::DriverSession,
};

/// Evaluate `expr` and print its value to stdout. Diagnostics (parse / type /
/// runtime errors) go to stderr and exit non-zero, mirroring `flux run`, so the
/// caller can show them as the result.
pub(crate) fn run_eval(expr: &str, mut flags: DriverFlags) {
    // The synthetic entry path never exists on disk, so caching against it is
    // pointless (it would key a cache entry on a phantom file).
    flags.cache.no_cache = true;

    let source = wrap_expression(expr);
    let path = synthetic_eval_path();
    let path = path.to_string_lossy().into_owned();
    let session = DriverSession::from(&flags);
    run_from_source(
        RunProgramRequest {
            path: &path,
            flags: &flags,
            session: &session,
        },
        source,
    );
}

/// Wrap a bare expression into a runnable program. Flux rejects top-level effects
/// (E413) and requires a `main` (E414), so the expression is evaluated inside
/// `fn main() with IO`. `println` is polymorphic (`forall a. a -> Unit`) and
/// renders through the universal value formatter, so any value — Int, list, tuple,
/// ADT — prints without needing a `Show` instance. Printing the expression
/// directly (rather than `to_string(expr)`) keeps the REPL-style representation:
/// numbers unquoted, strings quoted, exactly as the formatter shows them.
fn wrap_expression(expr: &str) -> String {
    format!("fn main() with IO {{\n    println({})\n}}\n", expr.trim())
}

/// A synthetic entry path under the current directory. It never needs to exist on
/// disk; the run pipeline uses it only for module-root discovery (so cwd-relative
/// `lib/Flow` prelude and `src/` modules resolve) and diagnostic file tagging.
fn synthetic_eval_path() -> PathBuf {
    let mut path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    path.push("__flux_eval__.flx");
    path
}

#[cfg(test)]
mod tests {
    use super::wrap_expression;

    #[test]
    fn wraps_expression_in_main_with_io() {
        let wrapped = wrap_expression("2 + 2");
        assert_eq!(wrapped, "fn main() with IO {\n    println(2 + 2)\n}\n");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            wrap_expression("  len([1, 2, 3])  "),
            "fn main() with IO {\n    println(len([1, 2, 3]))\n}\n"
        );
    }
}
