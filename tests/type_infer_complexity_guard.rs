use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_FN_LINES: usize = 60;
const MAX_BRANCH_POINTS: usize = 15;

#[derive(Debug)]
struct FnSpan {
    file: PathBuf,
    name: String,
    start_line: usize,
    lines: Vec<String>,
}

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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ast/type_infer");
    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);
    files.sort();

    let mut violations = Vec::new();
    for file in files {
        for f in collect_functions(&file) {
            let line_count = f.lines.len();
            if line_count > MAX_FN_LINES {
                violations.push(format!(
                    "{}:{} `{}` has {} lines (max {})",
                    f.file.display(),
                    f.start_line,
                    f.name,
                    line_count,
                    MAX_FN_LINES
                ));
            }
            let branches = count_branch_points(&f.lines);
            if branches > MAX_BRANCH_POINTS {
                violations.push(format!(
                    "{}:{} `{}` has {} branch points (max {})",
                    f.file.display(),
                    f.start_line,
                    f.name,
                    branches,
                    MAX_BRANCH_POINTS
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
