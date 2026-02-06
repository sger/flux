use std::fs;
use std::path::{Path, PathBuf};

use flux::{
    bytecode::compiler::Compiler,
    frontend::{
        diagnostics::{Diagnostic, DiagnosticsAggregator},
        lexer::Lexer,
        module_graph::ModuleGraph,
        parser::Parser,
    },
};

pub struct FixtureSnapshotCase {
    pub snapshot_name: String,
    pub transcript: String,
}

pub fn discover_fixtures(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir).unwrap_or_else(|e| {
            panic!("failed to read fixture directory `{}`: {e}", dir.display())
        });

        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("failed to read fixture entry: {e}"));
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("flx") {
                out.push(path);
            }
        }
    }

    let mut fixtures = Vec::new();
    walk(root, &mut fixtures);
    fixtures.sort();
    fixtures
}

pub fn snapshot_name(fixtures_root: &Path, fixture: &Path) -> String {
    let rel = fixture
        .strip_prefix(fixtures_root)
        .unwrap_or_else(|_| panic!("fixture `{}` is not under root", fixture.display()));
    let mut name = rel.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = name.strip_suffix(".flx") {
        name = stripped.to_string();
    }
    name.replace('/', "__")
}

pub fn run_fixture_dir_snapshots(
    workspace_root: &Path,
    fixtures_dir_rel: &str,
) -> Result<Vec<FixtureSnapshotCase>, String> {
    let fixtures_root = workspace_root.join(fixtures_dir_rel);
    let fixtures = discover_fixtures(&fixtures_root);
    if fixtures.is_empty() {
        return Err(format!(
            "no .flx fixtures found under `{}`",
            fixtures_root.display()
        ));
    }

    let mut cases = Vec::with_capacity(fixtures.len());
    for fixture in fixtures {
        let rel = fixture
            .strip_prefix(workspace_root)
            .unwrap_or(&fixture)
            .to_string_lossy()
            .replace('\\', "/");
        let snapshot = snapshot_name(&fixtures_root, &fixture);
        let transcript = build_transcript(&fixture, &rel, workspace_root)
            .unwrap_or_else(|e| format!("Fixture: {rel}\n== error ==\n{e}\n"));
        cases.push(FixtureSnapshotCase {
            snapshot_name: snapshot,
            transcript,
        });
    }

    Ok(cases)
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }

    out
}

pub fn normalize_transcript(text: &str, workspace_root: &Path) -> String {
    let mut normalized = strip_ansi(&text.replace("\r\n", "\n").replace('\\', "/"));

    let mut prefixes = vec![workspace_root.to_string_lossy().replace('\\', "/")];
    if let Ok(canonical) = workspace_root.canonicalize() {
        prefixes.push(canonical.to_string_lossy().replace('\\', "/"));
    }

    for prefix in prefixes {
        if prefix.is_empty() {
            continue;
        }
        let with_slash = format!("{prefix}/");
        normalized = normalized.replace(&with_slash, "");
        normalized = normalized.replace(&prefix, "");
    }

    normalized
}

pub fn build_transcript(
    fixture: &Path,
    fixture_rel: &str,
    workspace_root: &Path,
) -> Result<String, String> {
    let source = fs::read_to_string(fixture)
        .map_err(|e| format!("failed to read `{}`: {e}", fixture.display()))?;

    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    let mut diagnostics: Vec<Diagnostic> = std::mem::take(&mut parser.errors);
    let mut compile_status = String::from("ok");

    if diagnostics.is_empty() {
        let mut roots = Vec::new();
        if let Some(parent) = fixture.parent() {
            roots.push(parent.to_path_buf());
        }
        let src_root = workspace_root.join("src");
        if src_root.exists() {
            roots.push(src_root);
        }

        match ModuleGraph::build_with_entry_and_roots(fixture, &program, &roots) {
            Ok(graph) => {
                let mut compiler = Compiler::new_with_file_path(fixture_rel);
                for node in graph.topo_order() {
                    compiler.set_file_path(node.path.to_string_lossy().to_string());
                    if let Err(mut diags) = compiler.compile(&node.program) {
                        for diag in &mut diags {
                            if diag.file().is_none() {
                                diag.set_file(node.path.to_string_lossy().to_string());
                            }
                        }
                        diagnostics.append(&mut diags);
                        break;
                    }
                }
                if !diagnostics.is_empty() {
                    compile_status = String::from("failed (compile)");
                }
            }
            Err(mut diags) => {
                diagnostics.append(&mut diags);
                compile_status = String::from("failed (module)");
            }
        }
    } else {
        compile_status = String::from("failed (parse)");
    }

    let diagnostics_text = if diagnostics.is_empty() {
        String::from("<none>")
    } else {
        DiagnosticsAggregator::new(&diagnostics)
            .with_default_source(fixture_rel, source)
            .with_file_headers(false)
            .report()
            .rendered
    };

    let normalized_diagnostics = normalize_transcript(&diagnostics_text, workspace_root);

    Ok(format!(
        "Fixture: {fixture_rel}\n== compile ==\n{compile_status}\n== diagnostics ==\n{}\n== stdout ==\n<not executed>\n",
        normalized_diagnostics.trim_end()
    ))
}
