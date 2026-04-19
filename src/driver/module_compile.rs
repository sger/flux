#![cfg_attr(not(feature = "llvm"), allow(dead_code))]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate as flux;
use crate::{
    compiler::Compiler,
    diagnostics::{Diagnostic, DiagnosticPhase},
    syntax::{
        interner::Interner,
        module_graph::{ModuleKind, ModuleNode},
        program::Program,
    },
    types::module_interface::ModuleInterface,
};

use super::support::shared::{format_scheme_for_cli, tag_diagnostics};

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(crate) struct ModuleBuildState {
    pub(crate) old_interface_fingerprint: Option<String>,
    pub(crate) new_interface_fingerprint: Option<String>,
    pub(crate) interface_changed: bool,
    pub(crate) rebuild_required: bool,
    pub(crate) skipped: bool,
}

pub(crate) fn program_has_user_adt_declarations(program: &Program) -> bool {
    fn block_has_user_adt_declarations(block: &flux::syntax::block::Block) -> bool {
        block
            .statements
            .iter()
            .any(statement_has_user_adt_declarations)
    }

    fn statement_has_user_adt_declarations(statement: &flux::syntax::statement::Statement) -> bool {
        match statement {
            flux::syntax::statement::Statement::Data { .. } => true,
            flux::syntax::statement::Statement::Module { body, .. } => {
                block_has_user_adt_declarations(body)
            }
            _ => false,
        }
    }

    program
        .statements
        .iter()
        .any(statement_has_user_adt_declarations)
}

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

pub(crate) fn build_module_compiler(
    node: &ModuleNode,
    nodes_by_path: &HashMap<PathBuf, ModuleNode>,
    loaded_interfaces: &HashMap<PathBuf, ModuleInterface>,
    base_interner: &Interner,
    entry_module_kind: ModuleKind,
    strict_mode: bool,
    is_entry_module: bool,
) -> Compiler {
    let strict_mode = effective_module_strictness(node.kind, entry_module_kind, strict_mode);
    let mut compiler = Compiler::new_with_interner(
        node.path.to_string_lossy().to_string(),
        base_interner.clone(),
    );
    compiler.set_current_module_kind(node.kind);
    compiler.set_strict_require_main(is_entry_module);
    compiler.set_strict_mode(strict_mode);
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
    compiler
}

pub(crate) fn effective_module_strictness(
    module_kind: ModuleKind,
    entry_module_kind: ModuleKind,
    strict_mode: bool,
) -> bool {
    if module_kind == ModuleKind::FlowStdlib && entry_module_kind != ModuleKind::FlowStdlib {
        false
    } else {
        strict_mode
    }
}

/// Grouped inputs for replaying module diagnostics through a fresh compiler instance.
pub(crate) struct ModuleReplayRequest<'a> {
    pub(crate) node: &'a ModuleNode,
    pub(crate) nodes_by_path: &'a HashMap<PathBuf, ModuleNode>,
    pub(crate) loaded_interfaces: &'a HashMap<PathBuf, ModuleInterface>,
    pub(crate) base_interner: &'a Interner,
    pub(crate) entry_module_kind: ModuleKind,
    pub(crate) strict_mode: bool,
    pub(crate) enable_optimize: bool,
    pub(crate) enable_analyze: bool,
}

/// Replays module compilation to reconstruct diagnostics after parallel work.
pub(crate) fn replay_module_diagnostics(request: ModuleReplayRequest<'_>) -> Vec<Diagnostic> {
    let mut compiler = build_module_compiler(
        request.node,
        request.nodes_by_path,
        request.loaded_interfaces,
        request.base_interner,
        request.entry_module_kind,
        request.strict_mode,
        false,
    );
    let compile_result = compiler.compile_with_opts(
        &request.node.program,
        request.enable_optimize,
        request.enable_analyze,
    );
    let mut diagnostics = compiler.take_warnings();
    tag_module_diagnostics(
        &mut diagnostics,
        DiagnosticPhase::Validation,
        &request.node.path,
    );
    if let Err(mut diags) = compile_result {
        tag_module_diagnostics(&mut diags, DiagnosticPhase::TypeCheck, &request.node.path);
        diagnostics.extend(diags);
    }
    diagnostics
}

pub(crate) fn log_interface_diff(
    old: &flux::types::module_interface::ModuleInterface,
    new: &flux::types::module_interface::ModuleInterface,
    interner: &crate::syntax::interner::Interner,
) {
    for name in new.schemes.keys() {
        if !old.schemes.contains_key(name) {
            eprintln!(
                "  + public {}: {}",
                name,
                format_scheme_for_cli(interner, &new.schemes[name])
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
                format_scheme_for_cli(interner, old_scheme),
                format_scheme_for_cli(interner, new_scheme)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{effective_module_strictness, program_has_user_adt_declarations};
    use crate::{
        diagnostics::position::Span,
        syntax::{
            Identifier, block::Block, module_graph::ModuleKind, program::Program,
            statement::Statement, symbol::Symbol,
        },
    };

    fn sentinel() -> Identifier {
        Symbol::SENTINEL
    }

    #[test]
    fn detects_top_level_data_declaration() {
        let program = Program {
            statements: vec![Statement::Data {
                is_public: false,
                name: sentinel(),
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
                name: sentinel(),
                body: Block {
                    statements: vec![Statement::Data {
                        is_public: true,
                        name: sentinel(),
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
    fn ignores_program_without_data_declarations() {
        let program = Program::new();

        assert!(!program_has_user_adt_declarations(&program));
    }

    #[test]
    fn user_entry_disables_strictness_for_flow_stdlib_dependencies() {
        let strictness =
            effective_module_strictness(ModuleKind::FlowStdlib, ModuleKind::User, true);

        assert!(!strictness);
    }

    #[test]
    fn flow_entry_preserves_strictness_for_flow_stdlib_modules() {
        let strictness =
            effective_module_strictness(ModuleKind::FlowStdlib, ModuleKind::FlowStdlib, true);

        assert!(strictness);
    }
}
