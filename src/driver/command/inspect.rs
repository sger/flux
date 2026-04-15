//! Inspection-oriented driver commands.

use std::fs;

use crate::{
    ast::{collect_free_vars_in_program, find_tail_calls},
    bytecode::{compiler::Compiler, op_code::disassemble},
    driver::{
        command::shared::{
            ParseCommandConfig, emit_parser_diagnostics, parse_program_for_command,
            read_command_source, require_input_path,
        },
        flags::DriverFlags,
        mode::DiagnosticOutputFormat,
        support::shared::{DiagnosticRenderRequest, emit_diagnostics},
    },
    runtime::value::Value,
    syntax::{
        formatter::format_source, lexer::Lexer, linter::Linter, parser::Parser, program::Program,
    },
};

/// Lexes and prints the raw token stream for a source file.
pub fn show_tokens(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux tokens <file.flx>");
    let source = read_command_source(path);
    let mut lexer = Lexer::new(&source);
    println!("Tokens from {}:", path);
    println!("{}", "─".repeat(50));
    for tok in lexer.tokenize() {
        println!(
            "{:>3}:{:<3} {:12} {:?}",
            tok.position.line,
            tok.position.column,
            tok.token_type.to_string(),
            tok.literal
        );
    }
}

/// Compiles a source file and prints the produced bytecode plus nested function bytecode.
pub fn show_bytecode(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux bytecode <file.flx>");
    let mut parsed = parse_program_for_command(
        path,
        ParseCommandConfig {
            max_errors: flags.diagnostics.max_errors,
            diagnostics_format: flags.diagnostics.diagnostics_format,
            show_file_headers: false,
        },
    );
    let interner = parsed.parser.take_interner();
    let mut compiler = Compiler::new_with_interner(path, interner);
    compiler.set_strict_mode(flags.language.strict_mode);
    compiler.set_strict_inference(flags.language.strict_inference);
    let compile_result = compiler.compile_with_opts(
        &parsed.program,
        flags.language.enable_optimize,
        flags.language.enable_analyze,
    );
    let mut compiler_warnings = compiler.take_warnings();
    for diag in &mut compiler_warnings {
        if diag.file().is_none() {
            diag.set_file(path.to_string());
        }
    }
    if !compiler_warnings.is_empty() {
        emit_diagnostics(DiagnosticRenderRequest {
            diagnostics: &compiler_warnings,
            default_file: Some(path),
            default_source: Some(parsed.source.as_str()),
            show_file_headers: false,
            max_errors: flags.diagnostics.max_errors,
            format: flags.diagnostics.diagnostics_format,
            all_errors: false,
            text_to_stderr: true,
        });
    }
    if let Err(diags) = compile_result {
        emit_diagnostics(DiagnosticRenderRequest {
            diagnostics: &diags,
            default_file: Some(path),
            default_source: Some(parsed.source.as_str()),
            show_file_headers: false,
            max_errors: flags.diagnostics.max_errors,
            format: flags.diagnostics.diagnostics_format,
            all_errors: false,
            text_to_stderr: true,
        });
        std::process::exit(1);
    }
    let bytecode = compiler.bytecode();
    println!("Bytecode from {}:", path);
    println!("{}", "─".repeat(50));
    println!("Constants:");
    for (i, c) in bytecode.constants.iter().enumerate() {
        println!("  {}: {}", i, c);
    }
    println!("\nInstructions:");
    print!("{}", disassemble(&bytecode.instructions));
    for (i, c) in bytecode.constants.iter().enumerate() {
        if let Value::Function(f) = c {
            let name = f
                .debug_info
                .as_ref()
                .and_then(|d| d.name.as_deref())
                .unwrap_or("<anonymous>");
            println!("\nFunction <{}> (constant {}):", name, i);
            print!("{}", disassemble(&f.instructions));
        }
    }
}

/// Runs the linter for one source file after syntax diagnostics are emitted.
pub fn lint(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux lint <file.flx>");
    let mut parsed = parse_program_for_command(
        path,
        ParseCommandConfig {
            max_errors: flags.diagnostics.max_errors,
            diagnostics_format: flags.diagnostics.diagnostics_format,
            show_file_headers: false,
        },
    );
    let interner = parsed.parser.take_interner();
    let lints = Linter::new(Some(path.to_string()), &interner).lint(&parsed.program);
    if !lints.is_empty() {
        emit_diagnostics(DiagnosticRenderRequest {
            diagnostics: &lints,
            default_file: Some(path),
            default_source: Some(parsed.source.as_str()),
            show_file_headers: false,
            max_errors: flags.diagnostics.max_errors,
            format: flags.diagnostics.diagnostics_format,
            all_errors: false,
            text_to_stderr: false,
        });
    }
}

/// Formats one Flux source file in place or reports whether changes are needed.
pub fn fmt(path: &str, check: bool) {
    match fs::read_to_string(path) {
        Ok(source) => {
            let formatted = format_source(&source);
            if check {
                if source.trim() != formatted.trim() {
                    eprintln!("format: changes needed");
                    std::process::exit(1);
                }
                return;
            }
            if let Err(err) = fs::write(path, formatted) {
                eprintln!("Error writing {}: {}", path, err);
            }
        }
        Err(e) => eprintln!("Error reading {}: {}", path, e),
    }
}

/// Prints the free variables found in a parsed source file.
pub fn analyze_free_vars(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux analyze-free-vars <file.flx>");
    analyze_ast_command(
        path,
        flags.diagnostics.max_errors,
        flags.diagnostics.diagnostics_format,
        true,
        |source, mut parser, program| {
            let interner = parser.take_interner();
            let free_vars = collect_free_vars_in_program(&program);
            if free_vars.is_empty() {
                println!("✓ No free variables found in {}", path);
            } else {
                println!("Free variables in {}:", path);
                println!("{}", "─".repeat(50));
                let mut vars: Vec<_> = free_vars.iter().map(|sym| interner.resolve(*sym)).collect();
                vars.sort();
                for var in vars {
                    println!("  • {}", var);
                }
                println!("\nTotal: {} free variable(s)", free_vars.len());
                println!(
                    "\nℹ️  Free variables are identifiers that are referenced but not defined."
                );
                println!("   This may indicate undefined variables or missing imports.");
            }
            let _ = source;
        },
    );
}

/// Prints the tail-call sites found in a parsed source file.
pub fn analyze_tail_calls(flags: &DriverFlags) {
    let path = require_input_path(flags, "Usage: flux analyze-tail-calls <file.flx>");
    analyze_ast_command(
        path,
        flags.diagnostics.max_errors,
        flags.diagnostics.diagnostics_format,
        true,
        |source, _parser, program| {
            let tail_calls = find_tail_calls(&program);
            if tail_calls.is_empty() {
                println!("✓ No tail calls found in {}", path);
                println!(
                    "\nℹ️  Tail calls are function calls in tail position that can be optimized."
                );
            } else {
                println!("Tail calls in {}:", path);
                println!("{}", "─".repeat(50));
                let lines: Vec<_> = source.lines().collect();
                for (idx, call) in tail_calls.iter().enumerate() {
                    let line_num = call.span.start.line;
                    let line_text = if line_num > 0 && line_num <= lines.len() {
                        lines[line_num - 1].trim()
                    } else {
                        "<unknown>"
                    };
                    println!("  {}. Line {}: {}", idx + 1, line_num, line_text);
                }
                println!("\nTotal: {} tail call(s)", tail_calls.len());
                println!("\n✓ These calls are eligible for tail call optimization (TCO).");
                println!(
                    "  The Flux compiler automatically optimizes tail calls to avoid stack overflow."
                );
            }
        },
    );
}

/// Parses an AST-oriented command and forwards the parsed program to a command-specific closure.
fn analyze_ast_command<F>(
    path: &str,
    max_errors: usize,
    diagnostics_format: DiagnosticOutputFormat,
    show_file_headers: bool,
    on_success: F,
) where
    F: FnOnce(&str, Parser, Program),
{
    let source = read_command_source(path);
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    emit_parser_diagnostics(
        path,
        &source,
        &mut parser,
        ParseCommandConfig {
            max_errors,
            diagnostics_format,
            show_file_headers,
        },
    );
    on_success(&source, parser, program);
}
