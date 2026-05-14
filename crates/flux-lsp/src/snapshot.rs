use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use flux::ast::type_infer::{InferProgramResult, infer_program};
use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::lsp_support;
use flux::syntax::interner::Interner;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

use crate::line_index::{PositionEncoding, PositionMap};
use crate::prelude::Prelude;
use crate::symbol_index::SymbolIndex;

pub struct Snapshot {
    pub text: Arc<str>,
    pub program: Program,
    pub interner: Interner,
    pub infer: Option<InferProgramResult>,
    pub symbol_index: SymbolIndex,
    pub position_map: PositionMap,
    pub diagnostics: Vec<FluxDiagnostic>,
    /// Final segment of every module known to this session's prelude (e.g.
    /// `"String"` for `Flow.String`). Used by hover to label module-prefixed
    /// references like `String.join`.
    pub module_short_names: HashSet<String>,
    /// Short module name → member names (e.g. `"String" -> ["join", "split", ...]`).
    /// Populated from the prelude's `cached_member_schemes` so completion
    /// after `String.` can list available members without rebuilding the map
    /// on every keystroke.
    pub module_members: HashMap<String, Vec<String>>,
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

        // Collect short names (final dotted segment) of every loaded module so
        // hover can recognize `String.join` as referencing `Flow.String`.
        let module_short_names: HashSet<String> = prelude
            .loaded_modules
            .iter()
            .map(|qual| {
                qual.rsplit('.')
                    .next()
                    .unwrap_or(qual.as_str())
                    .to_string()
            })
            .collect();

        // Index module members by short name for completion. Walks the
        // compiler's `cached_member_schemes`; one entry per `(module, member)`.
        let mut module_members: HashMap<String, Vec<String>> = HashMap::new();
        for (module_sym, member_sym) in prelude.compiler.cached_member_schemes().keys() {
            let Some(qualified) = prelude.compiler.interner.try_resolve(*module_sym) else {
                continue;
            };
            let Some(member) = prelude.compiler.interner.try_resolve(*member_sym) else {
                continue;
            };
            if !prelude.loaded_modules.contains(qualified) {
                // Only surface members from prelude modules we actually
                // recognize — avoids leaking arbitrary compiler-internal
                // keys (e.g. effect-op intrinsics) into completion.
                continue;
            }
            let short = qualified.rsplit('.').next().unwrap_or(qualified);
            module_members
                .entry(short.to_string())
                .or_default()
                .push(member.to_string());
        }
        for v in module_members.values_mut() {
            v.sort();
            v.dedup();
        }

        Snapshot {
            text,
            program,
            interner,
            infer,
            symbol_index,
            position_map,
            diagnostics,
            module_short_names,
            module_members,
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
