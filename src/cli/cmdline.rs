//! CLI argument parsing and command selection.

use std::{ffi::OsString, path::Path};

use crate::{
    cli::render::text::{expected_flx, expected_flxi, fmt_check_usage, fmt_usage, unknown_command},
    cli::shared::{
        ParsedCliFlags, build_driver_flags, extract_cli_flag_groups, extract_cli_value_options,
    },
    driver::{RunMode, backend_policy::validate_flags, flags::DriverFlags, pipeline::RunTarget},
};

#[derive(Debug, Clone)]
/// Parsed top-level CLI commands supported by the Flux executable.
pub enum CliCommand {
    Run {
        flags: DriverFlags,
        target: RunTarget,
    },
    Tokens {
        flags: DriverFlags,
    },
    Bytecode {
        flags: DriverFlags,
    },
    Lint {
        flags: DriverFlags,
    },
    Fmt {
        path: String,
        check: bool,
    },
    Eval {
        expr: String,
        flags: DriverFlags,
    },
    Repl {
        flags: DriverFlags,
    },
    CacheInfo {
        flags: DriverFlags,
    },
    ModuleCacheInfo {
        flags: DriverFlags,
    },
    NativeCacheInfo {
        flags: DriverFlags,
    },
    Clean {
        flags: DriverFlags,
    },
    InterfaceInfo {
        flags: DriverFlags,
    },
    AnalyzeFreeVars {
        flags: DriverFlags,
    },
    AnalyzeTailCalls {
        flags: DriverFlags,
    },
    ParityCheck {
        raw_args: Vec<String>,
    },
    /// `flux init [name] [--lib]` — scaffold a package in place.
    Init {
        name: Option<String>,
        is_lib: bool,
    },
    /// `flux new <name> [--lib]` — scaffold a package into a new directory.
    New {
        name: String,
        is_lib: bool,
    },
    /// `flux build` / `run` / `test` / `check` — operate on the current
    /// package. The entry file comes from `Flume.Cli`, so these carry the
    /// selected `[[bin]]` rather than a path.
    Package {
        action: PackageAction,
        flags: DriverFlags,
        bin: Option<String>,
        program_args: Vec<String>,
    },
    Help,
}

/// Which package command was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageAction {
    /// Compile the package without running it.
    Build,
    /// Compile and run the package's entry point.
    Run,
    /// Run the package's `test_*` functions.
    Test,
    /// Type-check without producing artifacts.
    Check,
    /// Print the resolved dependency graph.
    Tree,
    /// Record a dependency in `flux.toml`.
    Add,
    /// Drop a dependency from `flux.toml`.
    Remove,
}

/// Parses process arguments into a concrete CLI command plus grouped driver flags.
///
/// Parsing happens in two lightweight passes over the mutable argv buffer:
/// one for boolean or mode-like flags and one for value-carrying options. The remaining
/// positional arguments then drive subcommand selection.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<CliCommand, String> {
    let mut args = collect_cli_args(args);
    // Everything after a `--` belongs to the program, not to flux. Split it
    // off first so the program may take flags that flux also understands (or
    // does not) without the CLI parser claiming or rejecting them.
    let program_args = split_program_args(&mut args);
    let (parsed, mut flags) = parse_driver_flags(&mut args)?;

    if has_no_command_or_input(&args) {
        return Ok(CliCommand::Help);
    }

    reject_unknown_flag_tokens(&args)?;

    let run_mode = run_mode_from_flags(parsed.execution.test_mode);

    if let Some(command) = parse_implicit_file_command(&args, flags.clone(), run_mode)? {
        return Ok(attach_program_args(command, program_args));
    }

    parse_subcommand(&args, &mut flags, run_mode).map(|c| attach_program_args(c, program_args))
}

/// Removes a `--` separator and everything after it, returning the tail.
///
/// The separator itself is dropped. A trailing bare `--` yields an empty
/// argument list, which is distinct from never having written one only in
/// that it is still an explicit choice; both give the program no arguments.
fn split_program_args(args: &mut Vec<String>) -> Vec<String> {
    match args.iter().position(|a| a == "--") {
        Some(idx) => args.split_off(idx).into_iter().skip(1).collect(),
        None => Vec::new(),
    }
}

/// Attaches program arguments to a run command; other commands ignore them.
fn attach_program_args(command: CliCommand, program_args: Vec<String>) -> CliCommand {
    match command {
        CliCommand::Run { flags, mut target } => {
            target.program_args = program_args;
            CliCommand::Run { flags, target }
        }
        // `flux run -- args` forwards to the package's entry point just as
        // `flux run file.flx -- args` does.
        CliCommand::Package {
            action,
            flags,
            bin,
            program_args: claimed,
        } => CliCommand::Package {
            action,
            flags,
            bin,
            // `add` and `remove` already claimed their arguments from the
            // command line; anything after `--` belongs to a program being
            // run, and these run none. Overwriting here would discard the
            // dependency they were asked to record.
            program_args: if claimed.is_empty() {
                program_args
            } else {
                claimed
            },
        },
        other => other,
    }
}

/// Converts raw process arguments into an owned CLI buffer.
fn collect_cli_args(args: impl IntoIterator<Item = OsString>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// Extracts grouped CLI flags, builds `DriverFlags`, and validates backend policy.
fn parse_driver_flags(args: &mut Vec<String>) -> Result<(ParsedCliFlags, DriverFlags), String> {
    let parsed = extract_cli_flag_groups(args);
    let values = extract_cli_value_options(args)?;
    let flags = build_driver_flags(parsed, values);
    validate_driver_flags(&flags, parsed.execution.test_mode)?;
    Ok((parsed, flags))
}

/// Validates the parsed driver flags against backend-specific CLI policy.
fn validate_driver_flags(flags: &DriverFlags, test_mode: bool) -> Result<(), String> {
    validate_flags(flags, test_mode).map_err(|err| err.to_string())
}

/// Returns whether the argv buffer contains no subcommand or implicit input path.
fn has_no_command_or_input(args: &[String]) -> bool {
    args.len() < 2
}

/// Rejects leftover CLI flags after the known flag-extraction passes complete.
///
/// `parity-check` forwards raw arguments to its own parser, and `eval` takes a
/// free-form expression (which may contain `-`-leading tokens), so their tails are exempt.
fn reject_unknown_flag_tokens(args: &[String]) -> Result<(), String> {
    // `parity-check` forwards raw arguments to its own parser, `eval` takes a
    // free-form expression, and the package commands take their own flags
    // (`--lib`, `--bin <name>`, `--filter <s>`).
    const OWN_FLAGS: &[&str] = &[
        "parity-check",
        "eval",
        "init",
        "new",
        "build",
        "run",
        "test",
        "check",
        "tree",
        // `add` and `remove` take flags the package manager parses —
        // `--git`, `--tag`, `--path`, and the rest — so the driver must not
        // reject them as unknown before forwarding.
        "add",
        "remove",
    ];
    if args
        .get(1)
        .is_some_and(|arg| OWN_FLAGS.contains(&arg.as_str()))
    {
        return Ok(());
    }

    for arg in args.iter().skip(1) {
        if arg.starts_with("--") && arg != "--help" {
            return Err(format!("Error: unknown flag `{arg}`."));
        }
    }

    Ok(())
}

/// Chooses the run mode from the parsed CLI execution flags.
fn run_mode_from_flags(test_mode: bool) -> RunMode {
    if test_mode {
        RunMode::Tests
    } else {
        RunMode::Program
    }
}

/// Parses the implicit `flux file.flx` form when the second argument is a source path.
fn parse_implicit_file_command(
    args: &[String],
    flags: DriverFlags,
    run_mode: RunMode,
) -> Result<Option<CliCommand>, String> {
    if !is_flx_file(&args[1]) {
        return Ok(None);
    }

    let path = require_flx_arg(args, 1, "Usage: flux <file.flx>")?;
    Ok(Some(run_command(flags, path, run_mode)))
}

/// `flux init [name] [--lib]`. The name defaults to the directory name.
fn parse_init_subcommand(args: &[String]) -> CliCommand {
    let is_lib = args.iter().any(|a| a == "--lib");
    let name = args.iter().skip(2).find(|a| !a.starts_with("--")).cloned();
    CliCommand::Init { name, is_lib }
}

/// `flux new <name> [--lib]`. Unlike `init`, the name is required.
fn parse_new_subcommand(args: &[String]) -> Result<CliCommand, String> {
    let is_lib = args.iter().any(|a| a == "--lib");
    let name = args
        .iter()
        .skip(2)
        .find(|a| !a.starts_with("--"))
        .cloned()
        .ok_or_else(|| "Usage: flux new <name> [--lib]".to_string())?;
    Ok(CliCommand::New { name, is_lib })
}

/// Builds a package command, reading `--bin <name>` from the argument list.
fn parse_package_subcommand(
    action: PackageAction,
    args: &[String],
    flags: &DriverFlags,
) -> CliCommand {
    let bin = args
        .iter()
        .position(|a| a == "--bin")
        .and_then(|idx| args.get(idx + 1))
        .cloned();
    // `add` and `remove` take their own arguments — the dependency name and
    // where it comes from — which the package manager parses. Every other
    // package command's arguments are consumed here, so they carry none.
    let program_args = if matches!(action, PackageAction::Add | PackageAction::Remove) {
        args.iter().skip(2).cloned().collect()
    } else {
        Vec::new()
    };
    CliCommand::Package {
        action,
        flags: flags.clone(),
        bin,
        program_args,
    }
}

/// Whether the arguments name a file, distinguishing `flux test` (the package
/// command) from any future file-taking form.
fn args_name_a_file(args: &[String]) -> bool {
    args.iter().skip(2).any(|a| is_flx_file(a))
}

/// Parses the explicit subcommand form after flag extraction and validation.
fn parse_subcommand(
    args: &[String],
    flags: &mut DriverFlags,
    run_mode: RunMode,
) -> Result<CliCommand, String> {
    match args[1].as_str() {
        "-h" | "--help" | "help" => Ok(CliCommand::Help),
        "run" => parse_run_subcommand(args, flags, run_mode),
        "tokens" => {
            parse_flx_subcommand(args, flags, 2, "Usage: flux tokens <file.flx>", |flags| {
                CliCommand::Tokens { flags }
            })
        }
        "bytecode" => {
            parse_flx_subcommand(args, flags, 2, "Usage: flux bytecode <file.flx>", |flags| {
                CliCommand::Bytecode { flags }
            })
        }
        "lint" => parse_flx_subcommand(args, flags, 2, "Usage: flux lint <file.flx>", |flags| {
            CliCommand::Lint { flags }
        }),
        "fmt" => parse_fmt_subcommand(args),
        "init" => Ok(parse_init_subcommand(args)),
        "new" => parse_new_subcommand(args),
        "build" => Ok(parse_package_subcommand(PackageAction::Build, args, flags)),
        "test" if !args_name_a_file(args) => {
            Ok(parse_package_subcommand(PackageAction::Test, args, flags))
        }
        "check" => Ok(parse_package_subcommand(PackageAction::Check, args, flags)),
        "tree" => Ok(parse_package_subcommand(PackageAction::Tree, args, flags)),
        "add" => Ok(parse_package_subcommand(PackageAction::Add, args, flags)),
        "remove" => Ok(parse_package_subcommand(PackageAction::Remove, args, flags)),
        "eval" => parse_eval_subcommand(args, flags),
        "repl" => Ok(CliCommand::Repl {
            flags: flags.clone(),
        }),
        "cache-info" => parse_flx_subcommand(
            args,
            flags,
            2,
            "Usage: flux cache-info <file.flx>",
            |flags| CliCommand::CacheInfo { flags },
        ),
        "module-cache-info" => parse_flx_subcommand(
            args,
            flags,
            2,
            "Usage: flux module-cache-info <file.flx>",
            |flags| CliCommand::ModuleCacheInfo { flags },
        ),
        "native-cache-info" => parse_flx_subcommand(
            args,
            flags,
            2,
            "Usage: flux native-cache-info <file.flx>",
            |flags| CliCommand::NativeCacheInfo { flags },
        ),
        "clean" => Ok(clean_command(flags.clone(), args)),
        "interface-info" => parse_flxi_subcommand(
            args,
            flags,
            2,
            "Usage: flux interface-info <file.flxi>",
            |flags| CliCommand::InterfaceInfo { flags },
        ),
        "analyze-free-vars" | "free-vars" => parse_flx_subcommand(
            args,
            flags,
            2,
            "Usage: flux analyze-free-vars <file.flx>",
            |flags| CliCommand::AnalyzeFreeVars { flags },
        ),
        "analyze-tail-calls" | "analyze-tails-calls" | "tail-calls" => parse_flx_subcommand(
            args,
            flags,
            2,
            "Usage: flux analyze-tail-calls <file.flx>",
            |flags| CliCommand::AnalyzeTailCalls { flags },
        ),
        "parity-check" => Ok(parity_check_command(args)),
        other => Err(unknown_command(other)),
    }
}

/// Builds a run command from a resolved source path and execution mode.
///
/// Keeping this small constructor separate makes the subcommand match easier to scan.
fn run_command(flags: DriverFlags, path: String, mode: RunMode) -> CliCommand {
    CliCommand::Run {
        flags,
        target: RunTarget {
            path,
            mode,
            program_args: Vec::new(),
        },
    }
}

/// Parses the explicit `run` subcommand form.
fn parse_run_subcommand(
    args: &[String],
    flags: &DriverFlags,
    run_mode: RunMode,
) -> Result<CliCommand, String> {
    // `flux run` with no file runs the current package; `flux run <file.flx>`
    // keeps its script-mode meaning.
    if !args_name_a_file(args) {
        return Ok(parse_package_subcommand(PackageAction::Run, args, flags));
    }
    let path = require_flx_arg(args, 2, "Usage: flux run <file.flx>")?;
    Ok(run_command(flags.clone(), path, run_mode))
}

/// Builds the `clean` command and attaches an optional `.flx` input path when present.
fn clean_command(mut flags: DriverFlags, args: &[String]) -> CliCommand {
    flags.input.input_path = optional_flx_input(args, 2);
    CliCommand::Clean { flags }
}

/// Builds the parity-check command from the remaining positional arguments.
fn parity_check_command(args: &[String]) -> CliCommand {
    CliCommand::ParityCheck {
        raw_args: args[2..].to_vec(),
    }
}

/// Returns an optional `.flx` positional argument at `index`.
fn optional_flx_input(args: &[String], index: usize) -> Option<String> {
    args.get(index).filter(|path| is_flx_file(path)).cloned()
}

/// Parses a subcommand that requires a `.flx` source path and stores it in grouped input flags.
fn parse_flx_subcommand(
    args: &[String],
    flags: &DriverFlags,
    index: usize,
    usage: &str,
    build: impl FnOnce(DriverFlags) -> CliCommand,
) -> Result<CliCommand, String> {
    parse_path_subcommand(args, flags, index, usage, require_flx_arg, build)
}

/// Parses a subcommand that requires a `.flxi` interface path and stores it in grouped input flags.
fn parse_flxi_subcommand(
    args: &[String],
    flags: &DriverFlags,
    index: usize,
    usage: &str,
    build: impl FnOnce(DriverFlags) -> CliCommand,
) -> Result<CliCommand, String> {
    parse_path_subcommand(args, flags, index, usage, require_flxi_arg, build)
}

/// Attaches a validated path argument to grouped input flags and constructs a command variant.
fn parse_path_subcommand(
    args: &[String],
    flags: &DriverFlags,
    index: usize,
    usage: &str,
    parse_path: impl Fn(&[String], usize, &str) -> Result<String, String>,
    build: impl FnOnce(DriverFlags) -> CliCommand,
) -> Result<CliCommand, String> {
    let mut flags = flags.clone();
    flags.input.input_path = Some(parse_path(args, index, usage)?);
    Ok(build(flags))
}

/// Parses the `eval` subcommand: everything after `eval` is joined into a single
/// expression string. Unlike every other subcommand this takes a free-form
/// expression rather than a `.flx` path, so it has no path validation — it only
/// rejects an empty expression. Joining `args[2..]` tolerates an accidentally
/// unquoted multi-word expression (`flux eval 2 + 2`) as well as the quoted form.
fn parse_eval_subcommand(args: &[String], flags: &DriverFlags) -> Result<CliCommand, String> {
    let expr = args[2..].join(" ");
    let expr = expr.trim();
    if expr.is_empty() {
        return Err("Usage: flux eval \"<expr>\"".to_string());
    }
    Ok(CliCommand::Eval {
        expr: expr.to_string(),
        flags: flags.clone(),
    })
}

/// Parses the `fmt` subcommand and returns the path/check-mode command variant.
fn parse_fmt_subcommand(args: &[String]) -> Result<CliCommand, String> {
    let (path, check) = parse_fmt_command(args)?;
    Ok(CliCommand::Fmt { path, check })
}

/// Parses `flux fmt` arguments and returns the target path plus `--check` mode.
///
/// The formatter accepts a single positional `.flx` path with an optional `--check` switch.
fn parse_fmt_command(args: &[String]) -> Result<(String, bool), String> {
    if args.len() < 3 {
        return Err(fmt_usage().to_string());
    }
    let check = args.iter().any(|arg| arg == "--check");
    if check && args.len() < 4 {
        return Err(fmt_check_usage().to_string());
    }
    let path = require_fmt_path(args, check)?;
    Ok((path, check))
}

/// Returns the required formatter path based on whether `--check` is present.
fn require_fmt_path(args: &[String], check: bool) -> Result<String, String> {
    let index = if check { 3 } else { 2 };
    require_flx_arg(args, index, fmt_usage())
}

/// Returns the required `.flx` argument at `index` or a CLI-formatted error.
fn require_flx_arg(args: &[String], index: usize, usage: &str) -> Result<String, String> {
    let path = args.get(index).ok_or_else(|| usage.to_string())?;
    if is_flx_file(path) {
        Ok(path.clone())
    } else {
        Err(expected_flx(path))
    }
}

/// Returns the required `.flxi` argument at `index` or a CLI-formatted error.
fn require_flxi_arg(args: &[String], index: usize, usage: &str) -> Result<String, String> {
    let path = args.get(index).ok_or_else(|| usage.to_string())?;
    if path.ends_with(".flxi") {
        Ok(path.clone())
    } else {
        Err(expected_flxi(path))
    }
}

/// Returns whether the provided path uses the `.flx` source-file extension.
fn is_flx_file(path: &str) -> bool {
    Path::new(path).extension().and_then(|ext| ext.to_str()) == Some("flx")
}

#[cfg(test)]
mod tests {
    use super::{
        CliCommand, clean_command, is_flx_file, optional_flx_input, parse_args,
        parse_flx_subcommand, parse_fmt_command, require_flx_arg, require_flxi_arg,
        run_mode_from_flags,
    };
    #[cfg(feature = "llvm")]
    use crate::driver::backend::Backend;
    use crate::driver::{AetherDumpMode, CoreDumpMode, RunMode, test_support::base_flags};
    fn cli(parts: &[&str]) -> Vec<std::ffi::OsString> {
        parts.iter().map(|part| (*part).into()).collect()
    }

    #[test]
    fn parses_implicit_file_run() {
        let command = parse_args(cli(&["flux", "examples/guide/arithmetic.flx"])).unwrap();
        match command {
            CliCommand::Run { target, .. } => {
                assert_eq!(target.mode, RunMode::Program);
                assert_eq!(target.path, "examples/guide/arithmetic.flx");
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn parses_dump_modes() {
        let command = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--dump-core=debug",
            "--dump-aether",
        ]))
        .unwrap();
        match command {
            CliCommand::Run { flags, .. } => {
                assert_eq!(flags.dumps.dump_core, CoreDumpMode::Debug);
                assert_eq!(flags.dumps.dump_aether, AetherDumpMode::Summary);
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn rejects_trace_aether_with_dump() {
        let err = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--trace-aether",
            "--dump-core",
        ]))
        .unwrap_err();
        assert!(err.contains("--trace-aether"));
    }

    #[test]
    fn rejects_dump_lir_without_native() {
        let err = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--dump-lir",
        ]))
        .unwrap_err();
        #[cfg(feature = "llvm")]
        {
            assert!(err.contains("dump-lir"));
        }
        #[cfg(not(feature = "llvm"))]
        {
            assert!(err.contains("native"));
        }
    }

    #[test]
    fn emit_llvm_implies_native_backend() {
        let command = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--emit-llvm",
        ]));
        #[cfg(feature = "llvm")]
        {
            let command = command.unwrap();
            match command {
                CliCommand::Run { flags, .. } => {
                    assert!(flags.is_native_backend());
                    assert!(flags.backend.emit_llvm);
                }
                other => panic!("expected run mode, got {other:?}"),
            }
        }
        #[cfg(not(feature = "llvm"))]
        {
            let err = command.unwrap_err();
            assert!(err.contains("native"));
        }
    }

    #[test]
    fn parses_grouped_flag_storage() {
        let command = parse_args(cli(&[
            "flux",
            "run",
            "examples/guide/arithmetic.flx",
            "--native",
            "--dump-cfg",
            "--cache-dir",
            ".flux-cache",
            "--no-cache",
            "--strict",
            "--optimize",
            "--analyze",
            "--verbose",
            "--trace",
            "--stats",
            "--prof",
            "-o",
            "out.ll",
        ]));

        #[cfg(feature = "llvm")]
        match command.unwrap() {
            CliCommand::Run { flags, .. } => {
                assert_eq!(flags.backend.output_path.as_deref(), Some("out.ll"));
                assert!(flags.dumps.dump_cfg);
                assert_eq!(
                    flags.cache.cache_dir.as_deref(),
                    Some(std::path::Path::new(".flux-cache"))
                );
                assert!(flags.cache.no_cache);
                assert!(flags.language.strict_mode);
                assert!(flags.language.enable_optimize);
                assert!(flags.language.enable_analyze);
                assert!(flags.runtime.verbose);
                assert!(flags.runtime.trace);
                assert!(flags.runtime.show_stats);
                assert!(flags.runtime.profiling);
                assert_eq!(flags.backend.selected, Backend::Native);
                assert!(flags.backend.use_llvm);
            }
            other => panic!("expected run mode, got {other:?}"),
        }

        #[cfg(not(feature = "llvm"))]
        {
            let err = command.unwrap_err();
            assert!(err.contains("native backend features require"));
        }
    }

    #[test]
    fn subcommands_store_input_path_in_grouped_input_flags() {
        let command =
            parse_args(cli(&["flux", "tokens", "examples/guide/arithmetic.flx"])).unwrap();
        match command {
            CliCommand::Tokens { flags } => {
                assert_eq!(
                    flags.input.input_path.as_deref(),
                    Some("examples/guide/arithmetic.flx")
                );
            }
            other => panic!("expected tokens mode, got {other:?}"),
        }
    }

    #[test]
    fn test_filter_is_stored_in_grouped_input_flags() {
        let command = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--test",
            "--test-filter",
            "arith",
        ]))
        .unwrap();
        match command {
            CliCommand::Run { flags, target } => {
                assert_eq!(target.mode, RunMode::Tests);
                assert_eq!(flags.input.test_filter.as_deref(), Some("arith"));
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_program_run_path() {
        let command = parse_args(cli(&["flux", "examples/guide/arithmetic.flx", "--native"]));

        #[cfg(feature = "llvm")]
        {
            match command.unwrap() {
                CliCommand::Run { flags, target } => {
                    assert_eq!(target.mode, RunMode::Program);
                    assert_eq!(target.path, "examples/guide/arithmetic.flx");
                    assert_eq!(flags.backend.selected, Backend::Native);
                    assert!(flags.is_native_backend());
                    assert!(flags.backend.use_llvm);
                }
                other => panic!("expected run mode, got {other:?}"),
            }
        }

        #[cfg(not(feature = "llvm"))]
        {
            let err = command.unwrap_err();
            assert!(err.contains("native backend features require"));
        }
    }

    #[test]
    fn rejects_removed_core_to_llvm_flag() {
        let err = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--core-to-llvm",
        ]))
        .unwrap_err();

        assert!(err.contains("--core-to-llvm"));
    }

    #[test]
    fn parses_native_test_run_path() {
        let command = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--native",
            "--test",
        ]));

        #[cfg(feature = "llvm")]
        {
            match command.unwrap() {
                CliCommand::Run { flags, target } => {
                    assert_eq!(target.mode, RunMode::Tests);
                    assert_eq!(target.path, "examples/guide/arithmetic.flx");
                    assert_eq!(flags.backend.selected, Backend::Native);
                    assert!(flags.is_native_backend());
                }
                other => panic!("expected run mode, got {other:?}"),
            }
        }

        #[cfg(not(feature = "llvm"))]
        {
            let err = command.unwrap_err();
            assert!(err.contains("native backend features require"));
        }
    }

    #[test]
    fn dump_lir_llvm_is_recognized_but_needs_backend_support() {
        let command = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--dump-lir-llvm",
        ]));

        #[cfg(feature = "llvm")]
        {
            let err = command.unwrap_err();
            assert!(err.contains("--dump-lir/--dump-lir-llvm requires the native backend"));
        }

        #[cfg(not(feature = "llvm"))]
        {
            let err = command.unwrap_err();
            assert!(err.contains("native backend features require"));
        }
    }

    #[test]
    fn parses_emit_binary_as_native_path() {
        let command = parse_args(cli(&[
            "flux",
            "examples/guide/arithmetic.flx",
            "--emit-binary",
        ]));

        #[cfg(feature = "llvm")]
        {
            let command = command.unwrap();
            match command {
                CliCommand::Run { flags, target } => {
                    assert_eq!(target.mode, RunMode::Program);
                    assert_eq!(flags.backend.selected, Backend::Native);
                    assert!(flags.is_native_backend());
                    assert!(flags.backend.emit_binary);
                }
                other => panic!("expected run mode, got {other:?}"),
            }
        }

        #[cfg(not(feature = "llvm"))]
        {
            let err = command.unwrap_err();
            assert!(err.contains("native"));
        }
    }

    #[test]
    fn parse_fmt_command_supports_check_mode() {
        let (path, check) = parse_fmt_command(&[
            "flux".into(),
            "fmt".into(),
            "--check".into(),
            "examples/guide/arithmetic.flx".into(),
        ])
        .unwrap();

        assert!(check);
        assert_eq!(path, "examples/guide/arithmetic.flx");
    }

    #[test]
    fn parse_fmt_command_requires_path() {
        let err = parse_fmt_command(&["flux".into(), "fmt".into()]).unwrap_err();

        assert!(err.contains("Usage: flux fmt"));
    }

    #[test]
    fn parse_fmt_command_requires_path_after_check() {
        let err = parse_fmt_command(&["flux".into(), "fmt".into(), "--check".into()]).unwrap_err();

        assert!(err.contains("Usage: flux fmt --check"));
    }

    #[test]
    fn parses_eval_expression() {
        let command = parse_args(cli(&["flux", "eval", "2 + 2"])).unwrap();
        match command {
            CliCommand::Eval { expr, .. } => assert_eq!(expr, "2 + 2"),
            other => panic!("expected eval mode, got {other:?}"),
        }
    }

    #[test]
    fn eval_joins_unquoted_expression_words() {
        let command = parse_args(cli(&["flux", "eval", "2", "+", "2"])).unwrap();
        match command {
            CliCommand::Eval { expr, .. } => assert_eq!(expr, "2 + 2"),
            other => panic!("expected eval mode, got {other:?}"),
        }
    }

    #[test]
    fn eval_without_expression_is_usage_error() {
        let err = parse_args(cli(&["flux", "eval"])).unwrap_err();
        assert!(err.contains("Usage: flux eval"));
    }

    #[test]
    fn parses_repl_subcommand() {
        let command = parse_args(cli(&["flux", "repl"])).unwrap();
        assert!(matches!(command, CliCommand::Repl { .. }));
    }

    #[test]
    fn run_mode_from_flags_maps_test_switch_to_tests_mode() {
        assert_eq!(run_mode_from_flags(false), RunMode::Program);
        assert_eq!(run_mode_from_flags(true), RunMode::Tests);
    }

    #[test]
    fn require_flx_arg_rejects_non_flux_source_paths() {
        let err = require_flx_arg(&["flux".into(), "file.txt".into()], 1, "usage").unwrap_err();

        assert!(err.contains(".flx"));
    }

    #[test]
    fn require_flxi_arg_rejects_non_interface_paths() {
        let err = require_flxi_arg(&["flux".into(), "file.flx".into()], 1, "usage").unwrap_err();

        assert!(err.contains(".flxi"));
    }

    #[test]
    fn require_flx_arg_requires_present_argument() {
        let err = require_flx_arg(&["flux".into()], 1, "usage").unwrap_err();

        assert_eq!(err, "usage");
    }

    #[test]
    fn require_flxi_arg_requires_present_argument() {
        let err = require_flxi_arg(&["flux".into()], 1, "usage").unwrap_err();

        assert_eq!(err, "usage");
    }

    #[test]
    fn is_flx_file_requires_flux_source_extension() {
        assert!(is_flx_file("file.flx"));
        assert!(!is_flx_file("file.flxi"));
        assert!(!is_flx_file("file.txt"));
    }

    #[test]
    fn optional_flx_input_only_accepts_flux_sources() {
        assert_eq!(
            optional_flx_input(&["flux".into(), "clean".into(), "file.flx".into()], 2),
            Some("file.flx".into())
        );
        assert_eq!(
            optional_flx_input(&["flux".into(), "clean".into(), "file.txt".into()], 2),
            None
        );
    }

    #[test]
    fn clean_command_preserves_only_flux_input_paths() {
        let command = clean_command(
            base_flags(),
            &["flux".into(), "clean".into(), "file.txt".into()],
        );

        match command {
            CliCommand::Clean { flags } => assert_eq!(flags.input.input_path, None),
            other => panic!("expected clean mode, got {other:?}"),
        }
    }

    #[test]
    fn parse_flx_subcommand_stores_validated_input_path() {
        let command = parse_flx_subcommand(
            &["flux".into(), "tokens".into(), "file.flx".into()],
            &base_flags(),
            2,
            "usage",
            |flags| CliCommand::Tokens { flags },
        )
        .unwrap();

        match command {
            CliCommand::Tokens { flags } => {
                assert_eq!(flags.input.input_path.as_deref(), Some("file.flx"))
            }
            other => panic!("expected tokens mode, got {other:?}"),
        }
    }

    #[test]
    fn unknown_command_error_mentions_bad_token() {
        let err = parse_args(cli(&["flux", "wat"])).unwrap_err();

        assert!(err.contains("wat"));
        assert!(err.contains("valid subcommand"));
    }
}
