//! Structured parity mismatch reporting with optional color output.

use super::{MismatchDetail, ParityResult, Verdict, Way, backend_spec};
use std::collections::BTreeSet;
use std::fs;

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
pub fn print_result(result: &ParityResult, filter: DisplayFilter, explain: bool) {
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
            print_cache_summary(result);
        }
        Verdict::Skip { reason } => {
            println!("{} {name} ({reason})", yellow("SKIP"));
            print_cache_summary(result);
        }
        Verdict::Mismatch { details } => {
            println!("{} {name}", red("MISMATCH"));
            if let Some(summary) = diagnose_mismatch(details) {
                println!("  {} {summary}", cyan("diagnosis:"));
            }
            if explain {
                print_explain_block(result);
            }
            for detail in details {
                print_mismatch_detail(detail);
            }
            print_cache_summary(result);
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
                match detail {
                    MismatchDetail::CoreMismatch {
                        left_way,
                        left,
                        right_way,
                        right,
                    } => {
                        println!("  {}", cyan("core_mismatch_summary:"));
                        print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
                    }
                    MismatchDetail::RepresentationMismatch {
                        left_way,
                        left,
                        right_way,
                        right,
                    } => {
                        println!("  {}", cyan("representation_mismatch_summary:"));
                        print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
                    }
                    _ => {}
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
                    let core_state = if arts.core.normalized.is_some() {
                        "captured"
                    } else {
                        "none"
                    };
                    let aether_state = if arts.aether.normalized.is_some() {
                        "captured"
                    } else {
                        "none"
                    };
                    let repr_state = if arts.repr.normalized.is_some() {
                        "captured"
                    } else {
                        "none"
                    };
                    println!(
                        "  {} {} core={} aether={} repr={} backend_ir={}",
                        cyan("artifacts:"),
                        way,
                        core_state,
                        aether_state,
                        repr_state,
                        arts.backend_ir.len()
                    );
                }
            }
        }
    }
    println!();
}

pub fn diagnose_mismatch(details: &[MismatchDetail]) -> Option<&'static str> {
    let has_core = details
        .iter()
        .any(|d| matches!(d, MismatchDetail::CoreMismatch { .. }));
    let has_aether = details
        .iter()
        .any(|d| matches!(d, MismatchDetail::AetherMismatch { .. }));
    let has_repr = details
        .iter()
        .any(|d| matches!(d, MismatchDetail::RepresentationMismatch { .. }));
    let has_backend_ir = details
        .iter()
        .any(|d| matches!(d, MismatchDetail::BackendIrMismatch { .. }));
    let has_backend_runtime = details
        .iter()
        .any(|d| matches!(d, MismatchDetail::BackendRuntimeMismatch { .. }));
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
        if details.iter().any(
            |d| matches!(d, MismatchDetail::CacheMismatch { field, .. } if field.contains(".flxi")),
        ) {
            return Some("semantic interface drift between fresh and cached ways");
        }
        if details.iter().any(
            |d| matches!(d, MismatchDetail::CacheMismatch { field, .. } if field.contains(".fxm")),
        ) {
            return Some(
                "VM artifact hydration or module cache drift between fresh and cached ways",
            );
        }
        if details.iter().any(|d| {
            matches!(d, MismatchDetail::CacheMismatch { field, .. } if field.contains("native"))
        }) {
            return Some("native module artifact or link behavior diverged between fresh and cached ways");
        }
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
    if has_repr && has_backend_surface {
        return Some("likely backend representation mismatch after Core/Aether parity");
    }
    if has_repr {
        return Some("likely backend representation contract drift");
    }
    if has_backend_ir {
        return Some("likely backend-specific IR divergence after shared parity");
    }
    if has_backend_runtime {
        return Some("likely backend/runtime divergence after shared parity");
    }
    if has_backend_surface {
        return Some("likely backend/runtime divergence; capture Core to confirm frontend parity");
    }
    None
}

fn print_cache_summary(result: &ParityResult) {
    for run in &result.results {
        if run.cache_observations.is_empty() {
            continue;
        }
        println!("  {} {}", cyan("cache:"), run.way);
        let created = run
            .cache_observations
            .iter()
            .filter(|obs| obs.state == super::CacheFileState::Created)
            .count();
        let existed = run
            .cache_observations
            .iter()
            .filter(|obs| obs.state == super::CacheFileState::Existed)
            .count();
        println!(
            "    created={created} reused={existed} artifacts={}",
            run.cache_observations.len()
        );
        for obs in &run.cache_observations {
            println!(
                "    - {} [{}] {}",
                obs.kind,
                state_label(obs.state),
                obs.path.display()
            );
        }
        if run.way == Way::LlvmCached && created > 0 && existed > 0 {
            println!(
                "    note: native artifact boundary working; cached output matched with artifact reuse observed"
            );
        } else if run.way == Way::LlvmCached && existed > 0 {
            println!(
                "    note: native cached way matched output; full module skipping is not required in this phase"
            );
        }
    }
}

fn state_label(state: super::CacheFileState) -> &'static str {
    match state {
        super::CacheFileState::Created => "created",
        super::CacheFileState::Existed => "reused",
    }
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
        MismatchDetail::RepresentationMismatch {
            left_way,
            left,
            right_way,
            right,
        } => {
            println!(
                "  {} backend representation contract differs",
                cyan("representation_mismatch:")
            );
            print_inline_diff(left_way.to_string(), left, right_way.to_string(), right);
        }
        MismatchDetail::BackendIrMismatch {
            baseline_way,
            backend,
            surface,
            summary,
        } => {
            println!(
                "  {} baseline {} vs backend {}.ir({})",
                cyan("backend_ir_mismatch:"),
                baseline_way,
                backend,
                surface
            );
            println!("    {summary}");
        }
        MismatchDetail::BackendRuntimeMismatch {
            baseline_way,
            backend,
            summary,
        } => {
            println!(
                "  {} baseline {} vs backend {}",
                cyan("backend_runtime_mismatch:"),
                baseline_way,
                backend
            );
            println!("    {summary}");
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

fn print_explain_block(result: &ParityResult) {
    let Some(Verdict::Mismatch { details }) = Some(&result.verdict) else {
        return;
    };
    let baseline_way = result.results.first().map(|run| run.way).unwrap_or(Way::Vm);
    println!("  {}", cyan("ladder:"));
    println!("    core: {}", layer_status(details, "core"));
    println!("    aether: {}", layer_status(details, "aether"));
    println!("    repr: {}", layer_status(details, "repr"));
    let baseline_spec = backend_spec(baseline_way.backend_id());
    println!(
        "    {}.ir({}): match",
        baseline_way.backend_id(),
        baseline_spec.ir_surface
    );
    for run in result.results.iter().skip(1) {
        let backend = run.way.backend_id();
        let spec = backend_spec(backend);
        let status = details.iter().find_map(|detail| match detail {
            MismatchDetail::BackendIrMismatch {
                backend: detail_backend,
                ..
            } if *detail_backend == backend => Some("suspect divergence"),
            MismatchDetail::BackendRuntimeMismatch {
                backend: detail_backend,
                ..
            } if *detail_backend == backend => Some("runtime diverged"),
            _ => None,
        });
        let fallback = if backend == baseline_way.backend_id() {
            "match"
        } else {
            "captured"
        };
        println!(
            "    {}.ir({}): {}",
            backend,
            spec.ir_surface,
            status.unwrap_or(fallback)
        );
    }

    let likely_source = likely_source_expressions(result);
    if !likely_source.is_empty() {
        println!("  {}", cyan("likely source:"));
        for source in likely_source {
            println!("    {source}");
        }
    }
    let symbols = extract_symbols(result);
    let likely_files = likely_rust_files(details, &symbols);
    if !likely_files.is_empty() {
        println!("  {}", cyan("likely rust code:"));
        for path in likely_files {
            println!("    {path}");
        }
    }
    let next = next_commands(result, &symbols);
    if !next.is_empty() {
        println!("  {}", cyan("next:"));
        for cmd in next {
            println!("    {cmd}");
        }
    }
}

fn layer_status(details: &[MismatchDetail], layer: &str) -> &'static str {
    let mismatch = details.iter().any(|detail| {
        matches!(
            (layer, detail),
            ("core", MismatchDetail::CoreMismatch { .. })
                | ("aether", MismatchDetail::AetherMismatch { .. })
                | ("repr", MismatchDetail::RepresentationMismatch { .. })
        )
    });
    if mismatch { "differs" } else { "match" }
}

fn likely_source_expressions(result: &ParityResult) -> Vec<String> {
    let Ok(source) = fs::read_to_string(&result.file) else {
        return Vec::new();
    };
    let print_lines = source
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            line.contains("print(")
                .then_some((idx + 1, line.trim().to_string()))
        })
        .collect::<Vec<_>>();
    let mut candidates = BTreeSet::new();
    if let Verdict::Mismatch { details } = &result.verdict {
        for detail in details {
            if let MismatchDetail::Stdout { left, right, .. } = detail {
                let left_lines = left.lines().collect::<Vec<_>>();
                let right_lines = right.lines().collect::<Vec<_>>();
                let max = left_lines.len().max(right_lines.len());
                for idx in 0..max {
                    let left_line = left_lines.get(idx).copied().unwrap_or("");
                    let right_line = right_lines.get(idx).copied().unwrap_or("");
                    if left_line != right_line
                        && let Some((lineno, line)) = print_lines.get(idx)
                    {
                        candidates.insert(format!("line {lineno}: {line}"));
                    }
                }
            }
        }
    }
    candidates.into_iter().take(3).collect()
}

fn extract_symbols(result: &ParityResult) -> Vec<String> {
    let mut symbols = BTreeSet::new();
    for source in likely_source_expressions(result) {
        for candidate in ["reverse", "contains", "slice", "Some", "Cons", "ArrayPush"] {
            if source.contains(candidate) {
                symbols.insert(candidate.to_string());
            }
        }
    }
    symbols.into_iter().collect()
}

fn likely_rust_files(details: &[MismatchDetail], symbols: &[String]) -> Vec<String> {
    let mut files = BTreeSet::new();
    for detail in details {
        match detail {
            MismatchDetail::CoreMismatch { .. } => {
                files.insert("src/ast/".to_string());
                files.insert("src/core/".to_string());
                files.insert("src/core/lower_ast/".to_string());
            }
            MismatchDetail::AetherMismatch { .. } => {
                files.insert("src/aether/".to_string());
            }
            MismatchDetail::RepresentationMismatch { .. } => {
                files.insert("src/bytecode/vm/".to_string());
                files.insert("src/lir/emit_llvm.rs".to_string());
                files.insert("runtime/c/flux_rt.c".to_string());
            }
            MismatchDetail::BackendIrMismatch { backend, .. }
            | MismatchDetail::BackendRuntimeMismatch { backend, .. } => {
                let spec = backend_spec(*backend);
                for file in spec.lowering_files {
                    files.insert((*file).to_string());
                }
                for file in spec.runtime_files {
                    files.insert((*file).to_string());
                }
            }
            _ => {}
        }
    }
    if symbols
        .iter()
        .any(|s| matches!(s.as_str(), "reverse" | "contains" | "slice"))
    {
        files.insert("src/bytecode/vm/core_dispatch.rs".to_string());
        files.insert("src/lir/emit_llvm.rs".to_string());
        files.insert("runtime/c/flux_rt.c".to_string());
        files.insert("runtime/c/array.c".to_string());
    }
    files.into_iter().collect()
}

fn next_commands(result: &ParityResult, symbols: &[String]) -> Vec<String> {
    let mut commands = Vec::new();
    if let Verdict::Mismatch { details } = &result.verdict
        && details.iter().any(|d| {
            matches!(
                d,
                MismatchDetail::BackendIrMismatch { .. }
                    | MismatchDetail::BackendRuntimeMismatch { .. }
            )
        })
    {
        commands.push(format!("cargo run -- --dump-cfg {}", result.file.display()));
        commands.push(format!(
            "cargo run --features core_to_llvm -- --dump-lir {} --native",
            result.file.display()
        ));
    }
    if !symbols.is_empty() {
        commands.push(format!(
            "rg -n \"{}\" src/lir src/bytecode/vm runtime/c --glob '!target'",
            symbols.join("|")
        ));
    }
    commands
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

#[cfg(test)]
mod tests {
    use super::diagnose_mismatch;
    use crate::parity::{MismatchDetail, Way};

    #[test]
    fn diagnose_representation_mismatch() {
        let details = vec![MismatchDetail::RepresentationMismatch {
            left_way: Way::Vm,
            left: "rule.match_ctor = ok".to_string(),
            right_way: Way::Llvm,
            right: "rule.match_ctor = bad".to_string(),
        }];

        assert_eq!(
            diagnose_mismatch(&details),
            Some("likely backend representation contract drift")
        );
    }

    #[test]
    fn diagnose_backend_runtime_mismatch() {
        let details = vec![MismatchDetail::BackendRuntimeMismatch {
            baseline_way: Way::Vm,
            backend: crate::parity::BackendId::Llvm,
            summary: "llvm diverged from baseline vm".to_string(),
        }];
        assert_eq!(
            diagnose_mismatch(&details),
            Some("likely backend/runtime divergence after shared parity")
        );
    }
}
