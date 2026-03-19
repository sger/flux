use crate::diagnostics::Diagnostic;
use crate::syntax::program::Program;

use super::super::{Compiler, pipeline::CollectionResult};

impl Compiler {
    /// Phase 6: Emit main entry call, suppress overlapping diagnostics, finalize.
    pub(in crate::bytecode::compiler) fn phase_finalization(
        &mut self,
        program: &Program,
        collection: &CollectionResult,
        mut hm_diagnostics: Vec<Diagnostic>,
    ) -> Result<(), Vec<Diagnostic>> {
        let main_symbol = self.interner.intern("main");

        if collection.main_state.has_main
            && !self.has_explicit_top_level_main_call(program, main_symbol)
        {
            self.emit_main_entry_call();
        }

        // HM no longer emits errors for annotated boundaries (return type,
        // let annotation) — those use `unify_propagate` (silent).
        //
        // For call-site argument mismatches, HM's `infer_call` still reports
        // (it's the only reporter for untyped functions).  When a typed function
        // causes the compiler to emit a per-argument boundary error, the HM
        // call-site error is redundant.  Drop any HM diagnostic that shares the
        // same code + message as an existing compiler error on an overlapping span.
        self.suppress_overlapping_hm_diagnostics(&mut hm_diagnostics);

        // HM diagnostics appended after bytecode errors so that specific,
        // actionable errors (e.g. E077 legacy list tail, E055 contract mismatch)
        // surface first in the error list.
        self.errors.extend(hm_diagnostics);

        // Return all errors at the end
        if !self.errors.is_empty() {
            return Err(std::mem::take(&mut self.errors));
        }

        Ok(())
    }
}
