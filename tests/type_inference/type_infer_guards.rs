//! Source-level guards over `src/ast/type_infer/`.
//!
//! Three code-quality checks share a common crawler over the module tree:
//! - rustdoc coverage for every function signature
//! - per-function line/branch budgets (proposal 0079 R2)
//! - naming conventions (forbidden legacy names, required entrypoints)

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("read_dir should succeed");
    for entry in entries {
        let entry = entry.expect("dir entry should be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_fn_start_line(trimmed: &str) -> bool {
    if trimmed.starts_with("//") {
        return false;
    }
    if trimmed.starts_with("fn ") {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("pub") {
        return rest.contains(" fn ");
    }
    false
}

fn function_name_from_line(line: &str) -> String {
    let after_fn = line
        .split_once("fn ")
        .map(|(_, right)| right)
        .unwrap_or(line)
        .trim_start();
    let end = after_fn
        .find(|c: char| c == '<' || c == '(' || c.is_whitespace())
        .unwrap_or(after_fn.len());
    after_fn[..end].to_string()
}

fn type_infer_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ast/type_infer")
}

fn type_infer_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rs_files(&type_infer_root(), &mut files);
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Guard 1: rustdoc coverage
// ---------------------------------------------------------------------------

fn has_preceding_doc_block(lines: &[&str], fn_line_idx: usize) -> bool {
    if fn_line_idx == 0 {
        return false;
    }

    let mut idx = fn_line_idx;
    while idx > 0 {
        idx -= 1;
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("#[") {
            continue;
        }
        if trimmed.starts_with("///") {
            return true;
        }
        return false;
    }
    false
}

#[test]
fn type_infer_functions_require_rustdoc() {
    let mut violations = Vec::new();

    for file in type_infer_files() {
        let content = fs::read_to_string(&file).expect("source file should be readable");
        let lines: Vec<&str> = content.lines().collect();
        let mut in_cfg_test = false;

        for (line_idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("#[cfg(test)]") {
                in_cfg_test = true;
                continue;
            }
            if in_cfg_test && trimmed.starts_with("mod tests") {
                break;
            }
            if !is_fn_start_line(trimmed) {
                continue;
            }
            if !has_preceding_doc_block(&lines, line_idx) {
                violations.push(format!(
                    "{}:{} missing rustdoc",
                    file.display(),
                    line_idx + 1
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "undocumented functions in src/ast/type_infer:\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Guard 2: per-function complexity budget
// ---------------------------------------------------------------------------

const MAX_FN_LINES: usize = 60;
const MAX_BRANCH_POINTS: usize = 15;

/// Functions exempt from the default limits because they are intentionally
/// a single flat dispatch (proposal 0079 R2).
const EXEMPT_FUNCTIONS: &[(&str, usize, usize)] = &[
    // infer_expression is a single exhaustive match over all Expression variants.
    ("infer_expression", 160, 35),
];

#[derive(Debug)]
struct FnSpan {
    file: PathBuf,
    name: String,
    start_line: usize,
    lines: Vec<String>,
}

fn strip_line_comment(s: &str) -> &str {
    if let Some((before, _)) = s.split_once("//") {
        before
    } else {
        s
    }
}

fn find_fn_end_idx(lines: &[String], start_idx: usize) -> usize {
    let mut depth = 0usize;
    let mut seen_open = false;

    for (idx, line) in lines.iter().enumerate().skip(start_idx) {
        let code = strip_line_comment(line);
        for ch in code.chars() {
            match ch {
                '{' => {
                    seen_open = true;
                    depth += 1;
                }
                '}' => {
                    if seen_open {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return idx + 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    lines.len()
}

fn collect_functions(file: &Path) -> Vec<FnSpan> {
    let content = fs::read_to_string(file).expect("source should be readable");
    let lines: Vec<String> = content.lines().map(str::to_owned).collect();
    let mut starts: Vec<(usize, String)> = Vec::new();

    let mut skip_rest_for_tests = false;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(test)]") {
            skip_rest_for_tests = true;
            continue;
        }
        if skip_rest_for_tests && trimmed.starts_with("mod tests") {
            break;
        }
        if is_fn_start_line(trimmed) {
            starts.push((idx, function_name_from_line(trimmed)));
        }
    }

    let mut result = Vec::new();
    for (start_idx, name) in starts {
        let end_idx = find_fn_end_idx(&lines, start_idx);
        result.push(FnSpan {
            file: file.to_path_buf(),
            name,
            start_line: start_idx + 1,
            lines: lines[start_idx..end_idx].to_vec(),
        });
    }
    result
}

fn count_branch_points(lines: &[String]) -> usize {
    let mut count = 0usize;
    for line in lines {
        let s = line.trim();
        if s.starts_with("//") {
            continue;
        }
        count += s.match_indices(" if ").count();
        if s.starts_with("if ") {
            count += 1;
        }
        count += s.match_indices("match ").count();
        count += s.match_indices("&&").count();
        count += s.match_indices("||").count();
        // Approximate match-arm complexity.
        count += s.match_indices("=>").count();
    }
    count
}

#[test]
fn type_infer_function_complexity_budget() {
    let mut violations = Vec::new();
    for file in type_infer_files() {
        for f in collect_functions(&file) {
            let (max_lines, max_branches) = EXEMPT_FUNCTIONS
                .iter()
                .find(|(name, _, _)| *name == f.name)
                .map(|(_, ml, mb)| (*ml, *mb))
                .unwrap_or((MAX_FN_LINES, MAX_BRANCH_POINTS));

            let line_count = f.lines.len();
            if line_count > max_lines {
                violations.push(format!(
                    "{}:{} `{}` has {} lines (max {})",
                    f.file.display(),
                    f.start_line,
                    f.name,
                    line_count,
                    max_lines
                ));
            }
            let branches = count_branch_points(&f.lines);
            if branches > max_branches {
                violations.push(format!(
                    "{}:{} `{}` has {} branch points (max {})",
                    f.file.display(),
                    f.start_line,
                    f.name,
                    branches,
                    max_branches
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "type_infer complexity violations:\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Guard 3: naming conventions
// ---------------------------------------------------------------------------

#[test]
fn type_infer_naming_conventions() {
    let mut names = BTreeSet::new();
    for file in &type_infer_files() {
        let content = fs::read_to_string(file).expect("source should be readable");
        for line in content.lines() {
            let trimmed = line.trim();
            if is_fn_start_line(trimmed) {
                names.insert(function_name_from_line(trimmed));
            }
        }
    }

    let forbidden = [
        "infer_fn",
        "infer_stmt",
        "infer_let",
        "infer_expr",
        "infer_infix",
        "infer_call",
        "infer_call_expr",
        "infer_if_expr",
        "infer_match_expr",
        "infer_lambda_expr",
        "infer_collection_expr",
        "infer_access_expr",
        "infer_perform_expr",
        "infer_handle_expr",
        "bind_pattern",
        "match_constraint_family",
        "family_expected_type",
        "build_type_param_map",
        "finalize_function_scheme",
    ];

    let mut violations = Vec::new();
    for old in forbidden {
        if names.contains(old) {
            violations.push(format!("deprecated function name still present: {old}"));
        }
    }

    // After proposal 0079 R2, the old family dispatchers (infer_literal_expression,
    // infer_collection_expression, infer_access_expression) are gone — all routing
    // happens in the flat `infer_expression` match. Per-variant helpers remain.
    let required_expression_entrypoints = [
        "infer_expression",
        "infer_infix_expression",
        "infer_function_call",
        "infer_call_expression",
        "infer_if_expression",
        "infer_match_expression",
        "infer_lambda_expression",
        "infer_perform_expression",
        "infer_handle_expression",
    ];
    for required in required_expression_entrypoints {
        if !names.contains(required) {
            violations.push(format!("required expression naming missing: {required}"));
        }
    }

    assert!(
        violations.is_empty(),
        "type_infer naming violations:\n{}",
        violations.join("\n")
    );
}
