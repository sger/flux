//! The debugger tables a `-g` build adds to each function (proposal 0188
//! S2): stop sites, where the compiler emits `OpBreakpoint`, and the local
//! variables, with the instructions where each holds its value.

use crate::bytecode::debug_info::{BreakSite, FunctionDebugInfo, LocalVar, Location, SiteKind};
use crate::bytecode::op_code::OpCode;
use crate::compiler::binding::Binding;
use crate::diagnostics::position::Span;
use crate::syntax::symbol::Symbol;

use super::Compiler;

/// The stop sites and local variables of one finished function.
pub(super) type DebugTables = (Vec<BreakSite>, Vec<LocalVar>);

/// `|>` stage sites. A stage stops right before its call instruction, after
/// its input (the previous stage) has run, so a pipeline steps in source
/// order. The site waits here, tagged with the call's nesting depth, until
/// the instruction for that call is emitted.
#[derive(Debug, Clone, Default)]
pub(super) struct PipeSites {
    pub(super) calls: std::collections::HashSet<crate::syntax::expression::ExprId>,
    depth: usize,
    pending: Vec<(usize, Span)>,
    /// A name for the next function literal, which would otherwise be
    /// named as a lambda: handler arms.
    pub(super) next_function_name: Option<String>,
}

impl Compiler {
    /// Whether this build adds the debugger tables.
    #[inline]
    pub(super) fn debug_tables_enabled(&self) -> bool {
        self.debug_level().is_enabled()
    }

    /// Starts logging the locals the current symbol table defines.
    pub(super) fn start_def_log(&mut self) {
        if self.debug_tables_enabled() {
            self.symbol_table.def_log = Some(Vec::new());
        }
    }

    /// Enters the locals defined since the last call into the current
    /// function's table. Each one holds its value from the next instruction
    /// on.
    pub(super) fn drain_def_log(&mut self) {
        let Some(log) = self.symbol_table.def_log.as_mut() else {
            return;
        };
        if log.is_empty() {
            return;
        }
        let defined = std::mem::take(log);
        let scope = &self.scopes[self.scope_index];
        // Parameters are defined after the entry site is emitted, but hold
        // their values from the start.
        let only_sites = scope.instructions.len() == scope.sites.len() * 3;
        let start_ip = if only_sites {
            0
        } else {
            scope.instructions.len()
        };
        for binding in defined {
            let temp = binding.name == Symbol::new(u32::MAX);
            let name = if temp {
                format!("__tmp{}", binding.index)
            } else {
                self.sym(binding.name).to_string()
            };
            let artificial = temp || name.starts_with("__");
            self.scopes[self.scope_index].locals.push(LocalVar {
                slot: binding.index as u32,
                name,
                start_ip,
                end_ip: usize::MAX,
                artificial,
            });
        }
    }

    /// Ends the scope of every open local from `mark` on at the current
    /// instruction.
    pub(super) fn close_locals_from(&mut self, mark: usize) {
        let scope = &mut self.scopes[self.scope_index];
        let end_ip = scope.instructions.len();
        for local in scope.locals.iter_mut().skip(mark) {
            if local.end_ip == usize::MAX {
                local.end_ip = end_ip;
            }
        }
    }

    /// Closes the scope being left and keeps its tables for the function
    /// that is being finished.
    pub(super) fn finish_scope_debug_tables(&mut self) {
        if !self.debug_tables_enabled() {
            return;
        }
        self.drain_def_log();
        self.close_locals_from(0);
        let scope = &mut self.scopes[self.scope_index];
        self.left_debug_tables = Some((
            std::mem::take(&mut scope.sites),
            std::mem::take(&mut scope.locals),
        ));
    }

    /// The tables of the scope `leave_scope` just closed.
    pub(super) fn take_debug_tables(&mut self) -> DebugTables {
        self.left_debug_tables.take().unwrap_or_default()
    }

    /// Adds the tables of the function `leave_scope` just closed to its debug
    /// info, with the names of the values it captures. A lambda is named
    /// after where it is written, as in the debug-info model. Does nothing
    /// without `-g`.
    pub(super) fn with_function_debug_tables(
        &mut self,
        info: FunctionDebugInfo,
        free_symbols: &[Binding],
    ) -> FunctionDebugInfo {
        if !self.debug_tables_enabled() {
            return info;
        }
        let (sites, locals) = self.take_debug_tables();
        let free_names = free_symbols
            .iter()
            .map(|free| self.sym(free.name).to_string())
            .collect();
        // Named after where the lambda starts: its entry site, else its body.
        let start = sites
            .first()
            .filter(|site| site.kind == SiteKind::FnEntry)
            .and_then(|site| site.location.clone())
            .or_else(|| info.boundary_location.clone());
        let mut info = info.with_debug_tables(sites, locals, free_names);
        if info.name.is_none() {
            info.name = self.pipe_sites.next_function_name.take();
        }
        if info.name.is_none()
            && let Some(boundary) = &start
        {
            let file = std::path::Path::new(&self.file_path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.file_path.clone());
            info.name = Some(format!(
                "<lambda {}:{}:{}>",
                file,
                boundary.span.start.line,
                boundary.span.start.column + 1
            ));
        }
        info
    }

    /// Emits a function's entry site at `span`, unless entering its scope
    /// already did.
    pub(super) fn emit_entry_site(&mut self, span: Span) {
        let scope = &self.scopes[self.scope_index];
        if self.debug_tables_enabled() && scope.sites.is_empty() && scope.instructions.is_empty() {
            self.emit_site(SiteKind::FnEntry, span);
        }
    }

    /// Emits the stop site for a statement in a function body. Nested
    /// function declarations get none: their own entry has one.
    pub(super) fn emit_statement_site(&mut self, statement: &crate::syntax::statement::Statement) {
        if self.scope_index == 0
            || !self.debug_tables_enabled()
            || matches!(
                statement,
                crate::syntax::statement::Statement::Function { .. }
            )
        {
            return;
        }
        self.emit_site(SiteKind::Stmt, statement.span());
    }

    /// Compiles `expression`, tracking call nesting so that each `|>` stage's
    /// site is emitted at its own call instruction.
    pub(super) fn compile_expression_tracking_pipes(
        &mut self,
        expression: &crate::syntax::expression::Expression,
    ) -> super::CompileResult<()> {
        use crate::syntax::expression::{ExprId, Expression};
        let Expression::Call {
            id, function, span, ..
        } = expression
        else {
            return self.compile_expression_untracked(expression);
        };
        if self.pipe_sites.calls.is_empty() || *id == ExprId::UNSET {
            // A call the compiler made in place of another (class dispatch,
            // dictionary passing) emits the instruction for the original.
            return self.compile_expression_untracked(expression);
        }
        self.pipe_sites.depth += 1;
        let depth = self.pipe_sites.depth;
        let pending_before = self.pipe_sites.pending.len();
        if self.pipe_sites.calls.contains(id) {
            // The stage is the function and its own arguments, not the input
            // piped into it.
            let stage = Span::new(function.span().start, span.end);
            self.pipe_sites.pending.push((depth, stage));
        }
        let result = self.compile_expression_untracked(expression);
        self.pipe_sites.pending.truncate(pending_before);
        self.pipe_sites.depth -= 1;
        result
    }

    /// Emits the waiting `|>` stage site if the call being emitted is that
    /// stage's call.
    pub(super) fn emit_pending_pipe_site(&mut self) {
        let Some(&(depth, span)) = self.pipe_sites.pending.last() else {
            return;
        };
        if depth == self.pipe_sites.depth {
            self.pipe_sites.pending.pop();
            self.emit_site(SiteKind::PipeStage, span);
        }
    }

    /// The sites before an effect operation runs: the `|>` stage it is, if
    /// any, and the perform itself.
    pub(super) fn emit_perform_sites(&mut self, span: Span) {
        if !self.debug_tables_enabled() {
            return;
        }
        self.emit_pending_pipe_site();
        self.emit_site(SiteKind::Perform, span);
    }

    /// Emits a debugger stop site at `span`. Does nothing without `-g`.
    pub(super) fn emit_site(&mut self, kind: SiteKind, span: Span) {
        // A site with no source line (compiler-made code) is nowhere to stop.
        if !self.debug_tables_enabled() || span.start.line == 0 {
            return;
        }
        let index = self.scopes[self.scope_index].sites.len();
        if index > u16::MAX as usize {
            return;
        }
        let previous_span = self.current_span;
        self.current_span = Some(span);
        let ip = self.emit(OpCode::OpBreakpoint, &[index]);
        self.current_span = previous_span;
        let location = self.scopes[self.scope_index]
            .locations
            .iter()
            .rev()
            .find(|entry| entry.offset == ip)
            .and_then(|entry| entry.location.clone())
            .or(Some(Location { file_id: 0, span }));
        self.scopes[self.scope_index]
            .sites
            .push(BreakSite { kind, ip, location });
    }
}
