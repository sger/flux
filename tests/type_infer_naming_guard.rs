use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

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

#[test]
fn type_infer_naming_conventions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ast/type_infer");
    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);
    files.sort();

    let mut names = BTreeSet::new();
    for file in &files {
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
