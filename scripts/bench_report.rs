#!/usr/bin/env rust-script

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SourceKind {
    Base,
    New,
}

#[derive(Debug, Clone)]
struct BenchStats {
    mean_ns: f64,
    bytes_per_second: Option<f64>,
    source: SourceKind,
}

#[derive(Debug, Clone)]
struct ReportRow {
    name: String,
    baseline_mean_ms: f64,
    current_mean_ms: f64,
    change_percent: f64,
    baseline_bytes_per_sec: Option<f64>,
    current_bytes_per_sec: Option<f64>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let baseline_root =
        PathBuf::from(env::var("PERF_BASELINE_DIR").unwrap_or_else(|_| "baseline_criterion".to_string()));
    let current_root =
        PathBuf::from(env::var("PERF_CURRENT_DIR").unwrap_or_else(|_| "target/criterion".to_string()));

    let baseline = collect_baseline_stats(&baseline_root)?;
    let current = collect_current_stats(&current_root)?;

    let missing: Vec<&str> = current
        .keys()
        .filter(|name| !baseline.contains_key(*name))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "baseline is missing benchmark(s): {}. Recreate baseline from the same bench set.",
            missing.join(", ")
        ));
    }

    println!(
        "benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec"
    );

    let mut rows = Vec::new();
    for (name, current_stats) in &current {
        let baseline_stats = baseline
            .get(name)
            .ok_or_else(|| format!("missing baseline stats for benchmark: {name}"))?;
        let baseline_ms = baseline_stats.mean_ns / 1_000_000.0;
        let current_ms = current_stats.mean_ns / 1_000_000.0;
        let change_percent = ((current_stats.mean_ns - baseline_stats.mean_ns) / baseline_stats.mean_ns) * 100.0;
        rows.push(ReportRow {
            name: name.clone(),
            baseline_mean_ms: baseline_ms,
            current_mean_ms: current_ms,
            change_percent,
            baseline_bytes_per_sec: baseline_stats.bytes_per_second,
            current_bytes_per_sec: current_stats.bytes_per_second,
        });
        println!(
            "{name}|{baseline_ms:.4}|{current_ms:.4}|{change_percent:.2}|{}|{}",
            format_or_dash(baseline_stats.bytes_per_second),
            format_or_dash(current_stats.bytes_per_second),
        );
    }

    let report_path = PathBuf::from(
        env::var("PERF_REPORT_PATH").unwrap_or_else(|_| "PERF_REPORT.md".to_string()),
    );
    let report = build_perf_report(&rows, &baseline_root, &current_root);
    fs::write(&report_path, report)
        .map_err(|e| format!("failed to write {}: {e}", report_path.display()))?;

    println!("wrote {}", report_path.display());

    Ok(())
}

fn collect_baseline_stats(root: &Path) -> Result<BTreeMap<String, BenchStats>, String> {
    if !root.exists() || !root.is_dir() {
        return Err(format!(
            "baseline directory not found: {}. Create it with `cp -r target/criterion baseline_criterion`.",
            root.display()
        ));
    }

    let mut stats: BTreeMap<String, BenchStats> = BTreeMap::new();
    let mut files = Vec::new();
    gather_estimates_files(root, &mut files)?;

    for path in files {
        if path.components().any(|c| c.as_os_str() == "report") {
            continue;
        }

        let source_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|x| x.to_str())
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        let source = match source_name {
            "base" => SourceKind::Base,
            "new" => SourceKind::New,
            _ => continue,
        };

        let benchmark_path = path
            .parent()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        let benchmark_dir = benchmark_path
            .parent()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        let fallback_name = relative_as_posix(root, benchmark_dir)?;

        let estimates_raw = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mean_ns = parse_mean_point_estimate_ns(&estimates_raw)
            .ok_or_else(|| format!("missing mean.point_estimate in {}", path.display()))?;
        if mean_ns <= 0.0 {
            return Err(format!(
                "non-positive mean.point_estimate in {}: {}",
                path.display(),
                mean_ns
            ));
        }

        let (name, bytes_count) = parse_benchmark_meta(&benchmark_path.join("benchmark.json"), &fallback_name)?;
        let candidate = BenchStats {
            mean_ns,
            bytes_per_second: bytes_per_second(bytes_count, mean_ns),
            source,
        };

        match stats.get(&name) {
            Some(existing) if existing.source >= candidate.source => {}
            _ => {
                stats.insert(name, candidate);
            }
        }
    }

    if stats.is_empty() {
        return Err(format!(
            "no baseline benchmark estimates found under: {}",
            root.display()
        ));
    }
    Ok(stats)
}

fn collect_current_stats(root: &Path) -> Result<BTreeMap<String, BenchStats>, String> {
    if !root.exists() || !root.is_dir() {
        return Err(format!("current criterion directory not found: {}", root.display()));
    }

    let mut stats: BTreeMap<String, BenchStats> = BTreeMap::new();
    let mut files = Vec::new();
    gather_estimates_files(root, &mut files)?;

    for path in files {
        let source_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|x| x.to_str())
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        if source_name != "new" {
            continue;
        }

        let benchmark_path = path
            .parent()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        let benchmark_dir = benchmark_path
            .parent()
            .ok_or_else(|| format!("invalid path: {}", path.display()))?;
        let fallback_name = relative_as_posix(root, benchmark_dir)?;

        let estimates_raw = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mean_ns = parse_mean_point_estimate_ns(&estimates_raw)
            .ok_or_else(|| format!("missing mean.point_estimate in {}", path.display()))?;
        if mean_ns <= 0.0 {
            return Err(format!(
                "non-positive mean.point_estimate in {}: {}",
                path.display(),
                mean_ns
            ));
        }

        let (name, bytes_count) = parse_benchmark_meta(&benchmark_path.join("benchmark.json"), &fallback_name)?;
        stats.insert(
            name,
            BenchStats {
                mean_ns,
                bytes_per_second: bytes_per_second(bytes_count, mean_ns),
                source: SourceKind::New,
            },
        );
    }

    if stats.is_empty() {
        return Err(format!(
            "no current benchmark estimates found under: {}/**/new/estimates.json",
            root.display()
        ));
    }

    Ok(stats)
}

fn gather_estimates_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(root).map_err(|e| format!("failed to read dir {}: {e}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read directory entry: {e}"))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|e| format!("failed to read metadata {}: {e}", path.display()))?;
        if metadata.is_dir() {
            gather_estimates_files(&path, out)?;
        } else if metadata.is_file() && path.file_name().and_then(|x| x.to_str()) == Some("estimates.json") {
            out.push(path);
        }
    }
    Ok(())
}

fn parse_benchmark_meta(path: &Path, fallback_name: &str) -> Result<(String, Option<u64>), String> {
    if !path.exists() {
        return Ok((fallback_name.to_string(), None));
    }

    let raw = fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let name = extract_json_string(&raw, "full_id")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_name.to_string());
    let bytes = extract_json_u64(&raw, "Bytes").filter(|v| *v > 0);
    Ok((name, bytes))
}

fn parse_mean_point_estimate_ns(raw: &str) -> Option<f64> {
    let mean_idx = raw.find("\"mean\"")?;
    let after_mean = &raw[mean_idx..];
    let point_idx = after_mean.find("\"point_estimate\"")?;
    let after_point = &after_mean[point_idx + "\"point_estimate\"".len()..];
    let colon_idx = after_point.find(':')?;
    let after_colon = &after_point[colon_idx + 1..];
    parse_json_number_prefix(after_colon)
}

fn extract_json_u64(raw: &str, key: &str) -> Option<u64> {
    let key_pattern = format!("\"{key}\"");
    let idx = raw.find(&key_pattern)?;
    let after_key = &raw[idx + key_pattern.len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let number_str = take_number_prefix(after_colon)?;
    number_str.parse::<u64>().ok()
}

fn extract_json_string(raw: &str, key: &str) -> Option<String> {
    let key_pattern = format!("\"{key}\"");
    let idx = raw.find(&key_pattern)?;
    let after_key = &raw[idx + key_pattern.len()..];
    let colon = after_key.find(':')?;
    let rest = after_key[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }

    let mut escaped = false;
    let mut out = String::new();
    for ch in rest[1..].chars() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

fn parse_json_number_prefix(s: &str) -> Option<f64> {
    let token = take_number_prefix(s)?;
    token.parse::<f64>().ok()
}

fn take_number_prefix(s: &str) -> Option<&str> {
    let trimmed = s.trim_start();
    let mut end = 0usize;
    for (i, ch) in trimmed.char_indices() {
        if ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | 'e' | 'E') {
            end = i + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        None
    } else {
        Some(&trimmed[..end])
    }
}

fn relative_as_posix(root: &Path, path: &Path) -> Result<String, String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| format!("failed to strip prefix {} from {}", root.display(), path.display()))?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<String>>()
        .join("/"))
}

fn bytes_per_second(bytes_count: Option<u64>, mean_ns: f64) -> Option<f64> {
    bytes_count.map(|b| b as f64 / (mean_ns / 1_000_000_000.0))
}

fn format_or_dash(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.2}"),
        None => "-".to_string(),
    }
}

fn find_row<'a>(rows: &'a [ReportRow], name: &str) -> Option<&'a ReportRow> {
    rows.iter().find(|r| r.name == name)
}

fn format_row_markdown(row: Option<&ReportRow>, name: &str) -> String {
    match row {
        Some(r) => format!(
            "| {name} | {:.4} | {:.4} | {:.2} | {} | {} |",
            r.baseline_mean_ms,
            r.current_mean_ms,
            r.change_percent,
            format_or_dash(r.baseline_bytes_per_sec),
            format_or_dash(r.current_bytes_per_sec),
        ),
        None => format!("| {name} | N/A | N/A | N/A | N/A | N/A |"),
    }
}

fn build_perf_report(rows: &[ReportRow], baseline_root: &Path, current_root: &Path) -> String {
    let mut sorted_rows = rows.to_vec();
    sorted_rows.sort_by(|a, b| a.name.cmp(&b.name));

    let mut raw_lines = String::from(
        "benchmark|baseline_mean_ms|current_mean_ms|change_percent|baseline_bytes_per_sec|current_bytes_per_sec\n",
    );
    for r in &sorted_rows {
        raw_lines.push_str(&format!(
            "{}|{:.4}|{:.4}|{:.2}|{}|{}\n",
            r.name,
            r.baseline_mean_ms,
            r.current_mean_ms,
            r.change_percent,
            format_or_dash(r.baseline_bytes_per_sec),
            format_or_dash(r.current_bytes_per_sec),
        ));
    }

    let mixed_tok = find_row(&sorted_rows, "lexer/tokenize/mixed_syntax");
    let mixed_loop = find_row(&sorted_rows, "lexer/next_token_loop/mixed_syntax");
    let comment_tok = find_row(&sorted_rows, "lexer/tokenize/comment_heavy");
    let comment_loop = find_row(&sorted_rows, "lexer/next_token_loop/comment_heavy");
    let ident_tok = find_row(&sorted_rows, "lexer/tokenize/identifier_heavy");
    let ident_loop = find_row(&sorted_rows, "lexer/next_token_loop/identifier_heavy");
    let string_tok = find_row(&sorted_rows, "lexer/tokenize/string_escape_interp_heavy");
    let string_loop = find_row(&sorted_rows, "lexer/next_token_loop/string_escape_interp_heavy");

    format!(
        "# PERF Report\n\n\
Baseline directory: `{}`\n\
Current directory: `{}`\n\n\
## Raw Comparison Output\n\
```text\n\
{}\
```\n\n\
## Corpus: mixed\n\
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |\n\
|---|---:|---:|---:|---:|---:|\n\
{}\n\
{}\n\n\
## Corpus: comment_heavy\n\
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |\n\
|---|---:|---:|---:|---:|---:|\n\
{}\n\
{}\n\n\
## Corpus: ident_heavy\n\
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |\n\
|---|---:|---:|---:|---:|---:|\n\
{}\n\
{}\n\n\
## Corpus: string_heavy\n\
| Benchmark | Baseline Mean (ms) | Current Mean (ms) | Change (%) | Baseline B/s | Current B/s |\n\
|---|---:|---:|---:|---:|---:|\n\
{}\n\
{}\n",
        baseline_root.display(),
        current_root.display(),
        raw_lines,
        format_row_markdown(mixed_tok, "lexer/tokenize/mixed_syntax"),
        format_row_markdown(mixed_loop, "lexer/next_token_loop/mixed_syntax"),
        format_row_markdown(comment_tok, "lexer/tokenize/comment_heavy"),
        format_row_markdown(comment_loop, "lexer/next_token_loop/comment_heavy"),
        format_row_markdown(ident_tok, "lexer/tokenize/identifier_heavy"),
        format_row_markdown(ident_loop, "lexer/next_token_loop/identifier_heavy"),
        format_row_markdown(string_tok, "lexer/tokenize/string_escape_interp_heavy"),
        format_row_markdown(string_loop, "lexer/next_token_loop/string_escape_interp_heavy"),
    )
}
