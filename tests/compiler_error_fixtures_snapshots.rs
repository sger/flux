mod diagnostics_env;
#[allow(dead_code)]
#[path = "support/examples_snapshot.rs"]
mod examples_snapshot;

use std::{collections::HashSet, fs, path::Path};

use flux::{
    bytecode::compiler::Compiler,
    diagnostics::{Diagnostic, DiagnosticsAggregator, quality::module_skipped_note},
    syntax::{lexer::Lexer, module_graph::ModuleGraph, parser::Parser},
};

fn build_compiler_transcript(
    fixture: &Path,
    fixture_rel: &str,
    workspace_root: &Path,
) -> Result<String, String> {
    let source = fs::read_to_string(fixture)
        .map_err(|e| format!("failed to read `{}`: {e}", fixture.display()))?;

    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    let mut diagnostics: Vec<Diagnostic> = parser.take_warnings();
    let entry_has_errors = !parser.errors.is_empty();
    diagnostics.append(&mut parser.errors);
    let mut compile_status = String::from("ok");

    if entry_has_errors {
        compile_status = String::from("failed (parse)");
    } else {
        let mut roots = Vec::new();
        if let Some(parent) = fixture.parent() {
            roots.push(parent.to_path_buf());
        }
        let src_root = workspace_root.join("src");
        if src_root.exists() {
            roots.push(src_root);
        }

        let interner = parser.take_interner();
        let graph_result =
            ModuleGraph::build_with_entry_and_roots(fixture, &program, interner, &roots);
        diagnostics.extend(graph_result.diagnostics);

        let mut failed: HashSet<_> = graph_result.failed_modules;
        let graph = graph_result.graph;
        let mut compiler = Compiler::new_with_interner(fixture_rel, graph_result.interner);
        let entry_canonical = std::fs::canonicalize(fixture).ok();

        for node in graph.topo_order() {
            if entry_has_errors
                && let Some(ref canon) = entry_canonical
                && &node.path == canon
            {
                continue;
            }

            let failed_dep = node
                .imports
                .iter()
                .find(|edge| failed.contains(&edge.target_path));
            if let Some(dep) = failed_dep {
                failed.insert(node.path.clone());
                diagnostics.push(module_skipped_note(
                    node.path.to_string_lossy().to_string(),
                    node.path.to_string_lossy().to_string(),
                    dep.name.clone(),
                ));
                continue;
            }

            compiler.set_file_path(node.path.to_string_lossy().to_string());
            let compile_result = compiler.compile(&node.program);
            let mut warnings = compiler.take_warnings();
            for diag in &mut warnings {
                if diag.file().is_none() {
                    diag.set_file(node.path.to_string_lossy().to_string());
                }
            }
            diagnostics.append(&mut warnings);

            if let Err(mut diags) = compile_result {
                for diag in &mut diags {
                    if diag.file().is_none() {
                        diag.set_file(node.path.to_string_lossy().to_string());
                    }
                }
                diagnostics.append(&mut diags);
                failed.insert(node.path.clone());
            }
        }

        if !diagnostics.is_empty() {
            compile_status = String::from("failed (compile)");
        }
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

    let normalized = examples_snapshot::normalize_transcript(&diagnostics_text, workspace_root);
    Ok(format!(
        "Fixture: {fixture_rel}\n== compile ==\n{compile_status}\n== diagnostics ==\n{}\n== stdout ==\n<not executed>\n",
        normalized.trim_end()
    ))
}

#[test]
fn compiler_error_fixtures_snapshot() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_root = workspace_root.join("examples/compiler_errors");
    let fixtures = examples_snapshot::discover_fixtures(&fixtures_root);

    for fixture in fixtures {
        let rel = fixture
            .strip_prefix(workspace_root)
            .unwrap_or(&fixture)
            .to_string_lossy()
            .replace('\\', "/");
        let snapshot_name = examples_snapshot::snapshot_name(&fixtures_root, &fixture);
        let transcript = build_compiler_transcript(&fixture, &rel, workspace_root)
            .unwrap_or_else(|e| format!("Fixture: {rel}\n== error ==\n{e}\n"));

        insta::with_settings!({
            snapshot_path => "snapshots/compiler_error_fixtures",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!(snapshot_name, transcript);
        });
    }
}
