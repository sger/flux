use std::ffi::OsString;

use crate::{
    cli::render::text,
    diagnostics::DEFAULT_MAX_ERRORS,
    driver::{
        backend_policy,
        flags::DriverFlags,
        mode::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat, RunMode},
        pipeline::RunTarget,
    },
};

#[derive(Debug, Clone)]
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

pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<CliCommand, String> {
    let mut args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    let verbose = args.iter().any(|arg| arg == "--verbose");
    let leak_detector = args.iter().any(|arg| arg == "--leak-detector");
    let trace = args.iter().any(|arg| arg == "--trace");
    let trace_aether = args.iter().any(|arg| arg == "--trace-aether");
    let profiling = args.iter().any(|arg| arg == "--prof");
    let no_cache = args.iter().any(|arg| arg == "--no-cache");
    let roots_only = args.iter().any(|arg| arg == "--roots-only");
    let enable_optimize = args.iter().any(|arg| arg == "--optimize" || arg == "-O");
    let enable_analyze = args.iter().any(|arg| arg == "--analyze" || arg == "-A");
    let show_stats = args.iter().any(|arg| arg == "--stats");
    let test_mode = args.iter().any(|arg| arg == "--test");
    let strict_mode = args.iter().any(|arg| arg == "--strict");
    let strict_types = args.iter().any(|arg| arg == "--strict-types");
    let all_errors = args.iter().any(|arg| arg == "--all-errors");
    let dump_repr = args.iter().any(|arg| arg == "--dump-repr");
    let dump_cfg = args.iter().any(|arg| arg == "--dump-cfg");
    let dump_aether = if args.iter().any(|arg| arg == "--dump-aether=debug") {
        AetherDumpMode::Debug
    } else if args.iter().any(|arg| arg == "--dump-aether") {
        AetherDumpMode::Summary
    } else {
        AetherDumpMode::None
    };
    let dump_lir = args.iter().any(|arg| arg == "--dump-lir");
    #[cfg(feature = "core_to_llvm")]
    let dump_lir_llvm = args.iter().any(|arg| arg == "--dump-lir-llvm");
    #[cfg(not(feature = "core_to_llvm"))]
    let dump_lir_llvm = false;
    #[cfg(feature = "native")]
    let use_core_to_llvm = args
        .iter()
        .any(|arg| arg == "--core-to-llvm" || arg == "--native");
    #[cfg(not(feature = "native"))]
    let use_core_to_llvm = false;
    let emit_llvm = args.iter().any(|arg| arg == "--emit-llvm");
    let emit_binary = args.iter().any(|arg| arg == "--emit-binary");
    let mut roots = Vec::new();

    retain_flag(&mut args, "--verbose", verbose);
    retain_flag(&mut args, "--leak-detector", leak_detector);
    retain_flag(&mut args, "--trace", trace);
    retain_flag(&mut args, "--trace-aether", trace_aether);
    retain_flag(&mut args, "--no-cache", no_cache);
    retain_flag(&mut args, "--prof", profiling);
    retain_flag(&mut args, "--root-only", roots_only);
    if enable_optimize {
        args.retain(|arg| arg != "--optimize" && arg != "-O");
    }
    if enable_analyze {
        args.retain(|arg| arg != "--analyze" && arg != "-A");
    }
    retain_flag(&mut args, "--stats", show_stats);
    retain_flag(&mut args, "--test", test_mode);
    retain_flag(&mut args, "--strict", strict_mode);
    retain_flag(&mut args, "--strict-types", strict_types);
    args.retain(|arg| arg != "--no-strict");
    retain_flag(&mut args, "--all-errors", all_errors);
    retain_flag(&mut args, "--dump-repr", dump_repr);
    retain_flag(&mut args, "--dump-cfg", dump_cfg);
    if dump_aether != AetherDumpMode::None {
        args.retain(|arg| arg != "--dump-aether" && arg != "--dump-aether=debug");
    }
    retain_flag(&mut args, "--dump-lir", dump_lir);
    if dump_lir_llvm {
        args.retain(|arg| arg != "--dump-lir-llvm");
    }
    if use_core_to_llvm {
        args.retain(|arg| arg != "--core-to-llvm" && arg != "--native");
    }
    retain_flag(&mut args, "--emit-llvm", emit_llvm);
    retain_flag(&mut args, "--emit-binary", emit_binary);

    let cache_dir = extract_cache_dir(&mut args)
        .ok_or_else(|| "Usage: flux <file.flx> --cache-dir <dir>".to_string())?;
    let output_path = extract_output_path(&mut args);
    let dump_core = extract_dump_core_mode(&mut args)
        .ok_or_else(|| "Usage: flux <file.flx> --dump-core[=debug]".to_string())?;
    let diagnostics_format = extract_diagnostic_format(&mut args)
        .ok_or_else(|| "Usage: flux <file.flx> --format <text|json|json-compact>".to_string())?;
    let max_errors = extract_max_errors(&mut args)
        .ok_or_else(|| "Usage: flux <file.flx> --max-errors <n>".to_string())?;
    let test_filter = extract_test_filter(&mut args)
        .ok_or_else(|| "Usage: flux <file.flx> --test --test-filter <pattern>".to_string())?;
    if !extract_roots(&mut args, &mut roots) {
        return Err("Usage: flux <file.flx> --root <path> [--root <path> ...]".to_string());
    }

    if args.len() < 2 {
        return Ok(CliCommand::Help);
    }

    let mut flags = DriverFlags {
        backend: crate::driver::backend::Backend::Vm,
        input_path: None,
        roots_only,
        enable_optimize,
        enable_analyze,
        max_errors,
        roots: roots.clone(),
        cache_dir: cache_dir.clone(),
        strict_mode,
        strict_types,
        diagnostics_format,
        all_errors,
        verbose,
        leak_detector,
        trace,
        no_cache,
        show_stats,
        trace_aether,
        profiling,
        dump_repr,
        dump_cfg,
        dump_core,
        dump_aether,
        dump_lir,
        dump_lir_llvm,
        use_core_to_llvm,
        emit_llvm,
        emit_binary,
        output_path,
        test_filter,
    }
    .finalize_backend();

    if let Err(err) = backend_policy::validate_flags(&flags, test_mode) {
        return Err(err.to_string());
    }

    let run_mode = if test_mode {
        RunMode::Tests
    } else {
        RunMode::Program
    };

    if is_flx_file(&args[1]) {
        let path = require_flx_arg(&args, 1, "Usage: flux <file.flx>")?;
        return Ok(CliCommand::Run {
            flags,
            target: RunTarget {
                path,
                mode: run_mode,
            },
        });
    }

    match args[1].as_str() {
        "-h" | "--help" | "help" => Ok(CliCommand::Help),
        "run" => {
            let path = require_flx_arg(&args, 2, "Usage: flux run <file.flx>")?;
            Ok(CliCommand::Run {
                flags,
                target: RunTarget {
                    path,
                    mode: run_mode,
                },
            })
        }
        "tokens" => {
            flags.input_path = Some(require_flx_arg(&args, 2, "Usage: flux tokens <file.flx>")?);
            Ok(CliCommand::Tokens { flags })
        }
        "bytecode" => {
            flags.input_path = Some(require_flx_arg(
                &args,
                2,
                "Usage: flux bytecode <file.flx>",
            )?);
            Ok(CliCommand::Bytecode { flags })
        }
        "lint" => {
            flags.input_path = Some(require_flx_arg(&args, 2, "Usage: flux line <file.flx>")?);
            Ok(CliCommand::Lint { flags })
        }
        "fmt" => {
            let (path, check) = parse_fmt_command(&args)?;
            Ok(CliCommand::Fmt { path, check })
        }
        "cache-info" => {
            flags.input_path = Some(require_flx_arg(
                &args,
                2,
                "Usage: flux cache-info <file.flx>",
            )?);
            Ok(CliCommand::CacheInfo { flags })
        }
        "module-cache-info" => {
            flags.input_path = Some(require_flx_arg(
                &args,
                2,
                "Usage: flux module-cache-info <file.flx>",
            )?);
            Ok(CliCommand::ModuleCacheInfo { flags })
        }
        "native-cache-info" => {
            flags.input_path = Some(require_flx_arg(
                &args,
                2,
                "Usage: flux native-cache-info <file.flx>",
            )?);
            Ok(CliCommand::NativeCacheInfo { flags })
        }
        "clean" => {
            flags.input_path = args.get(2).filter(|path| is_flx_file(path)).cloned();
            Ok(CliCommand::Clean { flags })
        }
        "interface-info" => {
            flags.input_path = Some(require_flxi_arg(
                &args,
                2,
                "Usage: flux interface-info <file.flxi>",
            )?);
            Ok(CliCommand::InterfaceInfo { flags })
        }
        "analyze-free-vars" | "free-vars" => {
            flags.input_path = Some(require_flx_arg(
                &args,
                2,
                "Usage: flux analyze-free-vars <file.flx>",
            )?);
            Ok(CliCommand::AnalyzeFreeVars { flags })
        }
        "analyze-tail-calls" | "analyze-tails-calls" | "tail-calls" => {
            flags.input_path = Some(require_flx_arg(
                &args,
                2,
                "Usage: flux analyze-tail-calls <file.flx>",
            )?);
            Ok(CliCommand::AnalyzeTailCalls { flags })
        }
        "parity-check" => Ok(CliCommand::ParityCheck {
            raw_args: args[2..].to_vec(),
        }),
        other => Err(format!(
            "Error: unknown command or invalid input `{other}`. Pass a `.flx` file or a valid subcommand."
        )),
    }
}

fn retain_flag(args: &mut Vec<String>, flag: &str, enabled: bool) {
    if enabled {
        args.retain(|arg| arg != flag);
    }
}

fn parse_fmt_command(args: &[String]) -> Result<(String, bool), String> {
    if args.len() < 3 {
        return Err("Usage: flux fmt [--check] <file.flx>".to_string());
    }
    let check = args.iter().any(|arg| arg == "--check");
    if check && args.len() < 4 {
        return Err("Usage: flux fmt --check <file.flx>".to_string());
    }
    let index = if check { 3 } else { 2 };
    let path = require_flx_arg(args, index, "Usage: flux fmt [--check] <file.flx>")?;
    Ok((path, check))
}

fn require_flx_arg(args: &[String], index: usize, usage: &str) -> Result<String, String> {
    let path = args.get(index).ok_or_else(|| usage.to_string())?;
    if is_flx_file(path) {
        Ok(path.clone())
    } else {
        Err(text::expected_flx_file(path))
    }
}

fn is_flx_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        == Some("flx")
}

fn require_flxi_arg(args: &[String], index: usize, usage: &str) -> Result<String, String> {
    let path = args.get(index).ok_or_else(|| usage.to_string())?;
    if path.ends_with(".flxi") {
        Ok(path.clone())
    } else {
        Err(text::expected_flxi_file(path))
    }
}

fn extract_cache_dir(args: &mut Vec<String>) -> Option<Option<std::path::PathBuf>> {
    let mut cache_dir = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--cache-dir" {
            args.remove(i);
            if i < args.len() {
                cache_dir = Some(std::path::PathBuf::from(args.remove(i)));
                continue;
            }
            eprintln!("Error: --cache-dir requires a directory path.");
            return None;
        } else if let Some(value) = args[i].strip_prefix("--cache-dir=") {
            cache_dir = Some(std::path::PathBuf::from(value));
            args.remove(i);
            continue;
        }
        i += 1;
    }
    Some(cache_dir)
}

fn extract_output_path(args: &mut Vec<String>) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" {
            args.remove(i);
            if i < args.len() {
                return Some(args.remove(i));
            }
            eprintln!("Error: -o requires an output path argument.");
            return None;
        }
        i += 1;
    }
    None
}

fn extract_dump_core_mode(args: &mut Vec<String>) -> Option<CoreDumpMode> {
    let mut mode = CoreDumpMode::None;
    let mut i = 0;
    while i < args.len() {
        let next_mode = if args[i] == "--dump-core" {
            args.remove(i);
            CoreDumpMode::Readable
        } else if let Some(value) = args[i].strip_prefix("--dump-core=") {
            let parsed = match value {
                "debug" => CoreDumpMode::Debug,
                "" => {
                    eprintln!("Error: --dump-core expects no value or `debug`.");
                    return None;
                }
                _ => {
                    eprintln!("Error: --dump-core expects no value or `debug`.");
                    return None;
                }
            };
            args.remove(i);
            parsed
        } else {
            i += 1;
            continue;
        };
        mode = next_mode;
    }
    Some(mode)
}

fn extract_diagnostic_format(args: &mut Vec<String>) -> Option<DiagnosticOutputFormat> {
    let mut format = DiagnosticOutputFormat::Text;
    let mut i = 0;
    while i < args.len() {
        let value = if args[i] == "--format" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --format <text|json|json-compact>");
                return None;
            }
            let v = args.remove(i + 1);
            args.remove(i);
            v
        } else if let Some(v) = args[i].strip_prefix("--format=") {
            let v = v.to_string();
            args.remove(i);
            v
        } else {
            i += 1;
            continue;
        };

        format = match value.as_str() {
            "text" => DiagnosticOutputFormat::Text,
            "json" => DiagnosticOutputFormat::Json,
            "json-compact" => DiagnosticOutputFormat::JsonCompact,
            _ => {
                eprintln!("Error: --format expects one of: text, json, json-compact.");
                return None;
            }
        };
    }
    Some(format)
}

fn extract_max_errors(args: &mut Vec<String>) -> Option<usize> {
    let mut max_errors = DEFAULT_MAX_ERRORS;
    let mut i = 0;
    while i < args.len() {
        let value_str = if args[i] == "--max-errors" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --max-errors <n>");
                return None;
            }
            let v = args.remove(i + 1);
            args.remove(i);
            v
        } else if let Some(v) = args[i].strip_prefix("--max-errors=") {
            let v = v.to_string();
            args.remove(i);
            v
        } else {
            i += 1;
            continue;
        };
        match value_str.parse::<usize>() {
            Ok(parsed) => max_errors = parsed,
            Err(_) => {
                eprintln!("Error: --max-errors expects a non-negative integer.");
                return None;
            }
        }
    }
    Some(max_errors)
}

fn extract_test_filter(args: &mut Vec<String>) -> Option<Option<String>> {
    let mut test_filter: Option<String> = None;
    let mut i = 1usize;
    while i < args.len() {
        if args[i] == "--test-filter" {
            if i + 1 >= args.len() {
                eprintln!("Usage: flux <file.flx> --test --test-filter <pattern>");
                return None;
            }
            test_filter = Some(args.remove(i + 1));
            args.remove(i);
        } else if let Some(v) = args[i].strip_prefix("--test-filter=") {
            test_filter = Some(v.to_string());
            args.remove(i);
        } else {
            i += 1;
        }
    }
    Some(test_filter)
}

fn extract_roots(args: &mut Vec<String>, roots: &mut Vec<std::path::PathBuf>) -> bool {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--root" {
            if i + 1 >= args.len() {
                eprintln!(
                    "Usage: flux <file.flx> --root <path> [--root <path> ...]\n       flux run <file.flx> --root <path> [--root <path> ...]"
                );
                return false;
            }
            let path = args.remove(i + 1);
            args.remove(i);
            roots.push(std::path::PathBuf::from(path));
        } else if let Some(v) = args[i].strip_prefix("--root=") {
            let path = v.to_string();
            args.remove(i);
            roots.push(std::path::PathBuf::from(path));
        } else {
            i += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::{
        cli::cmdline::{
            CliCommand, extract_cache_dir, extract_diagnostic_format, extract_dump_core_mode,
            extract_max_errors, extract_output_path, extract_roots, extract_test_filter,
            parse_args, parse_fmt_command,
        },
        diagnostics::DEFAULT_MAX_ERRORS,
        driver::mode::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat, RunMode},
    };

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
                assert_eq!(flags.dump_core, CoreDumpMode::Debug);
                assert_eq!(flags.dump_aether, AetherDumpMode::Summary);
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
    fn parses_explicit_run_with_test_mode_and_filter() {
        let command = parse_args(cli(&[
            "flux",
            "run",
            "examples/basics/arithmetic.flx",
            "--test",
            "--test-filter=math",
        ]))
        .unwrap();

        match command {
            CliCommand::Run { flags, target } => {
                assert_eq!(target.mode, RunMode::Tests);
                assert_eq!(target.path, "examples/basics/arithmetic.flx");
                assert_eq!(flags.test_filter.as_deref(), Some("math"));
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn parses_clean_with_optional_input_path() {
        let command = parse_args(cli(&["flux", "clean", "examples/basics/arithmetic.flx"]))
            .unwrap();

        match command {
            CliCommand::Clean { flags } => {
                assert_eq!(
                    flags.input_path.as_deref(),
                    Some("examples/basics/arithmetic.flx")
                );
            }
            other => panic!("expected clean command, got {other:?}"),
        }
    }

    #[test]
    fn clean_ignores_non_flx_optional_argument() {
        let command = parse_args(cli(&["flux", "clean", "not-a-file"]))
            .unwrap();

        match command {
            CliCommand::Clean { flags } => {
                assert_eq!(flags.input_path, None);
            }
            other => panic!("expected clean command, got {other:?}"),
        }
    }

    #[test]
    fn parses_tail_call_alias() {
        let command = parse_args(cli(&[
            "flux",
            "tail-calls",
            "examples/basics/arithmetic.flx",
        ]))
        .unwrap();

        match command {
            CliCommand::AnalyzeTailCalls { flags } => {
                assert_eq!(
                    flags.input_path.as_deref(),
                    Some("examples/basics/arithmetic.flx")
                );
            }
            other => panic!("expected analyze-tail-calls command, got {other:?}"),
        }
    }

    #[test]
    fn parity_check_preserves_raw_args() {
        let command =
            parse_args(cli(&["flux", "parity-check", "tests/parity", "--ways", "vm,llvm"]))
                .unwrap();

        match command {
            CliCommand::ParityCheck { raw_args } => {
                assert_eq!(raw_args, vec!["tests/parity", "--ways", "vm,llvm"]);
            }
            other => panic!("expected parity-check command, got {other:?}"),
        }
    }

    #[test]
    fn extract_cache_dir_supports_split_and_equals_forms() {
        let mut split_args = vec![
            "flux".to_string(),
            "examples/basics/arithmetic.flx".to_string(),
            "--cache-dir".to_string(),
            "tmp/cache".to_string(),
        ];
        let mut equals_args = vec![
            "flux".to_string(),
            "examples/basics/arithmetic.flx".to_string(),
            "--cache-dir=tmp/cache".to_string(),
        ];

        let split = extract_cache_dir(&mut split_args).unwrap();
        let equals = extract_cache_dir(&mut equals_args).unwrap();

        assert_eq!(split.as_deref(), Some(std::path::Path::new("tmp/cache")));
        assert_eq!(equals.as_deref(), Some(std::path::Path::new("tmp/cache")));
        assert_eq!(split_args, vec!["flux", "examples/basics/arithmetic.flx"]);
        assert_eq!(equals_args, vec!["flux", "examples/basics/arithmetic.flx"]);
    }

    #[test]
    fn extract_output_path_consumes_flag_and_value() {
        let mut args = vec![
            "flux".to_string(),
            "examples/basics/arithmetic.flx".to_string(),
            "-o".to_string(),
            "out.ll".to_string(),
        ];

        let output = extract_output_path(&mut args);

        assert_eq!(output.as_deref(), Some("out.ll"));
        assert_eq!(args, vec!["flux", "examples/basics/arithmetic.flx"]);
    }

    #[test]
    fn extract_dump_core_mode_prefers_last_seen_value() {
        let mut args = vec![
            "flux".to_string(),
            "examples/basics/arithmetic.flx".to_string(),
            "--dump-core".to_string(),
            "--dump-core=debug".to_string(),
        ];

        let mode = extract_dump_core_mode(&mut args).unwrap();

        assert_eq!(mode, CoreDumpMode::Debug);
        assert_eq!(args, vec!["flux", "examples/basics/arithmetic.flx"]);
    }

    #[test]
    fn extract_diagnostic_format_supports_json_compact() {
        let mut args = vec![
            "flux".to_string(),
            "examples/basics/arithmetic.flx".to_string(),
            "--format=json-compact".to_string(),
        ];

        let format = extract_diagnostic_format(&mut args).unwrap();

        assert_eq!(format, DiagnosticOutputFormat::JsonCompact);
        assert_eq!(args, vec!["flux", "examples/basics/arithmetic.flx"]);
    }

    #[test]
    fn extract_max_errors_defaults_without_flag() {
        let mut args = vec!["flux".to_string(), "examples/basics/arithmetic.flx".to_string()];

        let max_errors = extract_max_errors(&mut args).unwrap();

        assert_eq!(max_errors, DEFAULT_MAX_ERRORS);
        assert_eq!(args, vec!["flux", "examples/basics/arithmetic.flx"]);
    }

    #[test]
    fn extract_test_filter_ignores_program_path_slot() {
        let mut args = vec![
            "flux".to_string(),
            "--test-filtered.flx".to_string(),
            "--test-filter".to_string(),
            "suite".to_string(),
        ];

        let filter = extract_test_filter(&mut args).unwrap();

        assert_eq!(filter.as_deref(), Some("suite"));
        assert_eq!(args, vec!["flux", "--test-filtered.flx"]);
    }

    #[test]
    fn extract_roots_collects_split_and_equals_forms() {
        let mut args = vec![
            "flux".to_string(),
            "examples/basics/arithmetic.flx".to_string(),
            "--root".to_string(),
            "tests/flux".to_string(),
            "--root=examples".to_string(),
        ];
        let mut roots = Vec::new();

        let ok = extract_roots(&mut args, &mut roots);

        assert!(ok);
        assert_eq!(
            roots,
            vec![
                std::path::PathBuf::from("tests/flux"),
                std::path::PathBuf::from("examples")
            ]
        );
        assert_eq!(args, vec!["flux", "examples/basics/arithmetic.flx"]);
    }

    #[test]
    fn parse_fmt_command_requires_path_after_check() {
        let err = parse_fmt_command(&[
            "flux".to_string(),
            "fmt".to_string(),
            "--check".to_string(),
        ])
        .unwrap_err();

        assert_eq!(err, "Usage: flux fmt --check <file.flx>");
    }

    #[test]
    fn parse_fmt_command_accepts_check_form() {
        let (path, check) = parse_fmt_command(&[
            "flux".to_string(),
            "fmt".to_string(),
            "--check".to_string(),
            "examples/basics/arithmetic.flx".to_string(),
        ])
        .unwrap();

        assert_eq!(path, "examples/basics/arithmetic.flx");
        assert!(check);
    }

    #[test]
    fn parse_args_preserves_output_path_on_run_command() {
        let command = parse_args(cli(&[
            "flux",
            "run",
            "examples/basics/arithmetic.flx",
            "-o",
            "out.ll",
        ]))
        .unwrap();

        match command {
            CliCommand::Run { flags, .. } => {
                assert_eq!(flags.output_path.as_deref(), Some("out.ll"));
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }

    #[test]
    fn parse_args_keeps_multiple_root_flags() {
        let command = parse_args(cli(&[
            "flux",
            "examples/basics/arithmetic.flx",
            "--root",
            "tests/flux",
            "--root=examples",
        ]))
        .unwrap();

        match command {
            CliCommand::Run { flags, .. } => {
                assert_eq!(
                    flags.roots,
                    vec![
                        std::path::PathBuf::from("tests/flux"),
                        std::path::PathBuf::from("examples")
                    ]
                );
            }
            other => panic!("expected run mode, got {other:?}"),
        }
    }
}
