//! Environment-driven runtime config flags for the async fiber scheduler.
//! Each flag is read once on first use and cached in a `OnceLock`, mirroring
//! the `work_stealing_enabled` shape in [`super::scheduler`].

use std::sync::OnceLock;

/// Whether a parked/yielded VM fiber may migrate to another OS worker (i.e. be
/// stolen and resumed off its home worker).
///
/// **Off by default** — until the cross-worker steal path is wired up and the
/// migration test matrix is green, the scheduler keeps the no-migration
/// invariant. `FLUX_FIBER_MIGRATION=1` (or `true` / `on`) opts in; anything
/// else (including unset) is off.
///
/// When this is on, a stolen fiber that carries a continuation is promoted to
/// an [`crate::runtime::value::ArcValue`]-backed `ArcFiber` as it leaves the
/// victim's queue and demoted back into the thief's `Rc` world before running
/// (see [`super::fiber::Fiber::promote`] / [`super::fiber::ArcFiber::demote`]).
pub fn fiber_migration_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("FLUX_FIBER_MIGRATION").ok().as_deref(),
            Some("1") | Some("true") | Some("TRUE") | Some("on") | Some("ON")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fiber_migration_default_is_off() {
        // The test process doesn't set FLUX_FIBER_MIGRATION.
        assert!(!fiber_migration_enabled());
    }
}
