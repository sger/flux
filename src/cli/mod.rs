//! CLI entrypoints and argument parsing for the Flux executable.

use std::{ffi::OsString, process::ExitCode};

use crate::{
    cli::{
        cmdline::{CliCommand, parse_args},
        render::text::help_text,
    },
    driver::command::{cache, entry, inspect},
    parity::cli::run_parity_check,
};

pub mod cmdline;
pub mod render;
pub(crate) mod shared;

/// Parses CLI arguments, dispatches the selected command, and returns the process exit code.
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    entry::init();
    match parse_args(args) {
        Ok(command) => run_command(command),
        Err(message) => render_parse_error(&message),
    }
}

/// Dispatches a parsed CLI command to the corresponding driver entrypoint.
fn run_command(command: CliCommand) -> ExitCode {
    match command {
        CliCommand::Run { flags, target } => entry::run(flags, target),
        CliCommand::Tokens { flags } => inspect::show_tokens(&flags),
        CliCommand::Bytecode { flags } => inspect::show_bytecode(&flags),
        CliCommand::Lint { flags } => inspect::lint(&flags),
        CliCommand::Fmt { path, check } => inspect::fmt(&path, check),
        CliCommand::CacheInfo { flags } => cache::show_cache_info(&flags),
        CliCommand::ModuleCacheInfo { flags } => cache::show_module_cache_info(&flags),
        CliCommand::NativeCacheInfo { flags } => cache::show_native_cache_info(&flags),
        CliCommand::Clean { flags } => cache::clean(&flags),
        CliCommand::InterfaceInfo { flags } => cache::show_interface_info(&flags),
        CliCommand::AnalyzeFreeVars { flags } => inspect::analyze_free_vars(&flags),
        CliCommand::AnalyzeTailCalls { flags } => inspect::analyze_tail_calls(&flags),
        CliCommand::ParityCheck { raw_args } => run_parity_check(&raw_args),
        CliCommand::Help => show_help(),
    }
    ExitCode::SUCCESS
}

/// Prints the top-level CLI help text.
fn show_help() {
    print!("{}", help_text())
}

/// Prints a parse error and returns the CLI process exit code.
fn render_parse_error(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::SUCCESS
}
