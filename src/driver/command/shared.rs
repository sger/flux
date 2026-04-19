//! Shared helpers for driver commands.
//!
//! These helpers keep command modules focused on command-specific behavior instead of repeating
//! input-path validation, file loading, and parser diagnostic handling.

use std::fs;

use crate::{
    driver::{
        flags::DriverFlags,
        mode::DiagnosticOutputFormat,
        support::shared::{DiagnosticRenderRequest, emit_diagnostics},
    },
    syntax::{lexer::Lexer, parser::Parser, program::Program},
};

/// Lightweight parser command settings shared by command handlers that operate on one source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ParseCommandConfig {
    pub(crate) max_errors: usize,
    pub(crate) diagnostics_format: DiagnosticOutputFormat,
    pub(crate) show_file_headers: bool,
}

/// Parsed source file data returned to command handlers after syntax diagnostics are emitted.
pub(crate) struct ParsedCommandProgram {
    pub(crate) source: String,
    pub(crate) parser: Parser,
    pub(crate) program: Program,
}

/// Returns the required CLI input path or terminates the process with the given usage string.
pub(crate) fn require_input_path<'a>(flags: &'a DriverFlags, usage: &str) -> &'a str {
    flags.input.input_path.as_deref().unwrap_or_else(|| {
        eprintln!("{usage}");
        std::process::exit(1);
    })
}

/// Reads a source file for command processing and exits on IO failure.
pub(crate) fn read_command_source(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| {
        eprintln!("Error reading {path}: {err}");
        std::process::exit(1);
    })
}

/// Parses a single source file and emits syntax diagnostics before returning the parsed program.
pub(crate) fn parse_program_for_command(
    path: &str,
    config: ParseCommandConfig,
) -> ParsedCommandProgram {
    let source = read_command_source(path);
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    emit_parser_diagnostics(path, &source, &mut parser, config);
    ParsedCommandProgram {
        source,
        parser,
        program,
    }
}

/// Emits parser warnings and exits when syntax errors are present.
pub(crate) fn emit_parser_diagnostics(
    path: &str,
    source: &str,
    parser: &mut Parser,
    config: ParseCommandConfig,
) {
    let mut warnings = parser.take_warnings();
    for diag in &mut warnings {
        if diag.file().is_none() {
            diag.set_file(path.to_string());
        }
    }

    if !parser.errors.is_empty() {
        emit_diagnostics(DiagnosticRenderRequest {
            diagnostics: &parser.errors,
            default_file: Some(path),
            default_source: Some(source),
            show_file_headers: config.show_file_headers,
            max_errors: config.max_errors,
            format: config.diagnostics_format,
            all_errors: false,
            text_to_stderr: true,
        });
        std::process::exit(1);
    }

    if !warnings.is_empty() {
        emit_diagnostics(DiagnosticRenderRequest {
            diagnostics: &warnings,
            default_file: Some(path),
            default_source: Some(source),
            show_file_headers: config.show_file_headers,
            max_errors: config.max_errors,
            format: config.diagnostics_format,
            all_errors: false,
            text_to_stderr: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::ParseCommandConfig;
    use crate::driver::{DiagnosticOutputFormat, test_support::base_flags};

    #[test]
    fn parse_command_config_can_be_built_from_driver_flags() {
        let mut flags = base_flags();
        flags.diagnostics.max_errors = 42;
        flags.diagnostics.diagnostics_format = DiagnosticOutputFormat::JsonCompact;

        let config = ParseCommandConfig {
            max_errors: flags.diagnostics.max_errors,
            diagnostics_format: flags.diagnostics.diagnostics_format,
            show_file_headers: true,
        };

        assert_eq!(config.max_errors, 42);
        assert_eq!(
            config.diagnostics_format,
            DiagnosticOutputFormat::JsonCompact
        );
        assert!(config.show_file_headers);
    }
}
