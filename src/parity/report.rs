//! Structured parity mismatch reporting with optional color output.

use super::{MismatchDetail, ParityResult, Verdict, Way};

/// Filter which results to display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayFilter {
    All,
    PassOnly,
    FailOnly,
}

/// Whether to use ANSI color codes in output.
fn use_color() -> bool {
    std::env::var("NO_COLOR").is_err()
}

// ── ANSI helpers ───────────────────────────────────────────────────────────

fn green(s: &str) -> String {
    if use_color() {
        format!("\x1b[0;32m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn red(s: &str) -> String {
    if use_color() {
        format!("\x1b[0;31m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn yellow(s: &str) -> String {
    if use_color() {
        format!("\x1b[0;33m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn cyan(s: &str) -> String {
    if use_color() {
        format!("\x1b[0;36m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

// ── Per-file reporting ─────────────────────────────────────────────────────

/// Print the result for a single fixture, respecting the display filter.
pub fn print_result(result: &ParityResult, filter: DisplayFilter) {
    let dominated = matches!(
        (&result.verdict, filter),
        (Verdict::Pass, DisplayFilter::FailOnly)
            | (Verdict::Mismatch { .. }, DisplayFilter::PassOnly)
            | (Verdict::Skip { .. }, DisplayFilter::PassOnly)
    );
    if dominated {
        return;
    }

    let name = result.file.display();

    // Print copy-pasteable cargo run commands for each way
    for run in &result.results {
        let label = cyan(&format!("{}:", run.way));
        let cmd = cargo_run_for_way(run.way, &result.file.to_string_lossy());
        println!("{label:>14} {cmd}");
    }

    match &result.verdict {
        Verdict::Pass => {
            println!("{} {name}", green("PASS"));
        }
        Verdict::Skip { reason } => {
            println!("{} {name} ({reason})", yellow("SKIP"));
        }
        Verdict::Mismatch { details } => {
            println!("{} {name}", red("MISMATCH"));
            if let Some(summary) = diagnose_mismatch(details) {
                println!("  {} {summary}", cyan("diagnosis:"));
            }
            for detail in details {
                print_mismatch_detail(detail);
            }
        }
    }
    println!();
}

/// Print extra debug detail for the first failure when requested.
pub fn print_debug_first_failure(result: &ParityResult) {
    match &result.verdict {
        Verdict::Pass => return,
        Verdict::Skip { reason } => {
            println!("{} {}", cyan("debug:"), result.file.display());
            println!("  {} {reason}", cyan("skip_reason:"));
        }
        Verdict::Mismatch { details } => {
            println!("{} {}", cyan("debug:"), result.file.display());
            if let Some(summary) = diagnose_mismatch(details) {
                println!("  {} {summary}", cyan("diagnosis:"));
            }
            for detail in details {
                if let MismatchDetail::CoreMismatch {
                    left_way,
                    left,
                    right_way,
                    right,
                } = detail
                {
                    println!("  {}", cyan("core_mismatch_summary:"));
                    print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
                }
            }
            for run in &result.results {
                println!(
                    "  {} {} exit={} code={}",
                    cyan("way:"),
                    run.way,
                    run.exit_kind,
                    run.exit_code
                );
                if !run.stderr.trim().is_empty() {
                    println!("  {}", cyan("stderr:"));
                    print_block(&run.stderr);
                }
                if !run.stdout.trim().is_empty() {
                    println!("  {}", cyan("stdout:"));
                    print_block_limited(&run.stdout, 16);
                }
            }
            if !result.artifacts.is_empty() {
                for (way, arts) in &result.artifacts {
                    let core_state = if arts.normalized_dump_core.is_some() {
                        "captured"
                    } else {
                        "none"
                    };
                    let aether_state = if arts.normalized_dump_aether.is_some() {
                        "captured"
                    } else {
                        "none"
                    };
                    println!(
                        "  {} {} core={} aether={}",
                        cyan("artifacts:"),
                        way,
                        core_state,
                        aether_state
                    );
                }
            }
        }
    }
    println!();
}

pub fn diagnose_mismatch(details: &[MismatchDetail]) -> Option<&'static str> {
    let has_core = details.iter().any(|d| matches!(d, MismatchDetail::CoreMismatch { .. }));
    let has_aether = details.iter().any(|d| matches!(d, MismatchDetail::AetherMismatch { .. }));
    let has_backend_surface = details.iter().any(|d| {
        matches!(
            d,
            MismatchDetail::ExitKind { .. }
                | MismatchDetail::Stdout { .. }
                | MismatchDetail::Stderr { .. }
        )
    });
    let has_cache = details
        .iter()
        .any(|d| matches!(d, MismatchDetail::CacheMismatch { .. }));
    let has_strict = details
        .iter()
        .any(|d| matches!(d, MismatchDetail::StrictModeMismatch { .. }));

    if has_cache {
        return Some("cache behavior diverged between fresh and cached ways");
    }
    if has_strict {
        return Some("strict-mode behavior diverged from non-strict behavior");
    }
    if has_core {
        return Some("likely frontend/Core lowering divergence before backend execution");
    }
    if has_aether && has_backend_surface {
        return Some("likely ownership/Aether lowering bug affecting backend behavior");
    }
    if has_aether {
        return Some("likely ownership/Aether lowering divergence");
    }
    if has_backend_surface {
        return Some("likely backend/runtime divergence; capture Core to confirm frontend parity");
    }
    None
}

/// Build a copy-pasteable `cargo run` command for a given way.
pub fn cargo_run_for_way(way: Way, file: &str) -> String {
    match way {
        Way::Vm => format!("cargo run -- {file} --no-cache"),
        Way::Llvm => {
            format!("cargo run --features core_to_llvm -- {file} --native --no-cache")
        }
        Way::VmCached => format!("cargo run -- {file}"),
        Way::LlvmCached => format!("cargo run --features core_to_llvm -- {file} --native"),
        Way::VmStrict => format!("cargo run -- {file} --strict --no-cache"),
        Way::LlvmStrict => {
            format!("cargo run --features core_to_llvm -- {file} --native --strict --no-cache")
        }
    }
}

fn print_mismatch_detail(detail: &MismatchDetail) {
    match detail {
        MismatchDetail::ExitKind {
            left_way,
            left,
            right_way,
            right,
        } => {
            println!(
                "  {} {left_way}={left}, {right_way}={right}",
                cyan("exit_kind:")
            );
        }
        MismatchDetail::Stdout {
            left_way,
            left,
            right_way,
            right,
        } => {
            println!("  {}", cyan("stdout differs:"));
            print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
        }
        MismatchDetail::Stderr {
            left_way,
            left,
            right_way,
            right,
        } => {
            println!("  {}", cyan("stderr differs:"));
            print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
        }
        MismatchDetail::CoreMismatch {
            left_way,
            left,
            right_way,
            right,
        } => {
            println!(
                "  {} Core IR differs (semantic IR divergence)",
                cyan("core_mismatch:")
            );
            print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
        }
        MismatchDetail::AetherMismatch {
            left_way,
            left,
            right_way,
            right,
        } => {
            println!(
                "  {} Aether ownership differs (dup/drop/reuse divergence)",
                cyan("aether_mismatch:")
            );
            print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
        }
        MismatchDetail::CacheMismatch {
            fresh_way,
            cached_way,
            field,
            fresh,
            cached,
        } => {
            println!(
                "  {} fresh {} vs cached {} differ on {field}",
                cyan("cache_mismatch:"),
                fresh_way,
                cached_way
            );
            print_inline_diff(
                format!("{fresh_way} (fresh)"),
                fresh,
                format!("{cached_way} (cached)"),
                cached,
            );
        }
        MismatchDetail::StrictModeMismatch {
            normal_way,
            strict_way,
            field,
            normal,
            strict,
        } => {
            println!(
                "  {} {normal_way} vs {strict_way} differ on {field}",
                cyan("strict_mode_mismatch:")
            );
            print_inline_diff(
                format!("{normal_way}"),
                normal,
                format!("{strict_way} (strict)"),
                strict,
            );
        }
    }
}

fn print_inline_diff(left_label: String, left: &str, right_label: String, right: &str) {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();

    println!("    --- {left_label}");
    println!("    +++ {right_label}");

    let max_lines = left_lines.len().max(right_lines.len()).min(12);

    for i in 0..max_lines {
        let l = left_lines.get(i).copied().unwrap_or("");
        let r = right_lines.get(i).copied().unwrap_or("");
        if l != r {
            if !l.is_empty() {
                println!("    {}", red(&format!("-{l}")));
            }
            if !r.is_empty() {
                println!("    {}", green(&format!("+{r}")));
            }
        } else {
            println!("     {l}");
        }
    }

    let total = left_lines.len().max(right_lines.len());
    if total > max_lines {
        println!("    ... ({} more lines)", total - max_lines);
    }
}

fn print_block(text: &str) {
    for line in text.lines() {
        println!("    {line}");
    }
}

fn print_block_limited(text: &str, max_lines: usize) {
    let lines: Vec<&str> = text.lines().collect();
    for line in lines.iter().take(max_lines) {
        println!("    {line}");
    }
    if lines.len() > max_lines {
        println!("    ... ({} more lines)", lines.len() - max_lines);
    }
}

// ── Summary ────────────────────────────────────────────────────────────────

/// Print the aggregate summary across all fixtures.
pub fn print_summary(results: &[ParityResult]) {
    let total = results.len();
    let pass = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Pass))
        .count();
    let mismatch = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Mismatch { .. }))
        .count();
    let skip = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Skip { .. }))
        .count();

    println!();
    println!("=== Parity Results ===");
    println!("Total:    {total}");
    println!("Pass:     {}", green(&pass.to_string()));
    println!("Mismatch: {}", red(&mismatch.to_string()));
    println!("Skip:     {}", yellow(&skip.to_string()));

    if mismatch == 0 && pass > 0 {
        println!();
        println!("{}", green("All compiled examples match!"));
    }
}
