//! `cargo xtask bench stats-diff <base.json> <head.json>`: compares two
//! `flux --stats-json` files phase by phase, as Markdown. Phases match by
//! name, so a VM file and a native file share only the phases both have.
//! The times are single runs, so a small difference is noise.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

const COUNTS: [&str; 4] = ["source_lines", "globals", "functions", "instruction_bytes"];

pub fn run(root: &Path, base_path: &Path, head_path: &Path) -> Result<bool, String> {
    let load = |path: &Path| -> Result<Value, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    };
    let (base, head) = (load(base_path)?, load(head_path)?);
    // Shown from the repository root, so a pasted table carries no home
    // directory.
    let shown =
        |path: &Path| path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    print!("{}", render(&shown(base_path), &base, &shown(head_path), &head));
    Ok(true)
}

fn render(base_name: &str, base: &Value, head_name: &str, head: &Value) -> String {
    let mut out = String::new();
    let backend = |v: &Value| v["backend"].as_str().unwrap_or("?").to_string();
    let _ = writeln!(out, "Base: `{base_name}` ({})", backend(base));
    let _ = writeln!(out, "Head: `{head_name}` ({})\n", backend(head));
    let _ = writeln!(out, "| Phase | Base | Head | Head / base |\n|:---|---:|---:|---:|");

    let phases = |v: &Value| -> Vec<(String, f64)> {
        v["phases"].as_array().map_or(Vec::new(), |ps| {
            ps.iter()
                .filter_map(|p| Some((p["phase"].as_str()?.to_string(), p["ms"].as_f64()?)))
                .collect()
        })
    };
    let (base_ms, head_ms) = (phases(base), phases(head));
    let lookup = |list: &[(String, f64)], name: &str| {
        list.iter().find(|(n, _)| n == name).map(|(_, ms)| *ms)
    };
    // Head's order first, then any phase only base has, so a renamed or
    // dropped phase still shows up.
    let mut names: Vec<&str> = head_ms.iter().map(|(n, _)| n.as_str()).collect();
    names.extend(base_ms.iter().map(|(n, _)| n.as_str()).filter(|n| lookup(&head_ms, n).is_none()));
    for name in names {
        let (b, h) = (lookup(&base_ms, name), lookup(&head_ms, name));
        let _ = writeln!(out, "| `{name}` | {} | {} | {} |", ms(b), ms(h), ratio(b, h));
    }
    let total = |list: &[(String, f64)]| list.iter().map(|(_, ms)| ms).sum::<f64>();
    let (tb, th) = (total(&base_ms), total(&head_ms));
    let _ = writeln!(
        out,
        "| total | {} | {} | {} |",
        ms(Some(tb)),
        ms(Some(th)),
        ratio(Some(tb), Some(th))
    );

    let (base_counts, head_counts) = (counts(base), counts(head));
    let changed: Vec<&(String, Value)> = head_counts
        .iter()
        .filter(|(key, value)| {
            base_counts.iter().find(|(k, _)| k == key).map(|(_, v)| v) != Some(value)
        })
        .collect();
    if !changed.is_empty() {
        let _ = writeln!(out, "\nCounts that changed:\n");
        for (key, after) in changed {
            let before =
                base_counts.iter().find(|(k, _)| k == key).map_or(Value::Null, |(_, v)| v.clone());
            let _ = writeln!(out, "- `{key}`: {before} -> {after}");
        }
    }
    out
}

/// The top-level counts, then each `modules` count as `modules.<key>`.
fn counts(stats: &Value) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> =
        COUNTS.iter().map(|key| (key.to_string(), stats[key].clone())).collect();
    if let Some(modules) = stats["modules"].as_object() {
        found.extend(modules.iter().map(|(k, v)| (format!("modules.{k}"), v.clone())));
    }
    found
}

fn ms(value: Option<f64>) -> String {
    match value {
        None => "-".to_string(),
        Some(ms) if ms < 1000.0 => format!("{ms:.1} ms"),
        Some(ms) => format!("{:.3} s", ms / 1000.0),
    }
}

fn ratio(base: Option<f64>, head: Option<f64>) -> String {
    match (base, head) {
        (Some(b), Some(h)) if b != 0.0 => format!("{:.2}x", h / b),
        _ => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_and_changed_counts_are_compared() {
        let base: Value = serde_json::from_str(
            r#"{"backend": "native", "functions": 10, "modules": {"cached": 0},
                "phases": [{"phase": "parse", "ms": 2.0}, {"phase": "link", "ms": 1500.0}]}"#,
        )
        .unwrap();
        let head: Value = serde_json::from_str(
            r#"{"backend": "native", "functions": 10, "modules": {"cached": 18},
                "phases": [{"phase": "parse", "ms": 1.0}]}"#,
        )
        .unwrap();
        let text = render("cold.json", &base, "warm.json", &head);
        assert!(text.contains("| `parse` | 2.0 ms | 1.0 ms | 0.50x |"), "{text}");
        assert!(text.contains("| `link` | 1.500 s | - | - |"), "{text}");
        assert!(text.contains("| total | 1.502 s | 1.0 ms | 0.00x |"), "{text}");
        assert!(text.contains("- `modules.cached`: 0 -> 18"), "{text}");
        assert!(!text.contains("`functions`"), "{text}");
    }
}
