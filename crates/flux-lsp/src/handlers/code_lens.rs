//! `textDocument/codeLens` — runnable "▶ Run" / "▶ Run Test" lenses.
//!
//! A "▶ Run" lens sits above a top-level `fn main` (the program entry point);
//! a "▶ Run Test" lens sits above each top-level `fn test_*`. The lens command
//! is interpreted by the VS Code extension (`flux.run` / `flux.runTest`),
//! which launches the Flux CLI — `cargo run -- <file>` for a run, and
//! `… <file> --test --test-filter <name>` for a single test. The server only
//! locates the runnables and names the command; it never spawns a process.

use flux::syntax::statement::Statement;
use lsp_types::{CodeLens, Command, Range, Uri};
use serde_json::{Value, json};

use crate::snapshot::Snapshot;

/// Build the runnable lenses for `uri`'s buffer. The file URI travels as the
/// first command argument so the extension knows which file to run; a test
/// lens adds the test's function name as a second argument.
pub fn code_lenses(snapshot: &Snapshot, uri: &Uri) -> Vec<CodeLens> {
    let uri_arg = json!(uri);
    let mut lenses = Vec::new();
    for stmt in &snapshot.program.statements {
        let Statement::Function { name, span, .. } = stmt else {
            continue;
        };
        let Some(fn_name) = snapshot.interner.try_resolve(*name) else {
            continue;
        };
        let range = snapshot.position_map.flux_span_to_range(*span);
        if fn_name == "main" {
            lenses.push(runnable(range, "▶ Run", "flux.run", vec![uri_arg.clone()]));
        } else if is_test_name(fn_name) {
            lenses.push(runnable(
                range,
                "▶ Run Test",
                "flux.runTest",
                vec![uri_arg.clone(), json!(fn_name)],
            ));
        }
    }
    lenses
}

/// Flux test functions are the top-level `test_*` functions the CLI's `--test`
/// mode collects. A bare `test_` with no suffix is not a test.
fn is_test_name(name: &str) -> bool {
    name.strip_prefix("test_")
        .is_some_and(|rest| !rest.is_empty())
}

fn runnable(range: Range, title: &str, command: &str, arguments: Vec<Value>) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title: title.to_string(),
            command: command.to_string(),
            arguments: Some(arguments),
        }),
        data: None,
    }
}
