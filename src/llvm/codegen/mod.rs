mod adt;
mod arith;
pub(crate) mod builtins;
mod closure;
mod prelude;

pub use adt::{
    AdtMetadata, FLUX_ADT_TYPE_NAME, FLUX_TUPLE_TYPE_NAME, emit_adt_support, flux_adt_symbol,
};
pub use arith::{emit_and, emit_arith, emit_not, emit_or, flux_arith_symbol};
pub use closure::{
    FLUX_CLOSURE_TYPE_NAME, closure_type, emit_closure_support, flux_closure_symbol,
};
pub use prelude::{
    FluxNanboxLayout, FluxPtrTagLayout, emit_prelude, emit_prelude_and_arith, flux_prelude_symbol,
};

use crate::syntax::Identifier;
use crate::syntax::interner::Interner;

// ── Types shared with adt.rs (formerly in function.rs) ───────────────────────

/// Errors that can occur during LLVM IR lowering.
#[derive(Debug, Clone)]
pub enum CoreToLlvmError {
    Unsupported {
        feature: &'static str,
        context: String,
    },
    Malformed {
        message: String,
    },
    MissingSymbol {
        message: String,
    },
}

impl std::fmt::Display for CoreToLlvmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreToLlvmError::Unsupported { feature, context } => {
                write!(f, "unsupported CoreToLlvm feature `{feature}`: {context}")
            }
            CoreToLlvmError::Malformed { message } => {
                write!(f, "malformed Core lowering: {message}")
            }
            CoreToLlvmError::MissingSymbol { message } => {
                write!(f, "missing CoreToLlvm symbol: {message}")
            }
        }
    }
}

impl std::error::Error for CoreToLlvmError {}

/// Resolve an `Identifier` (interned symbol) to a display string.
pub(super) fn display_ident(ident: Identifier, interner: Option<&Interner>) -> String {
    interner
        .map(|it| it.resolve(ident).to_string())
        .unwrap_or_else(|| ident.to_string())
}
