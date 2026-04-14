//! Shared CLI parsing helpers used by the command-line entrypoint.
//!
//! This module keeps argument extraction allocation-light by mutating the argv buffer in place
//! and by grouping related flags into compact structs before they are converted into
//! `DriverFlags`.

use std::{path::PathBuf, str::FromStr};

use crate::{
    diagnostics::DEFAULT_MAX_ERRORS,
    driver::{
        AetherDumpMode, CoreDumpMode,
        backend::Backend,
        flags::{
            DriverBackendFlags, DriverCacheFlags, DriverDiagnosticFlags, DriverDumpFlags,
            DriverFlags, DriverInputFlags, DriverLanguageFlags, DriverRuntimeFlags,
        },
        mode::DiagnosticOutputFormat,
    },
};

/// Parsed backend-related CLI flags that affect backend selection or backend outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ParsedCliBackendFlags {
    pub(crate) use_core_to_llvm: bool,
    pub(crate) emit_llvm: bool,
    pub(crate) emit_binary: bool,
}

/// Parsed runtime-related CLI flags that affect execution-time reporting or tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ParsedCliRuntimeFlags {
    pub(crate) verbose: bool,
    pub(crate) leak_detector: bool,
    pub(crate) trace: bool,
    pub(crate) trace_aether: bool,
    pub(crate) show_stats: bool,
    pub(crate) profiling: bool,
}

/// Parsed language-related CLI flags that affect compilation semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ParsedCliLanguageFlags {
    pub(crate) enable_optimize: bool,
    pub(crate) enable_analyze: bool,
    pub(crate) strict_mode: bool,
    pub(crate) strict_types: bool,
}

/// Parsed dump-related CLI flags that select dump-only execution surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParsedCliDumpFlags {
    pub(crate) dump_repr: bool,
    pub(crate) dump_cfg: bool,
    pub(crate) dump_aether: AetherDumpMode,
    pub(crate) dump_lir: bool,
    pub(crate) dump_lir_llvm: bool,
}

impl Default for ParsedCliDumpFlags {
    /// Returns the dump flag set with every dump surface disabled.
    fn default() -> Self {
        Self {
            dump_repr: false,
            dump_cfg: false,
            dump_aether: AetherDumpMode::None,
            dump_lir: false,
            dump_lir_llvm: false,
        }
    }
}

/// Parsed execution-policy flags that do not fit the backend/runtime/language groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ParsedCliExecutionFlags {
    pub(crate) no_cache: bool,
    pub(crate) roots_only: bool,
    pub(crate) test_mode: bool,
    pub(crate) all_errors: bool,
}

/// Grouped boolean and mode-like CLI flags extracted from argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ParsedCliFlags {
    pub(crate) backend: ParsedCliBackendFlags,
    pub(crate) runtime: ParsedCliRuntimeFlags,
    pub(crate) language: ParsedCliLanguageFlags,
    pub(crate) dumps: ParsedCliDumpFlags,
    pub(crate) execution: ParsedCliExecutionFlags,
}

/// Value-carrying CLI options extracted from argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliValueOptions {
    pub(crate) paths: CliPathOptions,
    pub(crate) diagnostics: CliDiagnosticOptions,
    pub(crate) command: CliCommandValueOptions,
}

/// Path-like CLI values collected during value-option parsing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CliPathOptions {
    pub(crate) cache_dir: Option<PathBuf>,
    pub(crate) output_path: Option<String>,
    pub(crate) roots: Vec<PathBuf>,
}

/// Diagnostic-related CLI values collected during value-option parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CliDiagnosticOptions {
    pub(crate) diagnostics_format: DiagnosticOutputFormat,
    pub(crate) max_errors: usize,
}

/// Command-specific CLI values that are neither paths nor diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliCommandValueOptions {
    pub(crate) dump_core: CoreDumpMode,
    pub(crate) test_filter: Option<String>,
}

impl Default for CliDiagnosticOptions {
    /// Returns diagnostic option defaults used by the CLI parser.
    fn default() -> Self {
        Self {
            diagnostics_format: DiagnosticOutputFormat::Text,
            max_errors: DEFAULT_MAX_ERRORS,
        }
    }
}

impl Default for CliCommandValueOptions {
    /// Returns command-specific option defaults used by the CLI parser.
    fn default() -> Self {
        Self {
            dump_core: CoreDumpMode::None,
            test_filter: None,
        }
    }
}

impl Default for CliValueOptions {
    /// Returns the value-option set with every option left at its CLI default.
    fn default() -> Self {
        Self {
            paths: CliPathOptions::default(),
            diagnostics: CliDiagnosticOptions::default(),
            command: CliCommandValueOptions::default(),
        }
    }
}

/// Extracts grouped boolean and mode-like CLI flags in a single pass, removing them from argv.
pub(crate) fn extract_cli_flag_groups(args: &mut Vec<String>) -> ParsedCliFlags {
    let mut flags = ParsedCliFlags::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--verbose" => flags.runtime.verbose = remove_bool_flag(args, i),
            "--leak-detector" => flags.runtime.leak_detector = remove_bool_flag(args, i),
            "--trace" => flags.runtime.trace = remove_bool_flag(args, i),
            "--trace-aether" => flags.runtime.trace_aether = remove_bool_flag(args, i),
            "--prof" => flags.runtime.profiling = remove_bool_flag(args, i),
            "--roots-only" => flags.execution.roots_only = remove_bool_flag(args, i),
            "--optimize" | "-O" => flags.language.enable_optimize = remove_bool_flag(args, i),
            "--analyze" | "-A" => flags.language.enable_analyze = remove_bool_flag(args, i),
            "--stats" => flags.runtime.show_stats = remove_bool_flag(args, i),
            "--test" => flags.execution.test_mode = remove_bool_flag(args, i),
            "--strict" => flags.language.strict_mode = remove_bool_flag(args, i),
            "--strict-types" => flags.language.strict_types = remove_bool_flag(args, i),
            "--no-strict" => {
                args.remove(i);
            }
            "--all-errors" => flags.execution.all_errors = remove_bool_flag(args, i),
            "--dump-repr" => flags.dumps.dump_repr = remove_bool_flag(args, i),
            "--dump-cfg" => flags.dumps.dump_cfg = remove_bool_flag(args, i),
            "--dump-aether" => {
                flags.dumps.dump_aether = AetherDumpMode::Summary;
                args.remove(i);
            }
            "--dump-aether=debug" => {
                flags.dumps.dump_aether = AetherDumpMode::Debug;
                args.remove(i);
            }
            "--dump-lir" => flags.dumps.dump_lir = remove_bool_flag(args, i),
            "--dump-lir-llvm" => {
                #[cfg(feature = "core_to_llvm")]
                {
                    flags.dumps.dump_lir_llvm = remove_bool_flag(args, i);
                }
                #[cfg(not(feature = "core_to_llvm"))]
                {
                    args.remove(i);
                }
            }
            "--core-to-llvm" | "--native" => {
                #[cfg(feature = "native")]
                {
                    flags.backend.use_core_to_llvm = remove_bool_flag(args, i);
                }
                #[cfg(not(feature = "native"))]
                {
                    args.remove(i);
                }
            }
            "--emit-llvm" => flags.backend.emit_llvm = remove_bool_flag(args, i),
            "--emit-binary" => flags.backend.emit_binary = remove_bool_flag(args, i),
            "--no-cache" => flags.execution.no_cache = remove_bool_flag(args, i),
            _ => {
                i += 1;
            }
        }
    }

    if flags.runtime.profiling {
        // Profiling requires fresh execution state, so it implies cache bypass.
        flags.execution.no_cache = true;
    }

    flags
}

/// Extracts value-carrying CLI options in a single pass, removing them from argv.
///
/// This keeps parsing allocation-light by mutating the existing argument buffer rather than
/// rebuilding intermediate collections for each option family.
pub(crate) fn extract_cli_value_options(args: &mut Vec<String>) -> Result<CliValueOptions, String> {
    let mut values = CliValueOptions::default();
    let mut i = 0;
    while i < args.len() {
        if let Some(value) = take_required_long_option(
            args,
            &mut i,
            "--cache-dir",
            "Error: --cache-dir requires a directory path.",
        )? {
            values.paths.cache_dir = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = consume_short_value_option(args, &mut i, "-o")? {
            values.paths.output_path = Some(value);
            continue;
        }
        if let Some(mode) = consume_dump_core_option(args, &mut i)? {
            values.command.dump_core = mode;
            continue;
        }
        if let Some(format) = consume_named_value_option(
            args,
            &mut i,
            "--format",
            "Usage: flux <file.flx> --format <text|json|json-compact>",
            parse_diagnostic_format,
        )? {
            values.diagnostics.diagnostics_format = format;
            continue;
        }
        if let Some(max_errors) = consume_named_value_option(
            args,
            &mut i,
            "--max-errors",
            "Usage: flux <file.flx> --max-errors <n>",
            parse_max_errors,
        )? {
            values.diagnostics.max_errors = max_errors;
            continue;
        }
        if let Some(filter) = take_required_long_option(
            args,
            &mut i,
            "--test-filter",
            "Usage: flux <file.flx> --test --test-filter <pattern>",
        )? {
            values.command.test_filter = Some(filter);
            continue;
        }
        if let Some(root) = take_required_long_option(
            args,
            &mut i,
            "--root",
            "Usage: flux <file.flx> --root <path> [--root <path> ...]",
        )? {
            values.paths.roots.push(PathBuf::from(root));
            continue;
        }
        i += 1;
    }
    Ok(values)
}

/// Builds grouped driver flags from parsed CLI flag groups and value options.
pub(crate) fn build_driver_flags(parsed: ParsedCliFlags, values: CliValueOptions) -> DriverFlags {
    DriverFlags {
        backend: DriverBackendFlags {
            selected: Backend::Vm,
            use_core_to_llvm: parsed.backend.use_core_to_llvm,
            emit_llvm: parsed.backend.emit_llvm,
            emit_binary: parsed.backend.emit_binary,
            output_path: values.paths.output_path,
        },
        input: DriverInputFlags {
            input_path: None,
            roots: values.paths.roots,
            roots_only: parsed.execution.roots_only,
            test_filter: values.command.test_filter,
        },
        runtime: DriverRuntimeFlags {
            verbose: parsed.runtime.verbose,
            leak_detector: parsed.runtime.leak_detector,
            trace: parsed.runtime.trace,
            trace_aether: parsed.runtime.trace_aether,
            show_stats: parsed.runtime.show_stats,
            profiling: parsed.runtime.profiling,
        },
        dumps: DriverDumpFlags {
            dump_repr: parsed.dumps.dump_repr,
            dump_cfg: parsed.dumps.dump_cfg,
            dump_core: values.command.dump_core,
            dump_aether: parsed.dumps.dump_aether,
            dump_lir: parsed.dumps.dump_lir,
            dump_lir_llvm: parsed.dumps.dump_lir_llvm,
        },
        diagnostics: DriverDiagnosticFlags {
            max_errors: values.diagnostics.max_errors,
            diagnostics_format: values.diagnostics.diagnostics_format,
            all_errors: parsed.execution.all_errors,
        },
        cache: DriverCacheFlags {
            cache_dir: values.paths.cache_dir,
            no_cache: parsed.execution.no_cache,
        },
        language: DriverLanguageFlags {
            enable_optimize: parsed.language.enable_optimize,
            enable_analyze: parsed.language.enable_analyze,
            strict_mode: parsed.language.strict_mode,
            strict_types: parsed.language.strict_types,
        },
    }
    .finalize_backend()
}

/// Removes a present boolean flag from the mutable argument buffer and returns `true`.
fn remove_bool_flag(args: &mut Vec<String>, index: usize) -> bool {
    args.remove(index);
    true
}

/// Consumes a `--flag value` or `--flag=value` long option and returns its string payload.
///
/// The matched option is removed from `args` so subsequent parsing passes see only unhandled
/// positional arguments and options.
fn take_required_long_option(
    args: &mut Vec<String>,
    index: &mut usize,
    flag: &str,
    missing_value: &str,
) -> Result<Option<String>, String> {
    if args[*index] == flag {
        args.remove(*index);
        if *index < args.len() {
            return Ok(Some(args.remove(*index)));
        }
        return Err(missing_value.to_string());
    }
    if let Some(value) = args[*index].strip_prefix(&format!("{flag}=")) {
        let value = value.to_string();
        args.remove(*index);
        return Ok(Some(value));
    }
    Ok(None)
}

/// Consumes a named long option and parses its value with the provided parser.
fn consume_named_value_option<T>(
    args: &mut Vec<String>,
    index: &mut usize,
    flag: &str,
    missing_value: &str,
    parse: impl FnOnce(String) -> Result<T, String>,
) -> Result<Option<T>, String> {
    take_required_long_option(args, index, flag, missing_value)?
        .map(parse)
        .transpose()
}

/// Consumes a short option like `-o value` and returns the provided value.
///
/// This helper is intentionally narrow because the CLI only accepts a small set of short options
/// and treating them explicitly keeps parsing logic cheap and easy to follow.
fn consume_short_value_option(
    args: &mut Vec<String>,
    index: &mut usize,
    flag: &str,
) -> Result<Option<String>, String> {
    if args[*index] != flag {
        return Ok(None);
    }
    args.remove(*index);
    if *index < args.len() {
        return Ok(Some(args.remove(*index)));
    }
    Err(format!("Error: {flag} requires an output path argument."))
}

/// Consumes `--dump-core` or `--dump-core=debug` and returns the requested dump mode.
///
/// The parser rejects every other value so dump-surface selection stays aligned with the
/// supported Core dump modes.
fn consume_dump_core_option(
    args: &mut Vec<String>,
    index: &mut usize,
) -> Result<Option<CoreDumpMode>, String> {
    if args[*index] == "--dump-core" {
        args.remove(*index);
        return Ok(Some(CoreDumpMode::Readable));
    }
    let Some(value) = args[*index].strip_prefix("--dump-core=") else {
        return Ok(None);
    };
    let mode = match value {
        "debug" => CoreDumpMode::Debug,
        "" => return Err("Error: --dump-core expects no value or `debug`.".to_string()),
        _ => return Err("Error: --dump-core expects no value or `debug`.".to_string()),
    };
    args.remove(*index);
    Ok(Some(mode))
}

/// Parses a diagnostic format token into a concrete driver rendering mode.
fn parse_diagnostic_format(value: String) -> Result<DiagnosticOutputFormat, String> {
    let format = match value.as_str() {
        "text" => DiagnosticOutputFormat::Text,
        "json" => DiagnosticOutputFormat::Json,
        "json-compact" => DiagnosticOutputFormat::JsonCompact,
        _ => return Err("Error: --format expects one of: text, json, json-compact.".to_string()),
    };
    Ok(format)
}

/// Parses the `--max-errors` payload into a concrete error limit.
fn parse_max_errors(value: String) -> Result<usize, String> {
    usize::from_str(&value)
        .map_err(|_| "Error: --max-errors expects a non-negative integer.".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        CliCommandValueOptions, CliDiagnosticOptions, CliPathOptions, ParsedCliBackendFlags,
        ParsedCliDumpFlags, ParsedCliExecutionFlags, ParsedCliFlags, ParsedCliLanguageFlags,
        ParsedCliRuntimeFlags, build_driver_flags, extract_cli_flag_groups,
        extract_cli_value_options,
    };
    #[cfg(feature = "core_to_llvm")]
    use crate::driver::backend::Backend;
    use crate::driver::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat};
    use std::path::Path;

    #[test]
    fn extract_flags_groups_boolean_switches_and_aliases() {
        let mut args = vec![
            "flux".into(),
            "file.flx".into(),
            "--verbose".into(),
            "-O".into(),
            "--dump-aether=debug".into(),
            "--emit-binary".into(),
            "--prof".into(),
        ];

        let parsed = extract_cli_flag_groups(&mut args);

        assert!(parsed.runtime.verbose);
        assert!(parsed.language.enable_optimize);
        assert_eq!(parsed.dumps.dump_aether, AetherDumpMode::Debug);
        assert!(parsed.backend.emit_binary);
        assert!(parsed.runtime.profiling);
        assert!(parsed.execution.no_cache);
        assert_eq!(args, vec!["flux".to_string(), "file.flx".to_string()]);
    }

    #[test]
    fn extract_value_options_handles_mixed_value_forms() {
        let mut args = vec![
            "flux".into(),
            "run".into(),
            "file.flx".into(),
            "--cache-dir=.flux-cache".into(),
            "--format".into(),
            "json-compact".into(),
            "--max-errors=12".into(),
            "--test-filter".into(),
            "arith".into(),
            "--root=examples".into(),
            "--root".into(),
            "tests".into(),
            "--dump-core=debug".into(),
            "-o".into(),
            "out.ll".into(),
        ];

        let values = extract_cli_value_options(&mut args).unwrap();

        assert_eq!(
            values.paths.cache_dir.as_deref(),
            Some(Path::new(".flux-cache"))
        );
        assert_eq!(
            values.diagnostics.diagnostics_format,
            DiagnosticOutputFormat::JsonCompact
        );
        assert_eq!(values.diagnostics.max_errors, 12);
        assert_eq!(values.command.test_filter.as_deref(), Some("arith"));
        assert_eq!(values.command.dump_core, CoreDumpMode::Debug);
        assert_eq!(values.paths.output_path.as_deref(), Some("out.ll"));
        assert_eq!(values.paths.roots.len(), 2);
        assert_eq!(
            args,
            vec![
                "flux".to_string(),
                "run".to_string(),
                "file.flx".to_string()
            ]
        );
    }

    #[test]
    fn extract_value_options_rejects_missing_cache_dir_value() {
        let mut args = vec!["flux".into(), "file.flx".into(), "--cache-dir".into()];

        let err = extract_cli_value_options(&mut args).unwrap_err();

        assert!(err.contains("--cache-dir"));
    }

    #[test]
    fn extract_value_options_rejects_invalid_max_errors() {
        let mut args = vec!["flux".into(), "file.flx".into(), "--max-errors=abc".into()];

        let err = extract_cli_value_options(&mut args).unwrap_err();

        assert!(err.contains("--max-errors"));
    }

    #[test]
    fn extract_value_options_rejects_invalid_dump_core_value() {
        let mut args = vec!["flux".into(), "file.flx".into(), "--dump-core=raw".into()];

        let err = extract_cli_value_options(&mut args).unwrap_err();

        assert!(err.contains("--dump-core"));
    }

    #[test]
    fn extract_value_options_rejects_invalid_diagnostics_format() {
        let mut args = vec!["flux".into(), "file.flx".into(), "--format=yaml".into()];

        let err = extract_cli_value_options(&mut args).unwrap_err();

        assert!(err.contains("--format"));
    }

    #[test]
    fn extract_value_options_rejects_missing_root_value() {
        let mut args = vec!["flux".into(), "file.flx".into(), "--root".into()];

        let err = extract_cli_value_options(&mut args).unwrap_err();

        assert!(err.contains("--root"));
    }

    #[test]
    fn extract_value_options_rejects_missing_test_filter_value() {
        let mut args = vec!["flux".into(), "file.flx".into(), "--test-filter".into()];

        let err = extract_cli_value_options(&mut args).unwrap_err();

        assert!(err.contains("--test-filter"));
    }

    #[test]
    fn extract_value_options_rejects_missing_output_value() {
        let mut args = vec!["flux".into(), "file.flx".into(), "-o".into()];

        let err = extract_cli_value_options(&mut args).unwrap_err();

        assert!(err.contains("-o"));
    }

    #[test]
    fn build_driver_flags_preserves_grouped_flag_layout() {
        let flags = build_driver_flags(
            ParsedCliFlags {
                runtime: ParsedCliRuntimeFlags {
                    verbose: true,
                    profiling: true,
                    ..ParsedCliRuntimeFlags::default()
                },
                language: ParsedCliLanguageFlags {
                    enable_optimize: true,
                    ..ParsedCliLanguageFlags::default()
                },
                dumps: ParsedCliDumpFlags {
                    dump_cfg: true,
                    ..ParsedCliDumpFlags::default()
                },
                backend: ParsedCliBackendFlags {
                    emit_llvm: true,
                    ..ParsedCliBackendFlags::default()
                },
                execution: ParsedCliExecutionFlags {
                    no_cache: true,
                    ..ParsedCliExecutionFlags::default()
                },
            },
            super::CliValueOptions {
                paths: CliPathOptions {
                    output_path: Some("out.ll".into()),
                    ..CliPathOptions::default()
                },
                diagnostics: CliDiagnosticOptions::default(),
                command: CliCommandValueOptions::default(),
            },
        );

        assert!(flags.runtime.verbose);
        assert!(flags.language.enable_optimize);
        assert!(flags.dumps.dump_cfg);
        assert!(flags.backend.emit_llvm);
        assert!(flags.cache.no_cache);
        #[cfg(feature = "core_to_llvm")]
        assert_eq!(flags.backend.selected, Backend::Native);
    }
}
