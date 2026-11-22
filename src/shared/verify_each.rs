//! IR checks after every pass (proposal 0190 §14), as rustc's
//! `-Zvalidate-mir` and Swift's `-sil-verify-all` do.
//!
//! With the hidden `--verify-each` (or `FLUX_VERIFY_EACH=1`), each IR's
//! checker runs after every pass rather than at a few points: Core lint after
//! each Core pass, Aether's contract after each Aether pass, and
//! `lir::verify` after lowering and after each LIR pass. A failure names the
//! pass that ran last, so a broken pass is found where it breaks the IR.
//! It is off by default, since it multiplies the cost of checking.

use std::sync::OnceLock;

static ENABLED: OnceLock<bool> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// On for this test's thread only, since unit tests run in parallel.
    static TEST_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Sets the switch from `--verify-each`, falling back to `FLUX_VERIFY_EACH`.
pub fn configure(cli: bool) {
    ENABLED.get_or_init(|| cli || env_enabled());
}

fn env_enabled() -> bool {
    std::env::var("FLUX_VERIFY_EACH").is_ok_and(|value| value.trim() == "1")
}

/// Whether each pass is followed by a check.
pub fn is_active() -> bool {
    #[cfg(test)]
    if TEST_ENABLED.with(std::cell::Cell::get) {
        return true;
    }
    *ENABLED.get_or_init(env_enabled)
}

/// Runs `f` with the checks on, on this thread only.
#[cfg(test)]
pub(crate) fn with_enabled<R>(f: impl FnOnce() -> R) -> R {
    TEST_ENABLED.with(|cell| cell.set(true));
    let result = f();
    TEST_ENABLED.with(|cell| cell.set(false));
    result
}
