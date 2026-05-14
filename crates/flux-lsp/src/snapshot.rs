use std::sync::Arc;

use flux::ast::type_infer::{InferProgramResult, infer_program};
use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::lsp_support;
use flux::syntax::interner::Interner;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

use crate::hover_index::HoverIndex;
use crate::line_index::{PositionEncoding, PositionMap};
use crate::prelude::Prelude;
use crate::symbol_index::SymbolIndex;

pub struct Snapshot {
    pub text: Arc<str>,
    pub program: Program,
    pub interner: Interner,
    pub infer: Option<InferProgramResult>,
    pub hover_index: HoverIndex,
    pub symbol_index: SymbolIndex,
    pub position_map: PositionMap,
    pub diagnostics: Vec<FluxDiagnostic>,
}

impl Snapshot {
    /// Build a snapshot from source text. Inference runs through the shared
    /// `Compiler` held by `prelude`, which already has Flow prelude schemes
    /// loaded into its `cached_member_schemes`. That's what lets `print`,
    /// `Console`, etc. resolve to their real types in this buffer.
    pub fn build(text: Arc<str>, prelude: &mut Prelude, encoding: PositionEncoding) -> Self {
        // Swap the compiler's interner into the buffer's lexer so identifiers
        // in the buffer share IDs with the preloaded schemes. Swap the
        // enriched interner back when parsing finishes.
        let main_interner = std::mem::take(&mut prelude.compiler.interner);
        let lexer = Lexer::new_with_interner(text.as_ref().to_string(), main_interner);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let mut diagnostics = std::mem::take(&mut parser.errors);
        diagnostics.extend(parser.take_warnings());
        prelude.compiler.interner = parser.take_interner();

        let hover_index = HoverIndex::build(&program);
        let symbol_index = SymbolIndex::build(&program, &prelude.compiler.interner);
        let position_map = PositionMap::new(Arc::clone(&text), encoding);

        // Walk buffer-level `import Flow.*` statements and lazily preload any
        // Flow module not in the auto-prelude (e.g. `Flow.Async`,
        // `Flow.Tcp`). Without this, identifiers from those modules collapse
        // to free type variables during inference.
        load_buffer_imports(&program, prelude);

        let infer = run_inference(&program, &mut prelude.compiler);
        if let Some(result) = &infer {
            diagnostics.extend(result.diagnostics.iter().cloned());
        }

        // The snapshot keeps a clone of the (now enriched) interner so it can
        // resolve symbols independently of subsequent buffer edits.
        let interner = prelude.compiler.interner.clone();

        Snapshot {
            text,
            program,
            interner,
            infer,
            hover_index,
            symbol_index,
            position_map,
            diagnostics,
        }
    }
}

fn run_inference(
    program: &Program,
    compiler: &mut flux::compiler::Compiler,
) -> Option<InferProgramResult> {
    // Clear per-file scratch from the previous buffer (errors, scope state,
    // function effects) without dropping the prelude's `cached_member_schemes`.
    lsp_support::reset_per_file_state(compiler);
    compiler.set_file_path("<buffer>".to_string());

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let config = lsp_support::build_infer_config_for_program(compiler, program);
        infer_program(program, &compiler.interner, config)
    }))
    .ok()
}

fn load_buffer_imports(program: &Program, prelude: &mut Prelude) {
    let module_names: Vec<String> = program
        .statements
        .iter()
        .filter_map(|stmt| match stmt {
            Statement::Import { name, .. } => prelude
                .compiler
                .interner
                .try_resolve(*name)
                .filter(|s| s.starts_with("Flow."))
                .map(|s| s.to_string()),
            _ => None,
        })
        .collect();

    for name in module_names {
        prelude.preload_module_if_needed(&name);
    }
}
