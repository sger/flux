use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use crate as flux;
use crate::driver::frontend::{collect_roots, inject_flow_prelude, validate_no_primops_import};
use crate::driver::shared::tag_and_attach_file;
use flux::{
    diagnostics::Diagnostic,
    shared::cache_paths::{CacheLayout, resolve_cache_layout},
    syntax::{
        lexer::Lexer,
        module_graph::{GraphBuildResult, ModuleGraph},
        parser::Parser,
        program::Program,
    },
};

pub(crate) struct ProgramContext {
    pub(crate) source: String,
    pub(crate) program: Program,
    pub(crate) graph_result: GraphBuildResult,
    pub(crate) entry_has_errors: bool,
    pub(crate) parse_ms: f64,
    pub(crate) all_diagnostics: Vec<Diagnostic>,
    pub(crate) entry_path: PathBuf,
    pub(crate) cache_layout: CacheLayout,
}

pub(crate) fn build_program_context(
    path: &str,
    extra_roots: &[PathBuf],
    roots_only: bool,
    cache_dir: Option<&Path>,
    trace_aether: bool,
    use_native_backend: bool,
) -> Result<ProgramContext, String> {
    let source = fs::read_to_string(path).map_err(|e| format!("Error reading {}: {}", path, e))?;
    let entry_path = Path::new(path).to_path_buf();
    let cache_layout = resolve_cache_layout(&entry_path, cache_dir);

    let parse_start = Instant::now();
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
    let mut parse_warnings = parser.take_warnings();
    tag_and_attach_file(
        &mut parse_warnings,
        flux::diagnostics::DiagnosticPhase::Parse,
        path,
    );
    all_diagnostics.append(&mut parse_warnings);

    let entry_has_errors = !parser.errors.is_empty();
    if entry_has_errors {
        tag_and_attach_file(
            &mut parser.errors,
            flux::diagnostics::DiagnosticPhase::Parse,
            path,
        );
        all_diagnostics.append(&mut parser.errors);
    }

    let mut program = program;
    let mut primops_import_diags =
        validate_no_primops_import(&program, parser.interner(), path);
    if !primops_import_diags.is_empty() {
        tag_and_attach_file(
            &mut primops_import_diags,
            flux::diagnostics::DiagnosticPhase::Parse,
            path,
        );
        all_diagnostics.append(&mut primops_import_diags);
    }
    if !trace_aether {
        inject_flow_prelude(&mut program, &mut parser, use_native_backend);
    }

    let interner = parser.take_interner();
    let roots = collect_roots(&entry_path, extra_roots, roots_only);

    let mut graph_result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &roots);
    let parse_ms = parse_start.elapsed().as_secs_f64() * 1000.0;
    let mut graph_diags = std::mem::take(&mut graph_result.diagnostics);
    tag_and_attach_file(
        &mut graph_diags,
        flux::diagnostics::DiagnosticPhase::ModuleGraph,
        path,
    );
    all_diagnostics.extend(graph_diags);

    Ok(ProgramContext {
        source,
        program,
        graph_result,
        entry_has_errors,
        parse_ms,
        all_diagnostics,
        entry_path,
        cache_layout,
    })
}
