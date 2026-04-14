//! Static and formatted text fragments used by CLI parsing and help output.

/// Returns the top-level help text shown by `flux --help`.
pub fn help_text() -> &'static str {
    "\
Flux CLI

Usage:
  flux <file.flx>
  flux run <file.flx>
  flux tokens <file.flx>
  flux bytecode <file.flx>
  flux lint <file.flx>
  flux fmt [--check] <file.flx>
  flux cache-info <file.flx>
  flux module-cache-info <file.flx>
  flux native-cache-info <file.flx>
  flux interface-info <file.flxi>
  flux clean [<file.flx>]
  flux analyze-free-vars <file.flx>
  flux analyze-tail-calls <file.flx>
  flux parity-check <file-or-dir> [--ways vm,llvm] [--root <path> ...]
  flux <file.flx> --root <path> [--root <path> ...]
  flux run <file.flx> --root <path> [--root <path> ...]

Flags:
  --verbose          Show cache status (hit/miss/store)
  --trace            Print VM instruction trace
  --trace-aether     Print Aether report plus backend/execution path, then run
  --test             Run test_* functions and report results
  --test-filter <s>  Only run tests whose names contain <s>
  --leak-detector    Print approximate allocation stats after run
  --no-cache         Disable bytecode cache for this run
  --cache-dir <dir>  Override cache root (default: nearest Cargo.toml target/flux, else .flux/cache)
  --optimize, -O     Enable AST optimizations (desugar + constant fold)
  --analyze, -A      Enable analysis passes (free vars + tail calls)
  --format <f>       Diagnostics format: text|json|json-compact (default: text)
  --max-errors <n>   Limit displayed errors (default: 50)
  --root <path>      Add a module root (can be repeated)
  --roots-only       Use only explicitly provided --root values
  --stats            Print execution analytics (parse/compile/execute times, module info)
  --prof             Print per-function profiling report (call counts, time, allocations)
  --strict           Enable strict type/effect boundary checks
  --all-errors       Show diagnostics from all phases (disable stage-aware filtering)
  --dump-repr        Print the backend representation contract summary and exit
  --dump-cfg         Lower to Flux CFG IR, print a readable dump, and exit
  --dump-core        Lower to Flux Core IR, print a readable dump, and exit
  --dump-core=debug  Lower to Flux Core IR, print a raw debug dump, and exit
  --dump-aether      Show Aether memory model report (per-function reuse/drop stats)
  --dump-aether=debug
                    Show detailed Aether debug report (borrow signatures, call modes, dup/drop, reuse)
  --native           Compile via Core IR -> LLVM text IR -> native binary (requires LLVM tools)
  --core-to-llvm     Alias for --native
  --emit-llvm        Emit LLVM IR text (.ll) to stdout (with --native)
  --emit-binary      Compile to native binary via opt + llc + cc (with --native)
  -o <path>          Output path for --emit-llvm or --emit-binary
  -h, --help         Show this help message

Optimization & Analysis:
  --optimize         Apply transformations (faster bytecode)
  --analyze          Collect analysis data (free vars, tail calls)
  -O -A              Both optimization and analysis
"
}

/// Returns the `fmt` usage text shown when formatter arguments are missing.
pub fn fmt_usage() -> &'static str {
    "Usage: flux fmt [--check] <file.flx>"
}

/// Returns the `fmt --check` usage text shown when the checked formatter path is missing.
pub fn fmt_check_usage() -> &'static str {
    "Usage: flux fmt --check <file.flx>"
}

/// Formats the error shown when a command expects a `.flx` source file.
pub fn expected_flx(path: &str) -> String {
    format!(
        "Error: expected a `.flx` file, got `{path}`. Pass a Flux source file like `path/to/file.flx`."
    )
}

/// Formats the error shown when a command expects a `.flxi` interface file.
pub fn expected_flxi(path: &str) -> String {
    format!(
        "Error: expected a `.flxi` file, got `{path}`. Pass a Flux interface file like `path/to/module.flxi`."
    )
}

/// Formats the error shown when the CLI receives an unknown command token.
pub fn unknown_command(command: &str) -> String {
    format!(
        "Error: unknown command or invalid input `{command}`. Pass a `.flx` file or a valid subcommand."
    )
}

#[cfg(test)]
mod tests {
    use super::{
        expected_flx, expected_flxi, fmt_check_usage, fmt_usage, help_text, unknown_command,
    };

    #[test]
    fn help_text_mentions_core_commands_and_flags() {
        let text = help_text();

        assert!(text.contains("flux run <file.flx>"));
        assert!(text.contains("--dump-core"));
        assert!(text.contains("--format <f>"));
    }

    #[test]
    fn expected_flx_message_mentions_bad_path_and_example() {
        let message = expected_flx("bad.txt");

        assert!(message.contains("bad.txt"));
        assert!(message.contains("file.flx"));
    }

    #[test]
    fn expected_flxi_message_mentions_bad_path_and_example() {
        let message = expected_flxi("bad.txt");

        assert!(message.contains("bad.txt"));
        assert!(message.contains("module.flxi"));
    }

    #[test]
    fn fmt_usage_messages_match_cli_contract() {
        assert!(fmt_usage().contains("flux fmt"));
        assert!(fmt_check_usage().contains("--check"));
    }

    #[test]
    fn unknown_command_message_mentions_bad_token() {
        let message = unknown_command("wat");

        assert!(message.contains("wat"));
        assert!(message.contains("valid subcommand"));
    }
}
