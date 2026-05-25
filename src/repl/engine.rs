//! The Phase 2 persistent REPL engine (proposal 0176).
//!
//! Unlike the Phase 1 accumulate-source engine (which re-ran the whole session
//! every line), this keeps **one** prelude-loaded [`Compiler`] and **one** live
//! [`VM`] for the session's lifetime. Each entered line is compiled as a *delta*
//! on the persistent compiler — earlier session globals stay resolvable through
//! the compiler's symbol table, so a line like `let y = x + 1` resolves `x` to
//! its existing slot — and the delta bytecode is run on the live VM via
//! [`VM::run_chunk`], which keeps `globals` across chunks. Earlier declarations
//! therefore never recompile and their side effects never re-fire.
//!
//! Pure bindings/declarations compile straight to session globals (they persist
//! and run exactly once). A bare expression is wrapped as a top-level
//! `let __repl_N = <expr>` (so its value persists and `it` can reference it)
//! plus a fresh `fn main` that prints it; an *effectful* expression can't be a
//! top-level binding (Flux rejects top-level effects, E413), so it falls back to
//! running inside `main` — its result is not captured by `it` (a documented v1
//! gap).
//!
//! A line that fails to compile or run is rolled back by restoring a cheap clone
//! of the compiler taken before the attempt, so the session never enters a
//! broken state. (Globals the failed chunk partially wrote are left in the VM
//! but become unreachable once the compiler rolls back, and the slot is reused.)

use crate::compiler::Compiler;
use crate::diagnostics::Diagnostic;
use crate::driver::pipeline::program::{
    ReplBootstrap, RunProgramRequest, bootstrap_repl_session, infer_repl_expr_type,
    render_repl_diagnostics,
};
use crate::driver::shared::DriverDiagnosticConfig;
use crate::syntax::{lexer::Lexer, parser::Parser, program::Program, statement::Statement};
use crate::vm::VM;

/// One persistent REPL session: a prelude-loaded compiler + a live VM, mutated
/// in place as each line compiles to a delta and runs.
pub(super) struct ReplEngine {
    compiler: Compiler,
    vm: VM,
    /// Optimization level carried from the session so per-line compiles match
    /// the prelude's.
    optimize: bool,
    analyze: bool,
    diagnostics: DriverDiagnosticConfig,
    /// Synthetic entry path used for diagnostic file tagging.
    path: String,
    /// Monotonic counter for unique `it` result bindings (`it` can't be re-`let`,
    /// so each expression result gets a fresh `__repl_N`).
    result_counter: usize,
    /// The most recent expression result binding name; `it` resolves to it.
    last_result: Option<String>,
    /// The committed top-level declarations, in entry order. Kept as a
    /// lightweight parallel record only for `:type` (re-inference over a fresh
    /// compile) and `:list`; execution itself uses the persistent compiler. A
    /// rebind replaces the earlier entry in place rather than appending, so this
    /// record stays a duplicate-free, compilable snapshot of the session.
    committed: Vec<SessionDecl>,
}

/// One committed declaration in the parallel `:type` / `:list` record.
struct SessionDecl {
    /// The top-level name this binds, when it's a `let` / `fn` — used to supersede
    /// an earlier definition on rebinding. `None` for forms we don't dedup
    /// (`data` / `import` / `effect` / …) and for expression result bindings,
    /// whose `__repl_N` names are already unique.
    name: Option<String>,
    source: String,
}

/// The outcome of attempting one wrapped/standalone line.
enum LineOutcome {
    /// Compiled and ran cleanly (session state already advanced).
    Committed,
    /// Compile (or parse) failed; `diagnostics` describe why, against `source`.
    CompileFailed {
        source: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// Compiled but the chunk errored at runtime; already rolled back.
    RuntimeFailed(String),
}

impl ReplEngine {
    /// Build the session: load + compile the Flow prelude into one compiler and
    /// populate a live VM's globals from it. See [`bootstrap_repl_session`].
    pub(super) fn bootstrap(request: RunProgramRequest<'_>) -> Result<Self, String> {
        let ReplBootstrap {
            compiler,
            vm,
            optimize,
            analyze,
            diagnostics,
            path,
        } = bootstrap_repl_session(request)?;
        Ok(Self {
            compiler,
            vm,
            optimize,
            analyze,
            diagnostics,
            path,
            result_counter: 0,
            last_result: None,
            committed: Vec::new(),
        })
    }

    /// Evaluate a top-level declaration line (`let` / `fn` / `data` / `import` /
    /// `effect` / `class` / `instance` / `module` / `alias`). Compiled as-is at
    /// file scope so it persists as a session global; its initializer runs once.
    pub(super) fn eval_decl(&mut self, line: &str) -> bool {
        let resolved = self.resolve_it(line);
        match self.run_line(resolved.clone()) {
            LineOutcome::Committed => {
                let name = top_level_binding_name(&resolved);
                self.commit(name, resolved);
                true
            }
            LineOutcome::CompileFailed {
                source,
                diagnostics,
            } => {
                self.render(&diagnostics, &source);
                false
            }
            LineOutcome::RuntimeFailed(err) => {
                eprintln!("{err}");
                false
            }
        }
    }

    /// Evaluate a bare expression: bind it to a fresh `__repl_N` session global
    /// and print it (so `it` resolves to the value next line). If the expression
    /// is effectful — and so can't be a top-level binding — fall back to running
    /// it inside `main` without capturing `it`.
    pub(super) fn eval_expr(&mut self, line: &str) -> bool {
        let resolved = self.resolve_it(line);
        let name = format!("__repl_{}", self.result_counter);
        let binding = format!("let {name} = {resolved}");
        let pure = format!("{binding}\nfn main() with IO {{\n    println({name})\n}}\n");

        match self.run_line(pure) {
            LineOutcome::Committed => {
                self.commit(Some(name.clone()), binding);
                self.last_result = Some(name);
                self.result_counter += 1;
                true
            }
            LineOutcome::RuntimeFailed(err) => {
                eprintln!("{err}");
                false
            }
            LineOutcome::CompileFailed {
                source,
                diagnostics,
            } => {
                // A pure top-level binding is rejected for an effectful
                // expression (E413). Re-run it inside `main` instead — the effect
                // happens, but the result isn't captured by `it`.
                if has_top_level_effect_error(&diagnostics) {
                    self.eval_effectful_expr(&resolved)
                } else {
                    self.render(&diagnostics, &source);
                    false
                }
            }
        }
    }

    /// Run an effectful expression inside `main` (the only sanctioned effect
    /// entry). Nothing persists and `it` is left unchanged.
    fn eval_effectful_expr(&mut self, resolved: &str) -> bool {
        let source = format!("fn main() with IO {{\n    {resolved}\n}}\n");
        match self.run_line(source) {
            LineOutcome::Committed => true,
            LineOutcome::CompileFailed {
                source,
                diagnostics,
            } => {
                self.render(&diagnostics, &source);
                false
            }
            LineOutcome::RuntimeFailed(err) => {
                eprintln!("{err}");
                false
            }
        }
    }

    /// Infer and return the type of `expr` in the current session, without
    /// running it. Re-inferred over a fresh compile of the committed session
    /// source plus the query (so all session bindings resolve), mirroring the
    /// LSP hover path. `it` resolves to the latest result.
    pub(super) fn infer_type(&self, request: RunProgramRequest<'_>, expr: &str) -> Option<String> {
        let resolved = self.resolve_it(expr);
        let source = self.assemble_type_query(&resolved);
        infer_repl_expr_type(request, source)
    }

    /// The committed top-level declaration sources, for `:list`.
    pub(super) fn listing(&self) -> Vec<&str> {
        self.committed.iter().map(|d| d.source.as_str()).collect()
    }

    /// Record a committed declaration, replacing an earlier one that bound the
    /// same name (rebinding) in place so the `:type` / `:list` record stays a
    /// duplicate-free snapshot. Entries with no dedup name (or a unique
    /// `__repl_N`) are appended.
    fn commit(&mut self, name: Option<String>, source: String) {
        if let Some(ref n) = name
            && let Some(pos) = self
                .committed
                .iter()
                .position(|d| d.name.as_deref() == Some(n.as_str()))
        {
            self.committed[pos] = SessionDecl { name, source };
        } else {
            self.committed.push(SessionDecl { name, source });
        }
    }

    /// Replace standalone `it` tokens with the latest result binding's name.
    fn resolve_it(&self, src: &str) -> String {
        match &self.last_result {
            Some(prev) => super::rewrite_it(src, prev),
            None => src.to_string(),
        }
    }

    /// Compile `source` as a delta on the persistent compiler and run that delta
    /// on the live VM. Any failure restores a pre-attempt clone of the compiler.
    fn run_line(&mut self, source: String) -> LineOutcome {
        // Checkpoint *before* parsing so a failed line's interned symbols and
        // global definitions all roll back together.
        let checkpoint = self.compiler.clone();

        let (program, parse_errors) = self.parse(&source);
        if !parse_errors.is_empty() {
            self.compiler = checkpoint;
            return LineOutcome::CompileFailed {
                source,
                diagnostics: parse_errors,
            };
        }

        // Allow rebinding: a line that redefines an existing session name forgets
        // the old binding so the compiler's duplicate-definition check doesn't
        // reject it. The new binding lands on a fresh slot (clean shadowing). A
        // self-referential rebind (`let x = x + 1`) errors as undefined — a
        // documented v1 limitation — rather than silently reading a stale slot.
        self.forget_redefined(&program);

        // The offset at which this line's instructions will be appended. The VM
        // runs the *full* buffer from here so the new tail's absolute jump targets
        // (top-level `if` / `match`) resolve, while the prelude and earlier lines
        // are skipped.
        let start = self.compiler.top_level_instruction_len();
        if let Err(diagnostics) =
            self.compiler
                .compile_with_opts(&program, self.optimize, self.analyze)
        {
            self.compiler = checkpoint;
            return LineOutcome::CompileFailed {
                source,
                diagnostics,
            };
        }

        if let Err(err) = self.vm.run_top_level(self.compiler.bytecode(), start) {
            // The run may have written some globals before failing; restoring the
            // compiler makes them unreachable (the slot is reused later), and the
            // next line re-issues the rolled-back buffer, restoring the constants.
            self.compiler = checkpoint;
            return LineOutcome::RuntimeFailed(err);
        }

        LineOutcome::Committed
    }

    /// Forget any existing session binding that this line's top-level `let` / `fn`
    /// redefines, so the compiler accepts the rebind. `main` is left to the
    /// compiler (it already permits a fresh `main` each expression line).
    fn forget_redefined(&mut self, program: &Program) {
        let main = self.compiler.interner.lookup("main");
        for statement in &program.statements {
            let name = match statement {
                Statement::Let { name, .. } => *name,
                Statement::Function { name, .. } => *name,
                _ => continue,
            };
            if Some(name) == main {
                continue;
            }
            if self.compiler.symbol_table.exists_in_current_scope(name) {
                self.compiler.forget_session_binding(name);
            }
        }
    }

    /// Parse `source` against the compiler's interner so identifier IDs line up
    /// with the persistent symbol table (the `parse_module_for_goto_def` idiom).
    fn parse(&mut self, source: &str) -> (Program, Vec<Diagnostic>) {
        let interner = std::mem::take(&mut self.compiler.interner);
        let mut parser = Parser::new(Lexer::new_with_interner(source, interner));
        let program = parser.parse_program();
        let errors = parser.errors.clone();
        self.compiler.interner = parser.take_interner();
        (program, errors)
    }

    /// Build a `:type` query program: every committed declaration at file scope,
    /// then the query bound inside `main` so [`infer_repl_expr_type`] can read
    /// its inferred type back.
    fn assemble_type_query(&self, query: &str) -> String {
        use crate::driver::pipeline::program::REPL_TYPE_BINDING;
        let mut out = String::new();
        for decl in &self.committed {
            out.push_str(&decl.source);
            out.push('\n');
        }
        out.push_str("fn main() with IO {\n    let ");
        out.push_str(REPL_TYPE_BINDING);
        out.push_str(" = ");
        out.push_str(query);
        out.push_str("\n}\n");
        out
    }

    /// Render a line's error diagnostics (warnings dropped) against its source.
    fn render(&self, diagnostics: &[Diagnostic], source: &str) {
        render_repl_diagnostics(diagnostics, &self.path, source, &self.diagnostics);
    }
}

/// Whether the diagnostics include the "top-level effectful expression" error
/// (E413) — the signal that a bare expression must run inside `main` instead of
/// being bound as a top-level session global.
fn has_top_level_effect_error(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|diag| diag.code() == Some("E413"))
}

/// The top-level `let` / `fn` name a committed declaration line binds, for
/// deduping the `:type` / `:list` record on rebinding. A throwaway parse is
/// enough — only the binding's spelling is needed, not its interned identity.
fn top_level_binding_name(source: &str) -> Option<String> {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    let interner = parser.interner();
    program
        .statements
        .iter()
        .find_map(|statement| match statement {
            Statement::Let { name, .. } | Statement::Function { name, .. } => {
                Some(interner.resolve(*name).to_string())
            }
            _ => None,
        })
}
