//! `textDocument/documentLink` — clickable `import` module paths.
//!
//! Every `import A.B.C` whose module is loaded (the Flow stdlib, always
//! indexed, plus sibling user modules in the cursor file's component) becomes a
//! ctrl/cmd-clickable link that opens the module's `.flx` file. The target is
//! resolved eagerly from the same `module_programs` path cache goto-definition
//! uses, so no `documentLink/resolve` round-trip is needed.

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::statement::Statement;
use lsp_types::{DocumentLink, Uri};

use crate::snapshot::Snapshot;

pub fn document_links(snapshot: &Snapshot) -> Vec<DocumentLink> {
    let mut links = Vec::new();
    for stmt in &snapshot.program.statements {
        let Statement::Import { name, span, .. } = stmt else {
            continue;
        };
        let Some(name_text) = snapshot.interner.try_resolve(*name) else {
            continue;
        };
        let Some(target) = module_file_uri(snapshot, name_text) else {
            continue;
        };
        links.push(DocumentLink {
            range: import_name_range(snapshot, *span, name_text),
            target: Some(target),
            tooltip: Some(format!("Open module `{name_text}`")),
            data: None,
        });
    }
    links
}

/// The file URI backing the module `name_text`, if it's loaded. Mirrors
/// goto-definition's resolution: `module_programs` keys user modules by their
/// full declared name and Flow stdlib modules by their short final segment.
fn module_file_uri(snapshot: &Snapshot, name_text: &str) -> Option<Uri> {
    let (_, _, path) = snapshot.module_programs.get(name_text).or_else(|| {
        let short = name_text.rsplit('.').next().unwrap_or(name_text);
        snapshot.module_programs.get(short)
    })?;
    crate::vfs::path_to_uri(path)
}

/// The range of the module path in `import <path> …`. The path starts right
/// after the `import ` keyword and is ASCII (uppercase segments + dots), so its
/// codepoint width equals its byte length. Matches the locator's `ImportName`
/// span so the clickable region lines up with goto-definition's focus.
fn import_name_range(snapshot: &Snapshot, stmt: FluxSpan, name_text: &str) -> lsp_types::Range {
    let start = FluxPosition {
        line: stmt.start.line,
        column: stmt.start.column + "import ".len(),
    };
    let end = FluxPosition {
        line: start.line,
        column: start.column + name_text.len(),
    };
    snapshot
        .position_map
        .flux_span_to_range(FluxSpan { start, end })
}
