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
pub mod package;
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
        CliCommand::Eval { expr, flags } => entry::eval(&expr, flags),
        CliCommand::Repl { flags } => entry::repl(flags),
        // These report a missing input rather than exiting in-process, so the
        // exit code is decided here (KI-019).
        CliCommand::CacheInfo { flags } => return exit_status(cache::show_cache_info(&flags)),
        CliCommand::ModuleCacheInfo { flags } => {
            return exit_status(cache::show_module_cache_info(&flags));
        }
        CliCommand::NativeCacheInfo { flags } => {
            return exit_status(cache::show_native_cache_info(&flags));
        }
        CliCommand::Clean { flags } => cache::clean(&flags),
        CliCommand::InterfaceInfo { flags } => cache::show_interface_info(&flags),
        CliCommand::AnalyzeFreeVars { flags } => inspect::analyze_free_vars(&flags),
        CliCommand::AnalyzeTailCalls { flags } => inspect::analyze_tail_calls(&flags),
        CliCommand::ParityCheck { raw_args } => run_parity_check(&raw_args),
        CliCommand::Init { name, is_lib } => return package::init(name.as_deref(), is_lib),
        CliCommand::New { name, is_lib } => return package::new(&name, is_lib),
        CliCommand::Package {
            action,
            flags,
            bin,
            program_args,
        } => return package::package_command(action, flags, bin.as_deref(), program_args),
        CliCommand::Help => show_help(),
    }
    ExitCode::SUCCESS
}

/// Turns a command's success flag into a process exit code.
fn exit_status(ok: bool) -> ExitCode {
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Prints the top-level CLI help text.
fn show_help() {
    print!("{}", help_text())
}

/// Prints a parse error and returns the CLI process exit code.
///
/// Must be a failure code: rejected flags (`--native` without the `llvm`
/// feature, for one) print to stderr and produce no output, so a success code
/// makes callers see an empty-but-successful run.
fn render_parse_error(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}
