use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    bytecode::compiler::Compiler,
    diagnostics::{Diagnostic, DiagnosticPhase},
    driver::shared::{format_scheme_for_cli, tag_diagnostics},
    syntax::{
        block::Block,
        interner::Interner,
        module_graph::{ModuleKind, ModuleNode},
        program::Program,
        statement::Statement,
    },
    types::module_interface::ModuleInterface,
};

/// Tracks interface-diff and rebuild state for a single module build.
///
/// This struct is used by driver-level incremental compilation code to record
/// whether a module's public interface changed relative to the previously
/// cached interface and whether downstream work must be repeated.
///
/// Fields:
/// - `old_interface_fingerprint`: fingerprint loaded from the previous cached
///   interface, if one existed.
/// - `new_interface_fingerprint`: fingerprint produced by the current
///   compilation pass, if compilation reached interface generation.
/// - `interface_changed`: whether the public interface changed between old and
///   new states. Downstream module recompilation decisions are driven from
///   this.
/// - `rebuild_required`: whether this module must be rebuilt even if cached
///   artifacts exist.
/// - `skipped`: whether the driver reused existing artifacts and skipped
///   rebuilding the module body.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModuleBuildState {
    pub(crate) old_interface_fingerprint: Option<String>,
    pub(crate) new_interface_fingerprint: Option<String>,
    pub(crate) interface_changed: bool,
    pub(crate) rebuild_required: bool,
    pub(crate) skipped: bool,
}

/// Returns `true` when the statement directly or transitively declares a user
/// ADT.
///
/// A top-level `data` statement counts immediately. Module statements recurse
/// into their body so nested module declarations contribute to the result as
/// well. All other statement kinds are ignored.
fn statement_has_user_adt_declarations(statement: &Statement) -> bool {
    match statement {
        Statement::Data { .. } => true,
        Statement::Module { body, .. } => block_has_user_adt_declarations(body),
        _ => false,
    }
}

/// Returns `true` when any statement in the block declares a user ADT.
///
/// This is the block-level worker used by
/// [`statement_has_user_adt_declarations`] and ultimately
/// [`program_has_user_adt_declarations`]. The traversal is purely structural
/// and does not depend on name resolution, type inference, or backend IR.
fn block_has_user_adt_declarations(block: &Block) -> bool {
    block
        .statements
        .iter()
        .any(statement_has_user_adt_declarations)
}

/// Returns `true` if a program declares any user-defined ADT.
///
/// This walks the top-level statements and nested module bodies looking for
/// `data` declarations. The helper is intentionally structural: it does not
/// inspect inferred types or backend IR and can be safely used before later
/// compilation stages run.
///
/// Driver code uses this to make decisions that depend on whether the source
/// introduces user ADTs, such as cache invalidation or narrowing regression
/// checks to ADT-sensitive paths.
pub(crate) fn program_has_user_adt_declarations(program: &Program) -> bool {
    program
        .statements
        .iter()
        .any(statement_has_user_adt_declarations)
}

/// Tags a module's diagnostics with a phase and default file path.
///
/// Any diagnostic missing a phase is assigned `phase`. Any diagnostic missing a
/// file is assigned `path`. Existing phase and file annotations are preserved.
///
/// This should be used whenever diagnostics are produced from isolated module
/// compilation so downstream rendering can attribute the message to the correct
/// source file and pipeline stage.
pub(crate) fn tag_module_diagnostics(
    diags: &mut Vec<Diagnostic>,
    phase: DiagnosticPhase,
    path: &Path,
) {
    tag_diagnostics(diags, phase);
    for diag in diags {
        if diag.file().is_none() {
            diag.set_file(path.to_string_lossy().to_string());
        }
    }
}

/// Constructs a `Compiler` preloaded for compiling one module in a module graph.
///
/// The returned compiler is configured with:
/// - the module's source path and cloned interner
/// - the current module kind (`Flow` stdlib vs user module)
/// - strict-mode and strict-types policy, with `Flow` stdlib modules always
///   compiled non-strict
/// - `require main` enforcement for entry modules only
/// - preloaded dependency interfaces for imported modules
/// - preloaded dependency programs for imported modules
///
/// Non-`Flow` modules also receive all known `Flow` stdlib interfaces and ASTs
/// even when they were not imported explicitly. This mirrors Flux's auto-Flow
/// prelude behavior in sequential compilation paths and keeps parallel module
/// compilation semantically aligned with the main driver pipeline.
///
/// This helper is a driver-layer adapter around `bytecode::compiler::Compiler`;
/// it does not perform compilation by itself.
pub(crate) fn build_module_compiler(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
    interner: &Interner,
    strict_mode: bool,
    strict_types: bool,
    is_entry_module: bool,
) -> Compiler {
    let mut compiler =
        Compiler::new_with_interner(node.path.to_string_lossy().to_string(), interner.clone());
    compiler.set_current_module_kind(node.kind);
    compiler.set_strict_require_main(is_entry_module);
    compiler.set_strict_mode(node.kind != ModuleKind::FlowStdlib && strict_mode);
    compiler.set_strict_types(node.kind != ModuleKind::FlowStdlib && strict_types);

    for dep in &node.imports {
        if let Some(interface) = loaded_interfaces.get(&dep.target_path) {
            compiler.preload_module_interface(interface);
        }
        if let Some(dep_node) = nodes_by_path.get(&dep.target_path) {
            compiler.preload_dependency_program(&dep_node.program);
        }
    }
    if node.kind != ModuleKind::FlowStdlib {
        for (path, interface) in loaded_interfaces {
            if !node.imports.iter().any(|dep| &dep.target_path == path)
                && nodes_by_path
                    .get(path)
                    .is_some_and(|dep_node| dep_node.kind == ModuleKind::FlowStdlib)
            {
                compiler.preload_module_interface(interface);
            }
        }
        for (path, dep_node) in nodes_by_path {
            if !node.imports.iter().any(|dep| &dep.target_path == path)
                && dep_node.kind == ModuleKind::FlowStdlib
            {
                compiler.preload_dependency_program(&dep_node.program);
            }
        }
    }
    if node.kind == ModuleKind::FlowStdlib {
        compiler.set_strict_mode(false);
        compiler.set_strict_types(false);
    }
    compiler
}

/// Re-runs module compilation to recover warnings and errors for reporting.
///
/// Some parallel and cache-aware driver paths first compile modules to decide
/// cache reuse or interface changes and later need a stable way to reproduce
/// diagnostics for user-facing reporting. This helper rebuilds a compiler with
/// the same dependency preload context, compiles the module again, and returns
/// all warnings and errors with consistent phase/file annotations.
///
/// Diagnostic tagging policy:
/// - warnings emitted by the compiler are tagged as `Validation`
/// - hard compilation failures are tagged as `TypeCheck`
///
/// The returned vector contains both warnings and errors in the order they are
/// collected.
pub(crate) fn replay_module_diagnostics(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
    interner: &Interner,
    strict_mode: bool,
    strict_types: bool,
    enable_optimize: bool,
    enable_analyze: bool,
) -> Vec<Diagnostic> {
    let mut compiler = build_module_compiler(
        node,
        nodes_by_path,
        loaded_interfaces,
        interner,
        strict_mode,
        strict_types,
        false,
    );
    let compile_result = compiler.compile_with_opts(&node.program, enable_optimize, enable_analyze);
    let mut diagnostics = compiler.take_warnings();
    tag_module_diagnostics(&mut diagnostics, DiagnosticPhase::Validation, &node.path);
    if let Err(mut diags) = compile_result {
        tag_module_diagnostics(&mut diags, DiagnosticPhase::TypeCheck, &node.path);
        diagnostics.extend(diags);
    }
    diagnostics
}

/// Logs the public-surface diff between two module interfaces.
///
/// The diff is written to stderr in a compact CLI-oriented format:
/// - `+` for newly exported values
/// - `-` for removed exported values
/// - `~` for exported values whose scheme changed
///
/// Only public `schemes` are compared here; implementation details that do not
/// affect the module interface are intentionally ignored.
pub(crate) fn log_interface_diff(old: &ModuleInterface, new: &ModuleInterface) {
    for name in new.schemes.keys() {
        if !old.schemes.contains_key(name) {
            eprintln!(
                "  + public {}: {}",
                name,
                format_scheme_for_cli(&new.schemes[name])
            );
        }
    }
    for name in old.schemes.keys() {
        if !new.schemes.contains_key(name) {
            eprintln!("  - public {}", name);
        }
    }
    for (name, new_scheme) in &new.schemes {
        if let Some(old_scheme) = old.schemes.get(name)
            && old_scheme != new_scheme
        {
            eprintln!(
                "  ~ public {}: {} -> {}",
                name,
                format_scheme_for_cli(old_scheme),
                format_scheme_for_cli(new_scheme)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        diagnostics::{Diagnostic, DiagnosticPhase, position::Span},
        driver::module_compile::{
            ModuleBuildState, program_has_user_adt_declarations, tag_module_diagnostics,
        },
        syntax::{block::Block, program::Program, statement::Statement, symbol::Symbol},
    };
    use std::path::Path;

    #[test]
    fn module_build_state_defaults_to_no_change_and_not_skipped() {
        let state = ModuleBuildState::default();

        assert_eq!(state.old_interface_fingerprint, None);
        assert_eq!(state.new_interface_fingerprint, None);
        assert!(!state.interface_changed);
        assert!(!state.rebuild_required);
        assert!(!state.skipped);
    }

    #[test]
    fn detects_top_level_data_declarations() {
        let program = Program {
            statements: vec![Statement::Data {
                is_public: false,
                name: Symbol::SENTINEL,
                type_params: Vec::new(),
                variants: Vec::new(),
                deriving: Vec::new(),
                span: Span::default(),
            }],
            span: Span::default(),
        };

        assert!(program_has_user_adt_declarations(&program));
    }

    #[test]
    fn detects_nested_module_data_declaration() {
        let program = Program {
            statements: vec![Statement::Module {
                name: Symbol::SENTINEL,
                body: Block {
                    statements: vec![Statement::Data {
                        is_public: true,
                        name: Symbol::SENTINEL,
                        type_params: Vec::new(),
                        variants: Vec::new(),
                        deriving: Vec::new(),
                        span: Span::default(),
                    }],
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            span: Span::default(),
        };

        assert!(program_has_user_adt_declarations(&program));
    }

    #[test]
    fn ignores_module_without_data_declarations() {
        let program = Program {
            statements: vec![Statement::Module {
                name: Symbol::SENTINEL,
                body: Block {
                    statements: Vec::new(),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            span: Span::default(),
        };

        assert!(!program_has_user_adt_declarations(&program));
    }

    #[test]
    fn ignores_program_without_data_declarations() {
        let program = Program::new();

        assert!(!program_has_user_adt_declarations(&program));
    }

    #[test]
    fn tag_module_diagnostics_sets_missing_phase_and_file() {
        let mut diags = vec![Diagnostic::warning("warn")];

        tag_module_diagnostics(
            &mut diags,
            DiagnosticPhase::Validation,
            Path::new("tests/fixtures/example.flx"),
        );

        assert_eq!(diags[0].phase(), Some(DiagnosticPhase::Validation));
        assert_eq!(diags[0].file(), Some("tests/fixtures/example.flx"));
    }

    #[test]
    fn tag_module_diagnostics_preserves_existing_phase_and_file() {
        let mut diag = Diagnostic::warning("warn").with_phase(DiagnosticPhase::Parse);
        diag.set_file("already-set.flx");
        let mut diags = vec![diag];

        tag_module_diagnostics(
            &mut diags,
            DiagnosticPhase::TypeCheck,
            Path::new("replacement.flx"),
        );

        assert_eq!(diags[0].phase(), Some(DiagnosticPhase::Parse));
        assert_eq!(diags[0].file(), Some("already-set.flx"));
    }
}
