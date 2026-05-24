//! `flux repl` — an interactive read-eval-print loop (proposal 0175, Phase 1).
//!
//! This is the **accumulate-source** engine: the session keeps a growing buffer
//! of the declarations entered so far, and each expression line re-runs the whole
//! buffer wrapped in a synthetic `fn main` through the normal VM pipeline. Earlier
//! definitions therefore stay in scope for later lines. The known costs —
//! re-executing side-effecting declarations and O(n^2) recompilation — are
//! documented in the proposal and removed by Phase 2 (proposal 0176).
//!
//! Everything that can be a local binding (`let` / `fn`, plus the `it` result)
//! lives inside `main`'s body, so an expression that performs `IO` can be bound
//! to `it`; only the top-level-only forms (`import` / `data` / `effect` / `class`
//! / `instance` / `module`) stay at file scope. A bad line is rolled back (its
//! buffer entry is only committed after a clean compile/run), so the session never
//! enters a broken state.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::driver::{
    flags::DriverFlags,
    pipeline::program::{RunProgramRequest, eval_source_for_repl},
    session::DriverSession,
};
use crate::syntax::{
    lexeme::Lexeme, lexer::Lexer, parser::Parser, statement::Statement, token_type::TokenType,
};

/// The accumulated session: declarations remembered across lines.
#[derive(Default)]
struct ReplSession {
    /// Top-level-only declarations (`import` / `data` / `effect` / `class` /
    /// `instance` / `module`), emitted at file scope.
    top_decls: Vec<String>,
    /// `let` / `fn` declarations and `let __repl_N = <expr>` result bindings,
    /// emitted inside `main`'s body in entry order.
    body: Vec<String>,
    /// Name of the most recent expression's result binding, which `it` resolves
    /// to. `None` until the first expression is evaluated.
    last_result: Option<String>,
    /// Monotonic counter for unique result names (`it` can't be re-`let`, so each
    /// result gets a fresh `__repl_N`).
    result_counter: usize,
}

/// How a single entered line should be handled.
enum Line {
    /// Blank or comment-only — ignore.
    Skip,
    /// A bare expression to evaluate, bind to `it`, and print.
    Expr(String),
    /// A top-level-only declaration.
    TopDecl(String),
    /// A `let`/`fn` (or other in-`main`) declaration.
    BodyDecl(String),
}

/// Entry point for `flux repl`. Reads lines from stdin until EOF (or `:quit`).
pub fn run_repl(mut flags: DriverFlags) {
    // Each line recompiles the synthetic entry against the session buffer; like
    // `flux eval`, caching against a phantom entry path is pointless and mixing a
    // cached prelude with the fresh entry mis-resolves global slots, so disable it.
    flags.cache.no_cache = true;
    let path = synthetic_repl_path().to_string_lossy().into_owned();
    let session_cfg = DriverSession::from(&flags);

    print_banner();

    let mut repl = ReplSession::default();
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    while let Some(input) = read_input(&mut reader) {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(command) = trimmed.strip_prefix(':') {
            if handle_command(command, &mut repl) {
                break;
            }
            continue;
        }
        repl.eval_line(&input, &path, &flags, &session_cfg);
    }
    eprintln!("Goodbye.");
}

impl ReplSession {
    /// Classify, evaluate, and (on success) commit one non-command line.
    fn eval_line(&mut self, line: &str, path: &str, flags: &DriverFlags, cfg: &DriverSession) {
        match classify(line) {
            Line::Skip => {}
            Line::Expr(src) => {
                // Resolve `it` to the previous result's binding, then bind this
                // result under a fresh unique name (Flux forbids re-`let it`).
                let resolved = match &self.last_result {
                    Some(prev) => rewrite_it(&src, prev),
                    None => src,
                };
                let name = format!("__repl_{}", self.result_counter);
                let binding = format!("let {name} = {resolved}");
                let candidate = assemble(
                    &self.top_decls,
                    &self.body,
                    &[binding.clone(), format!("println({name})")],
                );
                if eval(path, flags, cfg, candidate, true) {
                    self.body.push(binding);
                    self.last_result = Some(name);
                    self.result_counter += 1;
                }
            }
            Line::BodyDecl(src) => {
                let candidate = assemble(&self.top_decls, &self.body, std::slice::from_ref(&src));
                if eval(path, flags, cfg, candidate, false) {
                    self.body.push(src);
                }
            }
            Line::TopDecl(src) => {
                let mut top = self.top_decls.clone();
                top.push(src.clone());
                let candidate = assemble(&top, &self.body, &[]);
                if eval(path, flags, cfg, candidate, false) {
                    self.top_decls.push(src);
                }
            }
        }
    }
}

/// Run one assembled candidate through the non-exiting REPL evaluator.
fn eval(path: &str, flags: &DriverFlags, cfg: &DriverSession, source: String, run: bool) -> bool {
    eval_source_for_repl(
        RunProgramRequest {
            path,
            flags,
            session: cfg,
        },
        source,
        run,
    )
}

/// Assemble a runnable program from the session buffers plus `trailing`
/// statements (the `let it = …` / `println(it)` for an expression, or the
/// candidate declaration being validated). In-`main` statements are indented one
/// level; multi-line statements keep their internal structure.
fn assemble(top: &[String], body: &[String], trailing: &[String]) -> String {
    let mut out = String::new();
    for decl in top {
        out.push_str(decl);
        out.push('\n');
    }
    if !top.is_empty() {
        out.push('\n');
    }
    out.push_str("fn main() with IO {\n");
    for stmt in body.iter().chain(trailing) {
        for line in stmt.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("}\n");
    out
}

/// Replace standalone `it` identifier tokens in `expr` with `replacement` (the
/// latest result binding's name). The committed form therefore never contains
/// `it`, which both resolves `it` to the previous result and avoids re-binding
/// the `it` name (Flux forbids two `let it` in one scope). Token-aware via the
/// lexer, so `it` inside strings/comments or within a longer identifier (`item`)
/// is left untouched.
fn rewrite_it(expr: &str, replacement: &str) -> String {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for token in Lexer::new(expr).tokenize() {
        if token.token_type == TokenType::Ident
            && token.literal.as_str() == "it"
            && let Lexeme::Span { span, .. } = &token.literal
        {
            spans.push((span.start, span.end));
        }
    }
    if spans.is_empty() {
        return expr.to_string();
    }
    let mut out = expr.to_string();
    // Splice from the end so earlier byte offsets stay valid.
    for (start, end) in spans.into_iter().rev() {
        out.replace_range(start..end, replacement);
    }
    out
}

/// Classify a (complete) input line by parsing it. Routing only — compile/type
/// errors are surfaced later by the evaluator, so a parse error here falls back
/// to "treat as an expression" so the real diagnostic is shown in context.
fn classify(line: &str) -> Line {
    let trimmed = line.trim();
    let mut parser = Parser::new(Lexer::new(trimmed));
    let program = parser.parse_program();
    if program.statements.is_empty() {
        // No statements + no errors ⇒ blank or comment-only; otherwise a parse
        // error we let the evaluator report.
        return if parser.errors.is_empty() {
            Line::Skip
        } else {
            Line::Expr(trimmed.to_string())
        };
    }
    match &program.statements[0] {
        Statement::Expression { .. } if program.statements.len() == 1 => {
            Line::Expr(trimmed.to_string())
        }
        Statement::Import { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. }
        | Statement::EffectAlias { .. }
        | Statement::Class { .. }
        | Statement::Instance { .. }
        | Statement::Module { .. } => Line::TopDecl(trimmed.to_string()),
        _ => Line::BodyDecl(trimmed.to_string()),
    }
}

/// Read one logical input (possibly spanning several physical lines when a form
/// is left open). Returns `None` at EOF. The prompt continues (`....>`) while the
/// delimiter balance is open or a string/comment is unterminated.
fn read_input(reader: &mut impl BufRead) -> Option<String> {
    let mut buffer = String::new();
    prompt(false);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                return if buffer.trim().is_empty() {
                    None
                } else {
                    Some(buffer)
                };
            }
            Ok(_) => buffer.push_str(&line),
            Err(_) => return None,
        }
        if !needs_more_input(&buffer) {
            return Some(buffer);
        }
        prompt(true);
    }
}

/// Whether the accumulated input is an incomplete form that should keep reading:
/// an open `(`/`[`/`{` delimiter, or an unterminated string / block comment. Uses
/// the lexer so delimiters inside strings and comments don't miscount.
fn needs_more_input(src: &str) -> bool {
    let mut depth: i32 = 0;
    for token in Lexer::new(src).tokenize() {
        match token.token_type {
            TokenType::LParen | TokenType::LBracket | TokenType::LBrace => depth += 1,
            TokenType::RParen | TokenType::RBracket | TokenType::RBrace => depth -= 1,
            TokenType::UnterminatedString | TokenType::UnterminatedBlockComment => return true,
            _ => {}
        }
    }
    depth > 0
}

/// Handle a `:`-prefixed meta command. Returns `true` when the REPL should quit.
fn handle_command(command: &str, session: &mut ReplSession) -> bool {
    let name = command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| !c.is_alphanumeric());
    match name {
        "q" | "quit" => return true,
        "help" | "?" => print_help(),
        "reset" => {
            session.top_decls.clear();
            session.body.clear();
            session.last_result = None;
            session.result_counter = 0;
            eprintln!("Session reset.");
        }
        "list" | "l" => print_listing(session),
        "type" | "t" => {
            eprintln!(":type is not yet implemented (coming in a follow-up).");
        }
        other => eprintln!("Unknown command `:{other}`. Type :help for the list."),
    }
    false
}

fn print_banner() {
    eprintln!("Flux REPL — type :help for commands, :quit to exit.");
}

fn print_help() {
    eprintln!("Commands:");
    eprintln!("  :type <expr>  Show the inferred type of <expr> (not yet implemented)");
    eprintln!("  :reset        Forget all session bindings");
    eprintln!("  :list         Show the accumulated session source");
    eprintln!("  :help, :?     Show this help");
    eprintln!("  :quit, :q     Exit");
}

fn print_listing(session: &ReplSession) {
    if session.top_decls.is_empty() && session.body.is_empty() {
        eprintln!("(session is empty)");
        return;
    }
    for decl in &session.top_decls {
        eprintln!("{decl}");
    }
    for stmt in &session.body {
        eprintln!("{stmt}");
    }
}

/// Print the prompt to stderr (so stdout carries only evaluation output) and
/// flush, since prompts have no trailing newline.
fn prompt(continuation: bool) {
    eprint!("{}", if continuation { "....> " } else { "flux> " });
    let _ = io::stderr().flush();
}

/// A synthetic entry path under the current directory, mirroring `flux eval`: it
/// never needs to exist on disk and is used only for module-root discovery (so a
/// cwd-relative `lib/Flow` prelude resolves) and diagnostic file tagging.
fn synthetic_repl_path() -> PathBuf {
    let mut path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    path.push("__flux_repl__.flx");
    path
}

#[cfg(test)]
mod tests {
    use super::{Line, assemble, classify, needs_more_input, rewrite_it};

    #[test]
    fn rewrite_it_replaces_only_standalone_it_identifiers() {
        assert_eq!(rewrite_it("it + 1", "__repl_0"), "__repl_0 + 1");
        assert_eq!(rewrite_it("it + it", "__repl_2"), "__repl_2 + __repl_2");
        assert_eq!(rewrite_it("1 + 2", "__repl_0"), "1 + 2");
        // `it` inside a string, or as part of a longer identifier, is untouched.
        assert_eq!(rewrite_it("\"it\"", "__repl_0"), "\"it\"");
        assert_eq!(rewrite_it("item + 1", "__repl_0"), "item + 1");
    }

    #[test]
    fn classifies_expression_top_decl_and_body_decl() {
        assert!(matches!(classify("1 + 2"), Line::Expr(_)));
        assert!(matches!(classify("let xs = [1, 2, 3]"), Line::BodyDecl(_)));
        assert!(matches!(classify("fn f() -> Int { 1 }"), Line::BodyDecl(_)));
        assert!(matches!(classify("import Flow.List"), Line::TopDecl(_)));
        assert!(matches!(
            classify("data Color { Red, Green }"),
            Line::TopDecl(_)
        ));
        assert!(matches!(classify(""), Line::Skip));
        assert!(matches!(classify("// a comment"), Line::Skip));
    }

    #[test]
    fn needs_more_input_tracks_open_delimiters_and_strings() {
        assert!(!needs_more_input("1 + 2"));
        assert!(needs_more_input("fn f() {"));
        assert!(!needs_more_input("fn f() { 1 }"));
        assert!(needs_more_input("[1, 2,"));
        assert!(needs_more_input("\"unterminated"));
    }

    #[test]
    fn assemble_wraps_body_in_main_and_keeps_top_decls_at_file_scope() {
        let program = assemble(
            &["import Flow.List".to_string()],
            &["let x = 1".to_string()],
            &["let it = x + 1".to_string(), "println(it)".to_string()],
        );
        assert!(program.starts_with("import Flow.List\n\nfn main() with IO {\n"));
        assert!(program.contains("    let x = 1\n"));
        assert!(program.contains("    let it = x + 1\n"));
        assert!(program.contains("    println(it)\n"));
        assert!(program.trim_end().ends_with('}'));
    }
}
