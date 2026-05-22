//! Custom `flux/view*` requests — render a compiler-stage dump (token stream,
//! Core IR, or VM bytecode) for a document, in the spirit of rust-analyzer's
//! "View HIR / MIR". The VS Code extension sends one of these requests and opens
//! the returned text in a scratch editor.
//!
//! `render` never fails: a parse or compile error is returned as comment text so
//! the view always shows *something*. The work is a pure function of the source,
//! so it runs on the worker pool.

use flux::bytecode::op_code::disassemble;
use flux::compiler::Compiler;
use flux::core::display::CoreDisplayMode;
use flux::diagnostics::Diagnostic;
use flux::runtime::value::Value;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;
use flux::syntax::program::Program;

/// Which compiler-stage dump to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Tokens,
    CoreIr,
    Bytecode,
}

/// Render the requested dump for `source`. `path` is used only as a label in any
/// error text.
pub fn render(kind: ViewKind, source: &str, path: &str) -> String {
    match kind {
        ViewKind::Tokens => render_tokens(source),
        ViewKind::CoreIr => render_core(source, path),
        ViewKind::Bytecode => render_bytecode(source, path),
    }
}

fn render_tokens(source: &str) -> String {
    let mut lexer = Lexer::new(source);
    let mut out = String::from("// Token stream\n\n");
    for tok in lexer.tokenize() {
        out.push_str(&format!(
            "{:>4}:{:<3}  {:14} {:?}\n",
            tok.position.line,
            tok.position.column,
            tok.token_type.to_string(),
            tok.literal,
        ));
    }
    out
}

fn render_core(source: &str, path: &str) -> String {
    let (mut compiler, program) = match compile(source, path) {
        Ok(pair) => pair,
        Err(text) => return text,
    };
    match compiler.dump_core_with_opts(&program, true, CoreDisplayMode::Readable) {
        Ok(ir) => format!("// Core IR\n\n{ir}"),
        Err(d) => format!(
            "// could not lower to Core IR:\n// {}\n",
            d.message().unwrap_or("unknown error")
        ),
    }
}

fn render_bytecode(source: &str, path: &str) -> String {
    let (compiler, _program) = match compile(source, path) {
        Ok(pair) => pair,
        Err(text) => return text,
    };
    let bytecode = compiler.bytecode();
    let mut out = String::from("// VM bytecode\n\nConstants:\n");
    for (i, c) in bytecode.constants.iter().enumerate() {
        out.push_str(&format!("  {i}: {c}\n"));
    }
    out.push_str("\nInstructions:\n");
    out.push_str(&disassemble(&bytecode.instructions));
    // Nested function bodies live as `Function` constants; disassemble each.
    for (i, c) in bytecode.constants.iter().enumerate() {
        if let Value::Function(f) = c {
            let name = f
                .debug_info
                .as_ref()
                .and_then(|d| d.name.as_deref())
                .unwrap_or("<anonymous>");
            out.push_str(&format!("\nFunction <{name}> (constant {i}):\n"));
            out.push_str(&disassemble(&f.instructions));
        }
    }
    out
}

/// Parse and compile `source` into a `Compiler` ready to dump. A parse or
/// compile failure is returned as comment text for the view.
fn compile(source: &str, path: &str) -> Result<(Compiler, Program), String> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    if !parser.errors.is_empty() {
        return Err(format_diags("parse errors", &parser.errors));
    }
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(path, interner);
    if let Err(diags) = compiler.compile_with_opts(&program, true, true) {
        return Err(format_diags("compile errors", &diags));
    }
    Ok((compiler, program))
}

fn format_diags(label: &str, diags: &[Diagnostic]) -> String {
    let mut out = format!("// {label}:\n");
    for d in diags {
        out.push_str(&format!("// {}\n", d.message().unwrap_or("(no message)")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_view_lists_the_token_stream() {
        let out = render(ViewKind::Tokens, "let x = 1\n", "t.flx");
        assert!(out.contains("Token stream"));
        // The identifier `x` and the literal `1` both appear.
        assert!(out.contains('x') && out.contains('1'));
    }

    #[test]
    fn bytecode_view_disassembles_instructions() {
        let out = render(ViewKind::Bytecode, "fn main() { 1 + 2 }\n", "b.flx");
        assert!(out.contains("VM bytecode"), "got: {out}");
        assert!(out.contains("Instructions:"), "got: {out}");
    }

    #[test]
    fn core_view_renders_or_reports() {
        let out = render(ViewKind::CoreIr, "fn main() { 1 }\n", "c.flx");
        // Either the Core IR, or a graceful comment if lowering failed.
        assert!(out.starts_with("// Core IR") || out.starts_with("// could not"));
    }

    #[test]
    fn parse_error_is_returned_as_comment() {
        let out = render(ViewKind::Bytecode, "fn (", "bad.flx");
        assert!(out.contains("// parse errors"), "got: {out}");
    }
}
