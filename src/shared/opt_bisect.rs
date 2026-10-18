//! Numbering optimizing rewrites, to find the one that breaks a program
//! (proposal 0190 §13), as LLVM's `-opt-bisect-limit` does.
//!
//! With `--opt-bisect-limit <n>` (or `FLUX_OPT_BISECT`), each optimizing
//! rewrite asks [`allow`] before it fires. Rewrites are numbered from 1 in the
//! order they are about to fire, and those numbered above `n` are skipped, so
//! `n = 0` runs none of them. With `-1` every rewrite runs and each prints one
//! `opt-bisect:` line to stderr. `cargo xtask bisect-opt` binary-searches `n`.
//!
//! Only rewrites a program is correct without are counted: Core
//! simplification, the AST folds of `--optimize`, and Aether's drop
//! specialization, fusion and reuse. ANF, dictionary and evidence passes and
//! dup/drop insertion are not.
//!
//! The numbering has to be the same on every run, so with a limit set the
//! process compiles on one thread ([`configure`]), and the counter is global.
//! Without one, [`allow`] is one read and always says yes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::syntax::interner::Interner;

/// `None`: no bisection. `Some(-1)`: run and print every rewrite.
static LIMIT: OnceLock<Option<i64>> = OnceLock::new();
static COUNT: AtomicU64 = AtomicU64::new(0);
/// The module and definition being optimized, for the printed lines.
static CONTEXT: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

#[cfg(test)]
thread_local! {
    /// A limit for this test's thread only: unit tests run in parallel, and a
    /// process-wide limit would skip rewrites in every other test.
    static TEST_LIMIT: std::cell::Cell<Option<(i64, u64)>> = const { std::cell::Cell::new(None) };
}

/// Sets the limit from `--opt-bisect-limit`, falling back to
/// `FLUX_OPT_BISECT`. With a limit, rayon's global pool gets one thread, so
/// modules compile, and rewrites are numbered, in one order.
pub fn configure(cli_limit: Option<i64>) {
    let limit = *LIMIT.get_or_init(|| cli_limit.or_else(env_limit));
    if limit.is_some() {
        // Fails only if the pool already exists, which nothing does before
        // the driver starts compiling.
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build_global();
    }
}

fn env_limit() -> Option<i64> {
    std::env::var("FLUX_OPT_BISECT").ok()?.trim().parse().ok()
}

fn limit() -> Option<i64> {
    *LIMIT.get_or_init(env_limit)
}

/// Whether a limit is set, so callers can skip work only bisection needs.
pub fn is_active() -> bool {
    #[cfg(test)]
    if TEST_LIMIT.with(std::cell::Cell::get).is_some() {
        return true;
    }
    limit().is_some()
}

/// Whether the next rewrite may fire. `pass` names the pass and `what`
/// describes the rewrite; `what` is only called when it is printed.
pub fn allow(pass: &str, what: impl FnOnce() -> String) -> bool {
    #[cfg(test)]
    if let Some((limit, count)) = TEST_LIMIT.with(std::cell::Cell::get) {
        let n = count + 1;
        TEST_LIMIT.with(|cell| cell.set(Some((limit, n))));
        return limit < 0 || n <= limit.unsigned_abs();
    }
    let Some(limit) = limit() else {
        return true;
    };
    let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if limit < 0 {
        let context = CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
        // An AST rewrite has no definition, only a module.
        if context.1.is_empty() {
            eprintln!("opt-bisect: {n} {pass}: {} ({})", what(), context.0);
        } else {
            eprintln!(
                "opt-bisect: {n} {pass}: {} in {} ({})",
                what(),
                context.1,
                context.0
            );
        }
        return true;
    }
    n <= limit.unsigned_abs()
}

/// Records the module being compiled, for the printed lines.
pub fn set_module(module: &str) {
    if is_active() {
        CONTEXT.lock().unwrap_or_else(|e| e.into_inner()).0 = module.to_string();
    }
}

/// Records the definition being optimized, for the printed lines.
pub fn set_def(def: impl FnOnce() -> String) {
    if is_active() {
        CONTEXT.lock().unwrap_or_else(|e| e.into_inner()).1 = def();
    }
}

/// A definition's name for `opt-bisect:` lines and `FLUX_OPT_BISECT_SHOW`.
pub fn def_name(name: crate::syntax::Identifier, interner: Option<&Interner>) -> String {
    interner
        .and_then(|interner| interner.try_resolve(name))
        .map_or_else(|| format!("{name:?}"), str::to_string)
}

/// Whether `FLUX_OPT_BISECT_SHOW` names `def`, whose code after the
/// optimizing passes should then be printed: `bisect-opt` diffs it between
/// two limits.
pub fn shows(def: &str) -> bool {
    is_active() && std::env::var("FLUX_OPT_BISECT_SHOW").is_ok_and(|shown| shown == def)
}

/// Runs `run` on this thread with `limit`, as `--opt-bisect-limit` would.
#[cfg(test)]
pub fn with_test_limit<R>(limit: i64, run: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_LIMIT.with(|cell| cell.set(None));
        }
    }
    TEST_LIMIT.with(|cell| cell.set(Some((limit, 0))));
    let _reset = Reset;
    run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_limit_every_rewrite_fires() {
        assert!(!is_active() || limit().is_some());
        if limit().is_none() {
            assert!((0..100).all(|_| allow("test", String::new)));
        }
    }

    #[test]
    fn rewrites_past_the_limit_are_skipped() {
        let fired: Vec<bool> =
            with_test_limit(2, || (0..4).map(|_| allow("test", String::new)).collect());
        assert_eq!(fired, [true, true, false, false]);
    }

    #[test]
    fn a_limit_of_zero_skips_everything_and_minus_one_nothing() {
        assert!(with_test_limit(0, || !allow("test", String::new)));
        assert!(with_test_limit(-1, || (0..10).all(|_| allow("test", String::new))));
    }
}
