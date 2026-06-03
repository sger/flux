//! `flux repl` — an interactive read-eval-print loop (proposals 0175 / 0176).
//!
//! This is the **persistent** engine (Phase 2): the session keeps one
//! prelude-loaded compiler and one live VM for its whole lifetime, and each
//! entered line compiles to a *delta* whose freshly-appended tail runs on the
//! live VM via [`VM::run_top_level`](crate::vm::VM::run_top_level). Earlier
//! declarations therefore
//! never recompile and their side effects never re-fire — the O(n²) recompile
//! and re-execution costs of Phase 1 are gone. See [`engine`] for the mechanism.
//!
//! Everything the user enters that is a declaration (`let` / `fn` / `data` /
//! `import` / `effect` / `class` / `instance` / `module` / `alias`) compiles at
//! file scope so it persists as a session global. A bare expression is bound to
//! a fresh `__repl_N` global and printed (so `it` references the value); an
//! effectful expression runs inside `main` without capturing `it`. A line that
//! fails to compile or run is rolled back, so the session never breaks.

mod browse;
mod completion;
mod engine;
mod info;

use std::cell::RefCell;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::driver::{
    flags::DriverFlags, pipeline::program::RunProgramRequest, session::DriverSession,
};
use crate::syntax::{
    lexeme::Lexeme, lexer::Lexer, parser::Parser, statement::Statement, token_type::TokenType,
};

use completion::ReplHelper;
use engine::ReplEngine;

/// The interactive editor, parameterised by the REPL's completion helper.
type ReplEditor = rustyline::Editor<ReplHelper, rustyline::history::FileHistory>;

/// How a single entered line should be routed.
enum LineKind {
    /// Blank or comment-only — ignore.
    Skip,
    /// A bare expression to evaluate, bind to `it`, and print.
    Expr,
    /// A top-level declaration (everything that isn't a bare expression).
    Decl,
}

/// What the dispatch loop should do after a `:`-command.
enum Command {
    /// Keep going.
    Handled,
    /// Re-bootstrap a fresh session.
    Reset,
    /// Exit the REPL.
    Quit,
}

/// Entry point for `flux repl`. Reads lines from stdin until EOF (or `:quit`).
pub fn run_repl(mut flags: DriverFlags) {
    // The prelude is compiled once at bootstrap and never recompiled, so module
    // caching against a phantom entry path buys nothing; disable it. The per-
    // module `[n of m] Compiling …` progress chatter is also noise here.
    flags.cache.no_cache = true;
    flags.runtime.quiet = true;
    let path = synthetic_repl_path().to_string_lossy().into_owned();
    let session = DriverSession::from(&flags);
    let request = RunProgramRequest {
        path: &path,
        flags: &flags,
        session: &session,
    };

    let engine = match ReplEngine::bootstrap(request) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("{err}");
            return;
        }
    };

    print_banner();
    // An interactive terminal gets the full rustyline editor (history, in-line
    // editing, Ctrl-R search); piped/redirected input (scripts, the integration
    // tests) takes the plain line-reader path, which keeps stdout clean and the
    // session deterministic.
    if io::stdin().is_terminal() {
        run_interactive(engine, request);
    } else {
        run_piped(engine, request);
    }
}

/// The piped / non-interactive loop: read complete logical inputs from stdin
/// (prompts go to stderr) until EOF or `:quit`.
fn run_piped(mut engine: ReplEngine, request: RunProgramRequest<'_>) {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    while let Some(input) = read_input(&mut reader) {
        if !dispatch(&input, &mut engine, request) {
            break;
        }
    }
    eprintln!("Goodbye.");
}

/// The interactive loop, backed by `rustyline`: arrow-key history, in-line
/// editing, and Ctrl-R reverse search, with history persisted across sessions.
/// Falls back to the piped loop if the editor can't be initialised.
fn run_interactive(mut engine: ReplEngine, request: RunProgramRequest<'_>) {
    // Shared, loop-updated in-scope name set for tab completion. The helper reads
    // it; the loop refreshes it after each entered line (below).
    let names = Rc::new(RefCell::new(engine.completion_names()));
    // `Circular` (menu-complete): the first Tab inserts a match inline and each
    // further Tab cycles to the next (Shift-Tab backward). Chosen over `List`
    // because List requires a *second* Tab to draw the candidate list whenever the
    // typed word is already the longest common prefix (e.g. `ma` for map/match/max),
    // which feels unresponsive.
    let config = rustyline::Config::builder()
        .completion_type(rustyline::CompletionType::Circular)
        .build();
    let mut editor: ReplEditor = match rustyline::Editor::with_config(config) {
        Ok(editor) => editor,
        Err(err) => {
            eprintln!("line editor unavailable ({err}); using basic input.");
            return run_piped(engine, request);
        }
    };
    editor.set_helper(Some(ReplHelper::new(Rc::clone(&names))));
    let history = history_path();
    if let Some(path) = &history {
        // A missing history file on first run is not an error.
        let _ = editor.load_history(path);
    }

    loop {
        match read_interactive_input(&mut editor) {
            // A completed input: record it in history (one entry even when it
            // spanned several physical lines), then dispatch it.
            Ok(Some(input)) => {
                let entry = input.trim_end();
                if !entry.is_empty() {
                    let _ = editor.add_history_entry(entry);
                }
                if !dispatch(&input, &mut engine, request) {
                    break;
                }
                // Newly-defined (or :load/:reset-replaced) bindings complete next.
                *names.borrow_mut() = engine.completion_names();
            }
            // Ctrl-C: abandon the current (possibly multi-line) input, keep going.
            Ok(None) => continue,
            // Ctrl-D / EOF / read error: leave the session.
            Err(_) => break,
        }
    }

    if let Some(path) = &history {
        let _ = editor.save_history(path);
    }
    eprintln!("Goodbye.");
}

/// Read one complete logical input through the editor, continuing onto `....>`
/// while the form is open (same delimiter/string balance as the piped path).
/// `Ok(None)` means Ctrl-C (abandon this input); `Err` means Ctrl-D/EOF.
fn read_interactive_input(
    editor: &mut ReplEditor,
) -> Result<Option<String>, rustyline::error::ReadlineError> {
    use rustyline::error::ReadlineError;

    let mut buffer = match editor.readline("flux> ") {
        Ok(line) => line,
        Err(ReadlineError::Interrupted) => return Ok(None),
        Err(err) => return Err(err),
    };
    while needs_more_input(&buffer) {
        match editor.readline("....> ") {
            Ok(line) => {
                buffer.push('\n');
                buffer.push_str(&line);
            }
            Err(ReadlineError::Interrupted) => return Ok(None),
            Err(err) => return Err(err),
        }
    }
    Ok(Some(buffer))
}

/// Process one complete logical input. Returns `false` when the loop should stop
/// (`:quit`). Shared by the interactive and piped paths.
fn dispatch(input: &str, engine: &mut ReplEngine, request: RunProgramRequest<'_>) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return true;
    }
    if let Some(command) = trimmed.strip_prefix(':') {
        match handle_command(command, engine, request) {
            Command::Quit => return false,
            Command::Reset => match ReplEngine::bootstrap(request) {
                Ok(fresh) => {
                    *engine = fresh;
                    eprintln!("Session reset.");
                }
                Err(err) => eprintln!("reset failed: {err}"),
            },
            Command::Handled => {}
        }
        return true;
    }
    match classify(trimmed) {
        LineKind::Skip => {}
        LineKind::Expr => {
            engine.eval_expr(trimmed);
        }
        LineKind::Decl => {
            engine.eval_decl(trimmed);
        }
    }
    true
}

/// Best-effort path for the persisted REPL history file (`~/.flux_repl_history`).
/// `None` when no home directory is discoverable — history then stays in-memory.
fn history_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let mut path = PathBuf::from(home);
    path.push(".flux_repl_history");
    Some(path)
}

/// Handle a `:`-prefixed meta command (the text after the `:`).
fn handle_command(
    command: &str,
    engine: &mut ReplEngine,
    request: RunProgramRequest<'_>,
) -> Command {
    let mut parts = command.trim().splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match name {
        "q" | "quit" => return Command::Quit,
        "reset" => return Command::Reset,
        "help" | "?" => print_help(),
        "list" | "l" => print_listing(engine),
        "type" | "t" => show_type(engine, rest, request),
        "info" | "i" => show_info(engine, rest, request),
        "browse" | "b" => show_browse(engine, rest),
        "load" => load_command(engine, rest, request),
        "reload" => reload_command(engine, request),
        other => eprintln!("Unknown command `:{other}`. Type :help for the list."),
    }
    Command::Handled
}

/// `:load <file>` — reset to a fresh prelude-loaded session, then load and run a
/// `.flx` file, keeping its top-level definitions in scope. The live session is
/// replaced only on success, so a failed load leaves the current session intact.
fn load_command(engine: &mut ReplEngine, arg: &str, request: RunProgramRequest<'_>) {
    let path = arg.trim().trim_matches(|c| c == '"' || c == '\'');
    if path.is_empty() {
        eprintln!("usage: :load <file>");
        return;
    }
    let mut fresh = match ReplEngine::bootstrap(request) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("load failed: {err}");
            return;
        }
    };
    if let Some(count) = fresh.load_file(Path::new(path)) {
        *engine = fresh;
        eprintln!("Loaded {path} ({count} definition{}).", plural(count));
    }
    // On failure the diagnostics were printed by `load_file`; keep the old session.
}

/// `:reload` — re-bootstrap a fresh session and re-load the file from the last
/// `:load`, picking up edits on disk. Keeps the previous session if reload fails.
fn reload_command(engine: &mut ReplEngine, request: RunProgramRequest<'_>) {
    let Some(path) = engine.loaded_path().cloned() else {
        eprintln!("Nothing to reload. Use :load <file> first.");
        return;
    };
    let mut fresh = match ReplEngine::bootstrap(request) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("reload failed: {err}");
            return;
        }
    };
    if let Some(count) = fresh.load_file(&path) {
        *engine = fresh;
        eprintln!(
            "Reloaded {} ({count} definition{}).",
            path.display(),
            plural(count)
        );
    }
}

/// Plural suffix for human-friendly counts: `""` for one, `"s"` otherwise.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Infer and print the type of `expr` in the current session, without evaluating
/// it. `it` resolves to the most recent result.
fn show_type(engine: &ReplEngine, expr: &str, request: RunProgramRequest<'_>) {
    if expr.is_empty() {
        eprintln!("usage: :type <expr>");
        return;
    }
    if let Some(ty) = engine.infer_type(request, expr) {
        println!("{expr} : {ty}");
    }
}

/// Show richer information about a single `name` than `:type`: its kind plus, as
/// applicable, constructors (types), operations (effects), the owning ADT
/// (constructors), or the defining origin (values). Unlike `:type`, this is keyed
/// on a name, not an arbitrary expression.
fn show_info(engine: &ReplEngine, name: &str, request: RunProgramRequest<'_>) {
    if name.is_empty() {
        eprintln!("usage: :info <name>");
        return;
    }
    if let Some(text) = engine.info(request, name) {
        println!("{text}");
    }
}

/// List every in-scope value binding with its type — the session's own
/// definitions and the auto-exposed prelude — optionally filtered to a name
/// prefix. Unlike `:list`, which echoes session declaration sources, this pairs
/// names with their inferred types.
fn show_browse(engine: &ReplEngine, filter: &str) {
    println!("{}", engine.browse(filter));
}

/// Classify a (complete) input line by parsing it. Routing only — compile/type
/// errors are surfaced later by the engine, so a parse error here falls back to
/// "treat as an expression" so the real diagnostic is shown in context.
fn classify(line: &str) -> LineKind {
    let trimmed = line.trim();
    let mut parser = Parser::new(Lexer::new(trimmed));
    let program = parser.parse_program();
    if program.statements.is_empty() {
        // No statements + no errors ⇒ blank or comment-only; otherwise a parse
        // error we let the engine report.
        return if parser.errors.is_empty() {
            LineKind::Skip
        } else {
            LineKind::Expr
        };
    }
    match &program.statements[0] {
        Statement::Expression { .. } if program.statements.len() == 1 => LineKind::Expr,
        _ => LineKind::Decl,
    }
}

/// Replace standalone `it` identifier tokens in `expr` with `replacement` (the
/// latest result binding's name). Token-aware via the lexer, so `it` inside
/// strings/comments or within a longer identifier (`item`) is left untouched.
/// Shared with [`engine`], which resolves `it` before compiling a line.
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

fn print_banner() {
    eprintln!("Flux REPL — type :help for commands, :quit to exit.");
}

fn print_help() {
    eprintln!("Commands:");
    eprintln!("  :type <expr>  Show the inferred type of <expr> (does not evaluate)");
    eprintln!("  :info <name>  Show a name's kind: type constructors, effect operations,");
    eprintln!("  :i <name>       a constructor's ADT, or a value's type and origin");
    eprintln!("  :browse [pfx]  List in-scope names with their types (session + prelude)");
    eprintln!("  :b [pfx]        optionally filtered to a name prefix");
    eprintln!("  :load <file>  Reset the session and load a .flx file's definitions");
    eprintln!("  :reload       Reload the last :load'd file from disk");
    eprintln!("  :reset        Forget all session bindings");
    eprintln!("  :list         Show the accumulated session declarations");
    eprintln!("  :help, :?     Show this help");
    eprintln!("  :quit, :q     Exit");
}

fn print_listing(engine: &ReplEngine) {
    let decls = engine.listing();
    if decls.is_empty() {
        eprintln!("(session is empty)");
        return;
    }
    for decl in decls {
        eprintln!("{decl}");
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
    use super::{LineKind, classify, needs_more_input, rewrite_it};

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
    fn classifies_expression_and_declarations() {
        assert!(matches!(classify("1 + 2"), LineKind::Expr));
        assert!(matches!(classify("let xs = [1, 2, 3]"), LineKind::Decl));
        assert!(matches!(classify("fn f() -> Int { 1 }"), LineKind::Decl));
        assert!(matches!(classify("import Flow.List"), LineKind::Decl));
        assert!(matches!(
            classify("data Color { Red, Green }"),
            LineKind::Decl
        ));
        assert!(matches!(classify(""), LineKind::Skip));
        assert!(matches!(classify("// a comment"), LineKind::Skip));
    }

    #[test]
    fn needs_more_input_tracks_open_delimiters_and_strings() {
        assert!(!needs_more_input("1 + 2"));
        assert!(needs_more_input("fn f() {"));
        assert!(!needs_more_input("fn f() { 1 }"));
        assert!(needs_more_input("[1, 2,"));
        assert!(needs_more_input("\"unterminated"));
    }
}
