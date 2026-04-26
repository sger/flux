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

/// `debug` / `debug_labeled` / `debug_with` — developer tracing to stderr.
/// Separate from `Console` so debug output does not compete with the
/// stdout channel and can be handled independently (captured in tests,
/// redirected, or silenced). See proposal 0171 and `lib/Flow/Debug.flx`.
pub const DEBUG: &str = "Debug";

/// Reserved non-determinism label. Documented in `Flow.Effects`, not
/// operationally emitted by compiler primops in this slice.
pub const NONDET: &str = "NonDet";

/// Reserved randomness label. Documented in `Flow.Effects`, not
/// operationally emitted by compiler primops in this slice.
pub const RANDOM: &str = "Random";

/// Reserved recoverable-exception label. Documented in `Flow.Effects`, not
/// operationally emitted by compiler primops in this slice.
pub const EXN: &str = "Exn";

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

/// Whether a function effect annotation name is one of the builtin 0161
/// aliases or fine-grained labels currently recognized by the compiler.
///
/// This intentionally covers only the labels/aliases that have operational
/// compiler support in the current slice. Broader documented labels like
/// `Random` / `NonDet` / `Exn` remain future-facing until their support lands.
pub fn is_known_function_effect_annotation_name(name: &str) -> bool {
    matches!(
        name,
        IO | TIME | CONSOLE | FILESYSTEM | STDIN | CLOCK | PANIC | DIV | DEBUG
    )
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
        DebugTrace => Some(DEBUG),
        ReadFile | WriteFile | ReadLines => Some(FILESYSTEM),
        ReadStdin => Some(STDIN),
        ClockNow | Time => Some(CLOCK),
        Panic => Some(PANIC),
        Div | Mod | IDiv | IMod | FDiv | Index => Some(DIV),
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
        // Panic, Div, and Debug stay as themselves — no coarse alias.
        other => Some(other),
    }
}

/// Look up the current coarse effect label (`IO`, `Time`, `Panic`, ...)
/// carried by a builtin function name.
///
/// This is the name-based bridge used by Aether/FBIP today. It delegates to
/// the same primop registry as the optimizer so compiler consumers stay in
/// sync until a later phase replaces the bridge entirely.
pub fn builtin_effect_for_name(name: &str) -> Option<&'static str> {
    for arity in 0..=3 {
        if let Some(op) = CorePrimOp::from_name(name, arity)
            && let Some(label) = primop_coarse_effect_label(op)
        {
            return Some(label);
        }
    }
    None
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
    fn failure_primops_have_div_label_at_both_granularities() {
        for op in [
            CorePrimOp::Div,
            CorePrimOp::Mod,
            CorePrimOp::IDiv,
            CorePrimOp::IMod,
            CorePrimOp::FDiv,
            CorePrimOp::Index,
        ] {
            assert_eq!(primop_fine_effect_label(op), Some(DIV), "{:?}", op);
            assert_eq!(primop_coarse_effect_label(op), Some(DIV), "{:?}", op);
        }
    }

    #[test]
    fn pure_primops_have_no_effect_label() {
        for op in [CorePrimOp::Add, CorePrimOp::Mul, CorePrimOp::IAdd] {
            assert_eq!(primop_fine_effect_label(op), None);
            assert_eq!(primop_coarse_effect_label(op), None);
        }
    }

    #[test]
    fn coarse_label_classification_is_exhaustive() {
        // The registry is the single source of truth for primop → effect-label
        // mapping (Proposal 0161). This test pins the classification of every
        // primop that carries a label so that accidentally pruning one is
        // caught at test time.
        let io_primops = [
            CorePrimOp::Print,
            CorePrimOp::Println,
            CorePrimOp::ReadFile,
            CorePrimOp::WriteFile,
            CorePrimOp::ReadLines,
            CorePrimOp::ReadStdin,
        ];
        for op in io_primops {
            assert_eq!(primop_coarse_effect_label(op), Some(IO), "{:?}", op);
        }

        for op in [CorePrimOp::ClockNow, CorePrimOp::Time] {
            assert_eq!(primop_coarse_effect_label(op), Some(TIME), "{:?}", op);
        }

        assert_eq!(primop_coarse_effect_label(CorePrimOp::Panic), Some(PANIC));

        for op in [
            CorePrimOp::Div,
            CorePrimOp::Mod,
            CorePrimOp::IDiv,
            CorePrimOp::IMod,
            CorePrimOp::FDiv,
            CorePrimOp::Index,
        ] {
            assert_eq!(primop_coarse_effect_label(op), Some(DIV), "{:?}", op);
        }

        for op in [CorePrimOp::Add, CorePrimOp::IAdd, CorePrimOp::Mul] {
            assert_eq!(primop_coarse_effect_label(op), None, "{:?}", op);
        }
    }

    #[test]
    fn builtin_name_bridge_uses_the_same_registry() {
        assert_eq!(builtin_effect_for_name("print"), Some(IO));
        assert_eq!(builtin_effect_for_name("now_ms"), Some(TIME));
        assert_eq!(builtin_effect_for_name("panic"), Some(PANIC));
        assert_eq!(builtin_effect_for_name("idiv"), Some(DIV));
        assert_eq!(builtin_effect_for_name("array_get"), None);
        assert_eq!(builtin_effect_for_name("iadd"), None);
        assert_eq!(builtin_effect_for_name("definitely_unknown_builtin"), None);
    }

    #[test]
    fn known_function_effect_annotation_names_match_current_builtin_surface() {
        for name in [IO, TIME, CONSOLE, FILESYSTEM, STDIN, CLOCK, PANIC, DIV] {
            assert!(is_known_function_effect_annotation_name(name), "{name}");
        }
        for name in [RANDOM, NONDET, EXN, "State", "DefinitelyUnknown"] {
            assert!(
                !is_known_function_effect_annotation_name(name),
                "{name} should not be treated as a builtin 0161 annotation"
            );
        }
    }
}
