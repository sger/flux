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
    Help,
}

/// Parses process arguments into a concrete CLI command plus grouped driver flags.
///
/// Parsing happens in two lightweight passes over the mutable argv buffer:
/// one for boolean or mode-like flags and one for value-carrying options. The remaining
/// positional arguments then drive subcommand selection.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<CliCommand, String> {
    let mut args = collect_cli_args(args);
    let (parsed, mut flags) = parse_driver_flags(&mut args)?;

    if has_no_command_or_input(&args) {
        return Ok(CliCommand::Help);
    }

    let run_mode = run_mode_from_flags(parsed.execution.test_mode);

    if let Some(command) = parse_implicit_file_command(&args, flags.clone(), run_mode)? {
        return Ok(command);
    }

    parse_subcommand(&args, &mut flags, run_mode)
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
        target: RunTarget { path, mode },
    }
}

/// Parses the explicit `run` subcommand form.
fn parse_run_subcommand(
    args: &[String],
    flags: &DriverFlags,
    run_mode: RunMode,
) -> Result<CliCommand, String> {
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
    use crate::driver::{
        AetherDumpMode, CoreDumpMode, RunMode, backend::Backend, test_support::base_flags,
    };
    use std::path::Path;

    fn cli(parts: &[&str]) -> Vec<std::ffi::OsString> {
        parts.iter().map(|part| (*part).into()).collect()
    }

    #[test]
    fn parses_implicit_file_run() {
        let command = parse_args(cli(&["flux", "examples/basics/arithmetic.flx"])).unwrap();
        match command {
            CliCommand::Run { target, .. } => {
                assert_eq!(target.mode, RunMode::Program);
                assert_eq!(target.path, "examples/basics/arithmetic.flx");
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn parses_dump_modes() {
        let command = parse_args(cli(&[
            "flux",
            "examples/basics/arithmetic.flx",
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
            "examples/basics/arithmetic.flx",
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
            "examples/basics/arithmetic.flx",
            "--dump-lir",
        ]))
        .unwrap_err();
        #[cfg(feature = "core_to_llvm")]
        {
            assert!(err.contains("dump-lir"));
        }
        #[cfg(not(feature = "core_to_llvm"))]
        {
            assert!(err.contains("native"));
        }
    }

    #[test]
    fn emit_llvm_implies_native_backend() {
        let command = parse_args(cli(&[
            "flux",
            "examples/basics/arithmetic.flx",
            "--emit-llvm",
        ]));
        #[cfg(feature = "core_to_llvm")]
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
        #[cfg(not(feature = "core_to_llvm"))]
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
            "examples/basics/arithmetic.flx",
            "--native",
            "--dump-cfg",
            "--cache-dir",
            ".flux-cache",
            "--no-cache",
            "--strict",
            "--strict-types",
            "--optimize",
            "--analyze",
            "--verbose",
            "--trace",
            "--stats",
            "--prof",
            "-o",
            "out.ll",
        ]))
        .unwrap();

        match command {
            CliCommand::Run { flags, .. } => {
                assert_eq!(flags.backend.output_path.as_deref(), Some("out.ll"));
                assert!(flags.dumps.dump_cfg);
                assert_eq!(
                    flags.cache.cache_dir.as_deref(),
                    Some(Path::new(".flux-cache"))
                );
                assert!(flags.cache.no_cache);
                assert!(flags.language.strict_mode);
                assert!(flags.language.strict_types);
                assert!(flags.language.enable_optimize);
                assert!(flags.language.enable_analyze);
                assert!(flags.runtime.verbose);
                assert!(flags.runtime.trace);
                assert!(flags.runtime.show_stats);
                assert!(flags.runtime.profiling);
                #[cfg(feature = "native")]
                {
                    assert_eq!(flags.backend.selected, Backend::Native);
                    assert!(flags.backend.use_core_to_llvm);
                }
                #[cfg(not(feature = "native"))]
                {
                    assert_eq!(flags.backend.selected, Backend::Vm);
                    assert!(!flags.backend.use_core_to_llvm);
                }
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn subcommands_store_input_path_in_grouped_input_flags() {
        let command =
            parse_args(cli(&["flux", "tokens", "examples/basics/arithmetic.flx"])).unwrap();
        match command {
            CliCommand::Tokens { flags } => {
                assert_eq!(
                    flags.input.input_path.as_deref(),
                    Some("examples/basics/arithmetic.flx")
                );
            }
            other => panic!("expected tokens mode, got {other:?}"),
        }
    }

    #[test]
    fn test_filter_is_stored_in_grouped_input_flags() {
        let command = parse_args(cli(&[
            "flux",
            "examples/basics/arithmetic.flx",
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
        let command =
            parse_args(cli(&["flux", "examples/basics/arithmetic.flx", "--native"])).unwrap();

        match command {
            CliCommand::Run { flags, target } => {
                assert_eq!(target.mode, RunMode::Program);
                assert_eq!(target.path, "examples/basics/arithmetic.flx");
                #[cfg(feature = "native")]
                {
                    assert_eq!(flags.backend.selected, Backend::Native);
                    assert!(flags.is_native_backend());
                    assert!(flags.backend.use_core_to_llvm);
                }
                #[cfg(not(feature = "native"))]
                {
                    assert_eq!(flags.backend.selected, Backend::Vm);
                    assert!(!flags.is_native_backend());
                }
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn parses_native_test_run_path() {
        let command = parse_args(cli(&[
            "flux",
            "examples/basics/arithmetic.flx",
            "--native",
            "--test",
        ]))
        .unwrap();

        match command {
            CliCommand::Run { flags, target } => {
                assert_eq!(target.mode, RunMode::Tests);
                assert_eq!(target.path, "examples/basics/arithmetic.flx");
                #[cfg(feature = "native")]
                {
                    assert_eq!(flags.backend.selected, Backend::Native);
                    assert!(flags.is_native_backend());
                }
                #[cfg(not(feature = "native"))]
                {
                    assert_eq!(flags.backend.selected, Backend::Vm);
                    assert!(!flags.is_native_backend());
                }
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn parses_emit_binary_as_native_path() {
        let command = parse_args(cli(&[
            "flux",
            "examples/basics/arithmetic.flx",
            "--emit-binary",
        ]));

        #[cfg(feature = "native")]
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

        #[cfg(not(feature = "native"))]
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
            "examples/basics/arithmetic.flx".into(),
        ])
        .unwrap();

        assert!(check);
        assert_eq!(path, "examples/basics/arithmetic.flx");
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
