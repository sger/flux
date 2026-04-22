//! Builtin effect label registry (Proposal 0161, Phases 0/B1/B2).
//!
//! Centralizes the well-known effect label names so call sites that
//! previously wrote `interner.intern("IO")` or `interner.intern("Time")`
//! share one source of truth. Also exposes the fine-grained decomposed
//! labels (`Console`, `FileSystem`, `Stdin`, `Clock`, `Panic`) that back
//! the `IO`/`Time` aliases in later phases.
//!
//! `Interner::intern` is idempotent — the helpers here do not cache a
//! `Symbol`; they delegate straight to the interner so a single registry
//! instance can be shared across the compiler without extra state.

use crate::syntax::{Identifier, interner::Interner};

// ── Monolithic labels (kept for today's compiler; become aliases in B3) ──

/// Label name for the monolithic `IO` effect. Aliased to
/// `<Console | FileSystem | Stdin>` once B3 lands.
pub const IO: &str = "IO";

/// Label name for the `Time` effect. Aliased to `<Clock>` once B3 lands.
pub const TIME: &str = "Time";

// ── Fine-grained decomposed labels (Proposal 0161 Phase 1) ──

/// `print` / `println` — stdout.
pub const CONSOLE: &str = "Console";

/// `read_file` / `write_file` / `read_lines` — filesystem I/O.
pub const FILESYSTEM: &str = "FileSystem";

/// `read_stdin` — stdin I/O.
pub const STDIN: &str = "Stdin";

/// `clock_now` / `now_ms` — wall-clock / monotonic time.
pub const CLOCK: &str = "Clock";

/// `panic` — intentional crash. Kept separate from `Exn` because it
/// cannot be discarded by the optimizer.
pub const PANIC: &str = "Panic";

/// `idiv` / `imod` / indexing — recoverable failure (div-by-zero, OOB).
pub const DIV: &str = "Div";

/// Intern the `IO` effect label.
pub fn io_effect_symbol(interner: &mut Interner) -> Identifier {
    interner.intern(IO)
}

/// Intern the `Time` effect label.
pub fn time_effect_symbol(interner: &mut Interner) -> Identifier {
    interner.intern(TIME)
}

/// Look up the `IO` effect label without mutating the interner. Returns
/// `None` if the program has not yet interned it.
pub fn io_effect_symbol_opt(interner: &Interner) -> Option<Identifier> {
    interner.lookup(IO)
}

/// Look up the `Time` effect label without mutating the interner.
pub fn time_effect_symbol_opt(interner: &Interner) -> Option<Identifier> {
    interner.lookup(TIME)
}

// ── Primop → effect label registry ──

use crate::core::CorePrimOp;

/// Fine-grained effect label a primop carries, as a stable string.
///
/// Returning the `&'static str` (rather than an `Identifier`) keeps the
/// registry pure — callers that need a `Symbol` hit the interner
/// themselves. `None` means the primop is pure (no effect row
/// contribution).
pub fn primop_fine_effect_label(op: CorePrimOp) -> Option<&'static str> {
    use CorePrimOp::*;
    match op {
        Println | Print => Some(CONSOLE),
        ReadFile | WriteFile | ReadLines => Some(FILESYSTEM),
        ReadStdin => Some(STDIN),
        ClockNow | Time => Some(CLOCK),
        Panic => Some(PANIC),
        _ => None,
    }
}

/// Coarse label (`IO` / `Time` / `None`) a primop maps to under the
/// current non-decomposed effect system. This matches the existing
/// `effect_kind` behavior and is what compiler passes ask for today.
///
/// Once B3 decomposes `IO`, callers should migrate to
/// [`primop_fine_effect_label`] and let the alias machinery handle the
/// expansion.
pub fn primop_coarse_effect_label(op: CorePrimOp) -> Option<&'static str> {
    match primop_fine_effect_label(op)? {
        CONSOLE | FILESYSTEM | STDIN => Some(IO),
        CLOCK => Some(TIME),
        // Panic and Div stay as themselves — no coarse alias.
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_primops_have_console_fine_and_io_coarse() {
        for op in [CorePrimOp::Print, CorePrimOp::Println] {
            assert_eq!(primop_fine_effect_label(op), Some(CONSOLE));
            assert_eq!(primop_coarse_effect_label(op), Some(IO));
        }
    }

    #[test]
    fn filesystem_primops_have_filesystem_fine_and_io_coarse() {
        for op in [
            CorePrimOp::ReadFile,
            CorePrimOp::WriteFile,
            CorePrimOp::ReadLines,
        ] {
            assert_eq!(primop_fine_effect_label(op), Some(FILESYSTEM));
            assert_eq!(primop_coarse_effect_label(op), Some(IO));
        }
    }

    #[test]
    fn stdin_primop_has_stdin_fine_and_io_coarse() {
        assert_eq!(primop_fine_effect_label(CorePrimOp::ReadStdin), Some(STDIN));
        assert_eq!(primop_coarse_effect_label(CorePrimOp::ReadStdin), Some(IO));
    }

    #[test]
    fn clock_primops_have_clock_fine_and_time_coarse() {
        for op in [CorePrimOp::ClockNow, CorePrimOp::Time] {
            assert_eq!(primop_fine_effect_label(op), Some(CLOCK));
            assert_eq!(primop_coarse_effect_label(op), Some(TIME));
        }
    }

    #[test]
    fn panic_primop_has_panic_label_at_both_granularities() {
        assert_eq!(primop_fine_effect_label(CorePrimOp::Panic), Some(PANIC));
        assert_eq!(primop_coarse_effect_label(CorePrimOp::Panic), Some(PANIC));
    }

    #[test]
    fn pure_primops_have_no_effect_label() {
        for op in [CorePrimOp::Add, CorePrimOp::Mul, CorePrimOp::IAdd] {
            assert_eq!(primop_fine_effect_label(op), None);
            assert_eq!(primop_coarse_effect_label(op), None);
        }
    }

    #[test]
    fn coarse_labels_align_with_existing_effect_kind() {
        // Ensures the new registry produces the same IO/Time/None classification
        // as the legacy `CorePrimOp::effect_kind()` match, keeping behavior
        // equivalent while the registry scaffolding stabilizes.
        use crate::core::PrimEffect;
        for op in [
            CorePrimOp::Print,
            CorePrimOp::Println,
            CorePrimOp::ReadFile,
            CorePrimOp::WriteFile,
            CorePrimOp::ReadLines,
            CorePrimOp::ReadStdin,
            CorePrimOp::ClockNow,
            CorePrimOp::Time,
            CorePrimOp::Panic,
            CorePrimOp::Add,
        ] {
            let coarse = primop_coarse_effect_label(op);
            let expected = match op.effect_kind() {
                PrimEffect::Io => Some(IO),
                PrimEffect::Time => Some(TIME),
                PrimEffect::Control => Some(PANIC),
                PrimEffect::Pure => None,
            };
            assert_eq!(
                coarse, expected,
                "coarse label mismatch for {:?}: {:?} vs {:?}",
                op, coarse, expected
            );
        }
    }
}
