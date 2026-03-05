use std::{
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

fn is_function_signature_line(trimmed: &str) -> bool {
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ast/type_infer");
    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);
    files.sort();

    let mut violations = Vec::new();

    for file in files {
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
            if !is_function_signature_line(trimmed) {
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
