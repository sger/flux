//! The Phase 2 persistent REPL engine (proposal 0176).
//!
//! Unlike the Phase 1 accumulate-source engine (which re-ran the whole session
//! every line), this keeps **one** prelude-loaded [`Compiler`] and **one** live
//! [`VM`] for the session's lifetime. Each entered line is compiled as a *delta*
//! on the persistent compiler — earlier session globals stay resolvable through
//! the compiler's symbol table, so a line like `let y = x + 1` resolves `x` to
//! its existing slot — and the freshly-compiled tail is run on the live VM via
//! [`VM::run_top_level`], which keeps `globals` across lines. Earlier
//! declarations therefore never recompile and their side effects never re-fire.
//!
//! Pure bindings/declarations compile straight to session globals (they persist
//! and run exactly once). A bare expression is wrapped as a top-level
//! `let __repl_N = <expr>` (so its value persists and `it` can reference it)
//! plus a fresh `fn main` that prints it; an *effectful* expression can't be a
//! top-level binding (Flux rejects top-level effects, E413), so it falls back to
//! running inside `main` — and when its result has a faithful literal form
//! (a primitive, or a list / tuple / ADT of such) it is re-bound so `it` captures
//! it without re-running the effect. Only resultless effects (`Unit`/`None` from
//! `print(..)`) and unrenderable values (maps, closures) leave `it` unchanged.
//!
//! A line that fails to compile or run is rolled back by restoring a cheap clone
//! of the compiler taken before the attempt, so the session never enters a
//! broken state. (Globals the failed chunk partially wrote are left in the VM
//! but become unreachable once the compiler rolls back, and the slot is reused.)

use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::free_vars::collect_free_vars;
use crate::compiler::Compiler;
use crate::diagnostics::Diagnostic;
use crate::driver::pipeline::program::{
    ReplBootstrap, RunProgramRequest, bootstrap_repl_session, infer_repl_expr_type,
    render_repl_diagnostics,
};
use crate::driver::shared::DriverDiagnosticConfig;
use crate::runtime::value::Value;
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
    /// The file most recently brought in with `:load`, replayed by `:reload`.
    loaded: Option<PathBuf>,
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
            loaded: None,
        })
    }

    /// Load a `.flx` file into the session: compile its whole source as one delta
    /// (so intra-file references all resolve in a single compile, unlike feeding
    /// it line by line) and run it, keeping its top-level definitions in scope.
    /// The path is recorded for `:reload`. Returns the number of top-level
    /// declarations on success; `None` after printing the failure — an I/O error,
    /// or a compile / runtime error rendered against the file.
    pub(super) fn load_file(&mut self, path: &Path) -> Option<usize> {
        let source = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("cannot read {}: {err}", path.display());
                return None;
            }
        };
        let count = {
            let mut parser = Parser::new(Lexer::new(&source));
            parser.parse_program().statements.len()
        };
        match self.run_line(source.clone()) {
            LineOutcome::Committed => {
                // Keep the loaded source in the `:type` / `:list` record as one
                // block so `:type` over a loaded name re-infers against it.
                self.commit(None, source);
                self.loaded = Some(path.to_path_buf());
                Some(count)
            }
            LineOutcome::CompileFailed {
                source,
                diagnostics,
            } => {
                self.render(&diagnostics, &source);
                None
            }
            LineOutcome::RuntimeFailed(err) => {
                eprintln!("{err}");
                None
            }
        }
    }

    /// The file recorded by the last successful `:load`, replayed by `:reload`.
    pub(super) fn loaded_path(&self) -> Option<&PathBuf> {
        self.loaded.as_ref()
    }

    /// Evaluate a top-level declaration line (`let` / `fn` / `data` / `import` /
    /// `effect` / `class` / `instance` / `module` / `alias`). Compiled as-is at
    /// file scope so it persists as a session global; its initializer runs once.
    pub(super) fn eval_decl(&mut self, line: &str) -> bool {
        let resolved = self.resolve_it(line);
        // A self-referential rebind (`let x = x + 1`, where `x` is already a
        // session binding) needs the initializer to read the *previous* value.
        // Compiling it directly would define a fresh `x` slot before the
        // initializer and read it back uninitialized; handle it specially.
        if let Some((name, init)) = self.self_referential_rebind(&resolved) {
            return self.eval_self_rebind(&resolved, &name, &init);
        }
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
                // A declaration whose initializer is effectful (e.g.
                // `let x = read_line()`) is rejected at top level (E413). A *named*
                // `let name = init` persists by capturing its initializer's value
                // (the effect fires once inside `main`, then `name` is re-bound to
                // the captured literal); a bare effectful expression — or
                // `let _ = print(..)` — just runs for effect, with its result
                // captured into `it` when it has a literal form.
                if has_top_level_effect_error(&diagnostics) {
                    if let Some((name, init)) = self.named_let_binding(&resolved) {
                        self.eval_effectful_named_binding(&name, &init)
                    } else {
                        self.eval_effectful_expr(&resolved)
                    }
                } else {
                    self.render(&diagnostics, &source);
                    false
                }
            }
            LineOutcome::RuntimeFailed(err) => {
                eprintln!("{err}");
                false
            }
        }
    }

    /// Detect a *self-referential rebind*: a single top-level `let name = init`
    /// where `init` reads `name` (a free use, scope-aware) and `name` already
    /// names a session binding. Returns `(name, init_source)` so the caller can
    /// evaluate `init` against the previous value. `None` for anything else —
    /// a fresh `let x = x` is a genuine unbound reference and is left to the
    /// normal path to report.
    fn self_referential_rebind(&self, line: &str) -> Option<(String, String)> {
        let mut parser = Parser::new(Lexer::new(line));
        let program = parser.parse_program();
        if !parser.errors.is_empty() || program.statements.len() != 1 {
            return None;
        }
        let Statement::Let { name, value, .. } = &program.statements[0] else {
            return None;
        };
        if !collect_free_vars(value).contains(name) {
            return None;
        }
        let name_str = parser.interner().resolve(*name).to_string();
        // Only an existing session binding qualifies: otherwise the reference is
        // genuinely unbound and should surface as such.
        let is_rebind = self
            .compiler
            .interner
            .lookup(&name_str)
            .is_some_and(|sym| self.compiler.symbol_table.exists_in_current_scope(sym));
        if !is_rebind {
            return None;
        }
        let span = value.span();
        let start = byte_offset(line, span.start)?;
        let end = byte_offset(line, span.end)?;
        let init = line.get(start..end)?.to_string();
        Some((name_str, init))
    }

    /// Evaluate a self-referential rebind by capturing the initializer's value
    /// while the previous `name` is still in scope, then rebinding `name` to that
    /// capture (proposal 0176). Two committed steps run in sequence: first
    /// `let __repl_prev_N = <init>`, whose `<init>` reads the old `name`; then
    /// `let name = __repl_prev_N`, which forgets the old `name` and rebinds it.
    /// The pair is atomic — a failure in either step restores the pre-attempt
    /// compiler so the session never half-applies the rebind — and the original
    /// line is what gets recorded for `:list`.
    fn eval_self_rebind(&mut self, original: &str, name: &str, init: &str) -> bool {
        let checkpoint = self.compiler.clone();
        let prev = format!("__repl_prev_{}", self.result_counter);
        self.result_counter += 1;

        let capture = format!("let {prev} = {init}");
        if !self.run_rebind_step(capture, &checkpoint) {
            return false;
        }
        let rebind = format!("let {name} = {prev}");
        if !self.run_rebind_step(rebind, &checkpoint) {
            return false;
        }
        self.commit(Some(name.to_string()), original.to_string());
        true
    }

    /// Run one synthetic step of a self-rebind, rolling the compiler all the way
    /// back to `checkpoint` (before step 1) on any failure so the pair is atomic.
    fn run_rebind_step(&mut self, source: String, checkpoint: &Compiler) -> bool {
        match self.run_line(source) {
            LineOutcome::Committed => true,
            LineOutcome::CompileFailed {
                source,
                diagnostics,
            } => {
                self.compiler = checkpoint.clone();
                self.render(&diagnostics, &source);
                false
            }
            LineOutcome::RuntimeFailed(err) => {
                self.compiler = checkpoint.clone();
                eprintln!("{err}");
                false
            }
        }
    }

    /// Detect a *named* top-level binding: a single `let name = init` whose `name`
    /// is a real binding (not the `_` wildcard). Returns `(name, init_source)` —
    /// the initializer sliced back out by span. Used only after the direct compile
    /// fails with E413, to persist the binding by capturing its effectful
    /// initializer's value. `None` for a destructure, an `fn`/other declaration, a
    /// `let _ = ...`, or anything that doesn't parse as exactly one `let`.
    fn named_let_binding(&self, line: &str) -> Option<(String, String)> {
        let mut parser = Parser::new(Lexer::new(line));
        let program = parser.parse_program();
        if !parser.errors.is_empty() || program.statements.len() != 1 {
            return None;
        }
        let Statement::Let { name, value, .. } = &program.statements[0] else {
            return None;
        };
        let name_str = parser.interner().resolve(*name).to_string();
        if name_str == "_" {
            return None;
        }
        let span = value.span();
        let start = byte_offset(line, span.start)?;
        let end = byte_offset(line, span.end)?;
        let init = line.get(start..end)?.to_string();
        Some((name_str, init))
    }

    /// Persist a *named* effectful binding (`let x = read_line()`). The direct
    /// compile failed with E413 (top-level effect), so run just the initializer
    /// inside `fn main() with IO` — the only context with the root IO handler — so
    /// the effect fires once and yields the value, then re-bind `let name =
    /// <literal>` purely so it persists without re-running the effect. A result
    /// with no faithful literal form (`Unit`/`None`, a map, a closure) can't
    /// persist: the effect still ran, so warn and move on.
    fn eval_effectful_named_binding(&mut self, name: &str, init: &str) -> bool {
        let main = format!("fn main() with IO {{\n    {init}\n}}\n");
        match self.run_line(main) {
            LineOutcome::Committed => {
                if let Some(literal) = value_as_literal(&self.vm.last_popped_stack_elem()) {
                    let binding = format!("let {name} = {literal}");
                    if let LineOutcome::Committed = self.run_line(binding.clone()) {
                        self.commit(Some(name.to_string()), binding);
                        return true;
                    }
                }
                eprintln!(
                    "note: `{name}` was not added to the session — its effectful \
                     result has no literal form to persist."
                );
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
    /// is effectful — and so can't be a top-level binding (E413) — fall back to
    /// running it inside `main`, where its result is still captured into `it`
    /// when it is a primitive value.
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
                // expression (E413). Re-run it inside `main` instead.
                if has_top_level_effect_error(&diagnostics) {
                    self.eval_effectful_expr(&resolved)
                } else {
                    self.render(&diagnostics, &source);
                    false
                }
            }
        }
    }

    /// Run an effectful expression inside `main` — the only context that carries
    /// the root IO handler — so its effect happens. `main` returns the
    /// expression's value; when that value round-trips as a literal (a
    /// `read_line()` String, a `now()` Int, a list / tuple / ADT of such, …),
    /// re-bind it so `it` captures it (proposal 0176). A result with no faithful
    /// literal form (`None`/Unit from `print(..)`, a map, a closure, …) runs its
    /// effect but is not captured — `it` is left unchanged.
    fn eval_effectful_expr(&mut self, resolved: &str) -> bool {
        let source = format!("fn main() with IO {{\n    {resolved}\n}}\n");
        match self.run_line(source) {
            LineOutcome::Committed => {
                if let Some(literal) = value_as_literal(&self.vm.last_popped_stack_elem()) {
                    self.capture_effectful_result(&literal);
                }
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

    /// Persist an effectful expression's already-computed result (rendered as a
    /// literal) into a fresh `__repl_N` session global and echo it, so `it`
    /// resolves to it next line. The literal re-bind is pure — it does not re-run
    /// the original effect. A failure here is non-fatal: the effect already ran,
    /// so we simply skip the capture.
    fn capture_effectful_result(&mut self, literal: &str) {
        let name = format!("__repl_{}", self.result_counter);
        let binding = format!("let {name} = {literal}");
        let pure = format!("{binding}\nfn main() with IO {{\n    println({name})\n}}\n");
        if let LineOutcome::Committed = self.run_line(pure) {
            self.commit(Some(name.clone()), binding);
            self.last_result = Some(name);
            self.result_counter += 1;
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

    /// In-scope identifier names for REPL tab completion, as the user actually
    /// types them. Unions the compiler's bare names (exposed library members,
    /// builtin primops, ADT constructors — see [`Compiler::repl_bare_names`]) with
    /// the session's own `let` / `fn` bindings, `it` (when a result exists), the
    /// language keywords, and the fully-qualified names (for `.`-prefix
    /// completion). The synthetic `__repl_*` result bindings are hidden — `it` is
    /// the handle for those. Sorted and de-duplicated.
    pub(super) fn completion_names(&self) -> Vec<String> {
        let mut names = self.compiler.repl_bare_names();
        names.extend(
            self.committed
                .iter()
                .filter_map(|decl| decl.name.clone())
                .filter(|name| !name.starts_with("__")),
        );
        if self.last_result.is_some() {
            names.push("it".to_string());
        }
        names.extend(
            crate::syntax::token_type::KEYWORDS
                .iter()
                .map(|kw| kw.to_string()),
        );
        names.extend(self.compiler.repl_qualified_names());
        names.sort_unstable();
        names.dedup();
        names
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

        // The line is committed: record any top-level `data` declarations so a
        // later line can construct / spread / access their record types
        // (proposal 0176). Only reached on success, so a failed line — already
        // rolled back above — never pollutes the session's ADT set.
        let data_decls = top_level_data_decls(&program);
        if !data_decls.is_empty() {
            self.compiler.accumulate_repl_session_data(data_decls);
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

/// Upper bound on the number of [`Value`] nodes rendered into one capture
/// literal. A bare effectful expression can evaluate to an arbitrarily large
/// structure (a long list, a deep tree); re-binding it as source means
/// re-parsing that source every subsequent line, so cap the rendered size and
/// fall back to "not captured" past the limit rather than emit a multi-megabyte
/// literal.
const MAX_CAPTURE_LITERAL_NODES: usize = 1024;

/// Render a [`Value`] as a Flux source literal that re-parses to the same value,
/// for persisting an effectful expression's result into the session without
/// re-running its effect. Handles primitives plus the compound shapes with a
/// faithful literal form — `Some`/`Left`/`Right`, tuples, arrays, proper lists,
/// and user ADT constructors (`Ctor(..)` / nullary `Ctor`, whose name the
/// accumulated `data` decls keep in session scope). `None` for values with no
/// faithful literal form: `None`/Unit (so a `print(..)` result doesn't pollute
/// `it`), maps, closures/continuations, non-finite floats, improper lists,
/// strings with an unescapable control char, or any structure exceeding
/// [`MAX_CAPTURE_LITERAL_NODES`].
fn value_as_literal(value: &Value) -> Option<String> {
    let mut budget = MAX_CAPTURE_LITERAL_NODES;
    value_as_literal_within(value, &mut budget)
}

/// Recursive worker for [`value_as_literal`], decrementing `budget` per node so
/// a pathologically large result bails to `None` instead of a huge literal.
fn value_as_literal_within(value: &Value, budget: &mut usize) -> Option<String> {
    *budget = budget.checked_sub(1)?;
    match value {
        Value::Integer(n) => Some(n.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        // Render with a decimal point and no exponent so it re-infers as `Float`
        // (not `Int`) and parses; skip forms Flux's lexer wouldn't accept.
        Value::Float(f) if f.is_finite() => {
            let rendered = format!("{f:?}");
            (rendered.contains('.') && !rendered.contains(['e', 'E'])).then_some(rendered)
        }
        Value::String(s) => string_literal(s),
        Value::EmptyList => Some("[]".to_string()),
        Value::Some(v) => Some(format!("Some({})", value_as_literal_within(v, budget)?)),
        Value::Left(v) => Some(format!("Left({})", value_as_literal_within(v, budget)?)),
        Value::Right(v) => Some(format!("Right({})", value_as_literal_within(v, budget)?)),
        // The empty tuple is Flux's `Unit` — the result of a statement-effect like
        // `print(..)`. Leave it uncaptured (like `None`) so those don't pollute `it`.
        Value::Tuple(elements) if elements.is_empty() => None,
        Value::Tuple(elements) => {
            let items = render_elements(elements, budget)?;
            match items.len() {
                // A 1-tuple needs the trailing comma to stay a tuple, not a group.
                1 => Some(format!("({},)", items[0])),
                _ => Some(format!("({})", items.join(", "))),
            }
        }
        Value::Array(elements) => {
            let items = render_elements(elements, budget)?;
            if items.is_empty() {
                Some("[||]".to_string())
            } else {
                Some(format!("[|{}|]", items.join(", ")))
            }
        }
        // Only a *proper* list (cons spine ending in `[]`) has a `[..]` literal;
        // an improper / `None`-terminated spine has none.
        Value::Cons(_) => {
            let mut items = Vec::new();
            let mut cursor = value;
            loop {
                match cursor {
                    Value::Cons(cell) => {
                        items.push(value_as_literal_within(&cell.head, budget)?);
                        cursor = &cell.tail;
                    }
                    Value::EmptyList => break,
                    _ => return None,
                }
            }
            Some(format!("[{}]", items.join(", ")))
        }
        Value::Adt(adt) => {
            if adt.fields.is_empty() {
                Some(adt.constructor.to_string())
            } else {
                let items: Option<Vec<String>> = adt
                    .fields
                    .iter()
                    .map(|field| value_as_literal_within(field, budget))
                    .collect();
                Some(format!("{}({})", adt.constructor, items?.join(", ")))
            }
        }
        Value::AdtUnit(name) => Some(name.to_string()),
        _ => None,
    }
}

/// Render every element of a tuple/array, threading the node `budget`. `None` if
/// any element has no literal form or the budget runs out.
fn render_elements(elements: &[Value], budget: &mut usize) -> Option<Vec<String>> {
    elements
        .iter()
        .map(|element| value_as_literal_within(element, budget))
        .collect()
}

/// A Flux double-quoted string literal for `s`, escaping the sequences Flux
/// recognises (`\ " newline tab CR #`). `None` if `s` holds a control character
/// with no Flux escape, so the caller skips the capture rather than emit a
/// literal that would fail to lex.
fn string_literal(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '#' => out.push_str("\\#"),
            c if c.is_control() => return None,
            c => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}

/// Byte offset of a 1-based-line / 0-based-column [`Position`] within `source`,
/// for slicing an AST node's source text back out (parser spans are line/column,
/// not byte offsets). Columns are counted per `char`, matching the lexer, and the
/// returned index always falls on a UTF-8 boundary. `None` if the position is
/// past the end of `source`.
fn byte_offset(source: &str, pos: crate::diagnostics::position::Position) -> Option<usize> {
    let mut line = 1usize;
    let mut col = 0usize;
    for (i, ch) in source.char_indices() {
        if line == pos.line && col == pos.column {
            return Some(i);
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line == pos.line && col == pos.column).then_some(source.len())
}

/// The top-level `data` declarations in a just-compiled line, cloned for the
/// session's accumulated ADT set (proposal 0176). Only top-level declarations
/// are taken — a `data` nested in a `module` keeps its module-qualified scope
/// and is resolved through the persistent compiler's normal module handling.
fn top_level_data_decls(program: &Program) -> Vec<Statement> {
    program
        .statements
        .iter()
        .filter(|stmt| matches!(stmt, Statement::Data { .. }))
        .cloned()
        .collect()
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

#[cfg(test)]
mod tests {
    use super::ReplEngine;
    use crate::driver::pipeline::program::RunProgramRequest;
    use crate::driver::{session::DriverSession, test_support::base_flags};

    /// Build a real prelude-loaded `ReplEngine` in-process, mirroring how
    /// `run_repl` wires up the session (cache off, quiet on, synthetic entry
    /// path so the cwd-relative `lib/Flow` prelude resolves).
    fn bootstrap_test_engine() -> ReplEngine {
        let mut flags = base_flags();
        flags.cache.no_cache = true;
        flags.runtime.quiet = true;
        let session = DriverSession::from(&flags);
        let mut path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        path.push("__flux_repl__.flx");
        let path = path.to_string_lossy().into_owned();
        let request = RunProgramRequest {
            path: &path,
            flags: &flags,
            session: &session,
        };
        ReplEngine::bootstrap(request).expect("bootstrap REPL session")
    }

    /// Proposal 0176 acceptance criterion: per-line work does not grow with
    /// session length — a function defined on line 1 and called on every later
    /// line must not be recompiled. The persistent compiler appends each line to
    /// one top-level instruction buffer and runs only the new tail, so the proof
    /// is structural: compiling a later line never mutates the bytes emitted for
    /// earlier lines (append-only ⇒ no recompilation).
    #[test]
    fn repl_does_not_recompile_earlier_lines() {
        let mut engine = bootstrap_test_engine();

        // Line 1: define a function the later lines all call.
        assert!(
            engine.eval_decl("fn double(n: Int) -> Int { n * 2 }"),
            "function definition should compile"
        );

        // Snapshot the instruction buffer after line 1.
        let baseline = engine.compiler.bytecode().instructions;
        let prefix = baseline.len();
        assert!(prefix > 0, "line 1 must emit some top-level instructions");

        // Each later line calls `double`. The line-1 prefix must stay
        // byte-identical (not recompiled) and the buffer must only grow.
        let mut prev_len = prefix;
        for i in 0..5 {
            assert!(engine.eval_expr("double(21)"), "call line {i} should run");
            let after = engine.compiler.bytecode().instructions;
            assert_eq!(
                &after[..prefix],
                &baseline[..],
                "line 1's instructions must be untouched on call line {i} \
                 (earlier lines are never recompiled)"
            );
            assert!(
                after.len() > prev_len,
                "the buffer must grow by appending line {i}, never shrink/rewrite"
            );
            prev_len = after.len();
        }
    }
}
