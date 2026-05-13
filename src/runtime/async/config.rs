//! Environment-driven runtime config flags for the async fiber scheduler.
//! Each flag is read once on first use and cached in a `OnceLock`, mirroring
//! the `work_stealing_enabled` shape in [`super::scheduler`].

use std::sync::OnceLock;

/// Whether a parked/yielded VM fiber may migrate to another OS worker (i.e. be
/// stolen and resumed off its home worker).
///
/// **On by default** — the VM worker path now deep-copies shared VM state and
/// uses the Arc mirror for stolen parked/yielded fibers. `FLUX_FIBER_MIGRATION=0`
/// (or `false` / `off`) disables migration as a diagnostic escape hatch.
///
/// When this is on, a stolen fiber that carries a continuation is promoted to
/// an [`crate::runtime::value::ArcValue`]-backed `ArcFiber` as it leaves the
/// victim's queue and demoted back into the thief's `Rc` world before running
/// (see [`super::fiber::Fiber::promote`] / [`super::fiber::ArcFiber::demote`]).
pub fn fiber_migration_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("FLUX_FIBER_MIGRATION").ok().as_deref(),
            Some("0") | Some("false") | Some("FALSE") | Some("off") | Some("OFF")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fiber_migration_default_is_on() {
        // The test process doesn't set FLUX_FIBER_MIGRATION.
        assert!(fiber_migration_enabled());
    }
}
