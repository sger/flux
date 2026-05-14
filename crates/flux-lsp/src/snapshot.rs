use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use flux::ast::type_infer::{InferProgramConfig, InferProgramResult, infer_program};
use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::syntax::interner::Interner;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::program::Program;

use crate::span_index::SpanIndex;
use crate::symbol_index::SymbolIndex;

pub struct Snapshot {
    pub text: Arc<str>,
    pub program: Program,
    pub interner: Interner,
    pub infer: Option<InferProgramResult>,
    pub span_index: SpanIndex,
    pub symbol_index: SymbolIndex,
    pub diagnostics: Vec<FluxDiagnostic>,
}

impl Snapshot {
    /// Build a snapshot from source text: lex + parse + infer.
    ///
    /// Inference uses a minimal stand-alone config (no preloaded prelude),
    /// which means single-file diagnostics. Cross-module resolution is a
    /// follow-up; the inference engine is resilient to missing schemes and
    /// returns partial `expr_types` regardless.
    pub fn build(text: Arc<str>) -> Self {
        let lexer = Lexer::new(text.as_ref().to_string());
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let mut diagnostics = std::mem::take(&mut parser.errors);
        diagnostics.extend(parser.take_warnings());
        let mut interner = parser.take_interner();

        let span_index = SpanIndex::build(&program);
        let symbol_index = SymbolIndex::build(&program, &interner);

        let infer = run_inference(&program, &mut interner);
        if let Some(result) = &infer {
            diagnostics.extend(result.diagnostics.iter().cloned());
        }

        Snapshot {
            text,
            program,
            interner,
            infer,
            span_index,
            symbol_index,
            diagnostics,
        }
    }
}

fn run_inference(program: &Program, interner: &mut Interner) -> Option<InferProgramResult> {
    let flow_module_symbol = interner.intern("Flow");
    let config = InferProgramConfig {
        file_path: None,
        preloaded_base_schemes: HashMap::new(),
        preloaded_module_member_schemes: HashMap::new(),
        task_module_bindings: HashSet::new(),
        task_spawn_exposed: false,
        known_flow_names: HashSet::new(),
        flow_module_symbol,
        preloaded_effect_op_signatures: HashMap::new(),
        effect_row_aliases: HashMap::new(),
        class_env: None,
    };
    // Inference walks unfamiliar program shapes during typing. If it panics on
    // a half-typed buffer we want to surface the rest of the snapshot rather
    // than tear down the server.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        infer_program(program, interner, config)
    }))
    .ok()
}
