//! Richer goto-definition result, mirroring rust-analyzer's
//! [`NavigationTarget`](../../../E:/Github/rust-analyzer/crates/ide/src/navigation_target.rs)
//! and GHC's `NameAnn`/`EpAnn` split
//! (`compiler/GHC/Parser/Annotation.hs:581-635`): the outer anchor and
//! the identifier-only sub-span are first-class and distinct.
//!
//! Ranges are stored in *LSP coordinates* (already converted via the
//! destination file's `PositionMap`). This lets a goto-def branch that
//! produces a target in a different file (cross-module) do the
//! conversion where the right `PositionMap` is in scope; the LSP
//! boundary in `global_state::handle_definition` doesn't need to
//! re-derive it.
//!
//! Lowered to [`lsp_types::LocationLink`] at the LSP boundary —
//! `target_range` carries `full_range`, `target_selection_range` carries
//! `focus_range`, giving VS Code's peek view the precise identifier
//! highlight that GHC/HLS users expect.

use lsp_types::{LocationLink, Range, Uri};

/// One goto-definition destination plus the metadata an IDE peek view
/// renders around it. Ranges are pre-converted to LSP coordinates
/// against the destination file's position map.
#[derive(Debug, Clone)]
pub struct NavigationTarget {
    pub uri: Uri,
    /// Whole declaration extent — the signature plus body for `fn`, the
    /// entire `let x = expr` for `let`, the whole `data ... { ... }` for
    /// data. Becomes `LocationLink::target_range`.
    pub full_range: Range,
    /// Identifier text only — just `foo` in `fn foo(x, y) { ... }`.
    /// Becomes `LocationLink::target_selection_range`. When the upstream
    /// data can't distinguish (e.g. effect ops carry only one span), it
    /// equals `full_range`.
    pub focus_range: Range,
    /// Display name — the identifier the user lands on. Not surfaced
    /// through `LocationLink` today but kept for future hover/peek
    /// metadata (matching rust-analyzer's `NavigationTarget::name`).
    pub name: String,
}

impl NavigationTarget {
    /// Build a target whose `full_range` and `focus_range` coincide.
    /// Used when only one range is meaningful (e.g. import statement
    /// where the alias position is the whole declaration, or upstream
    /// data that exposes only one span).
    pub fn collapsed(uri: Uri, range: Range, name: impl Into<String>) -> Self {
        Self {
            uri,
            full_range: range,
            focus_range: range,
            name: name.into(),
        }
    }

    /// Lower to a `LocationLink` for clients that requested
    /// `linkSupport`. `origin_selection_range` is the source-side span
    /// the cursor word covers — computed at the LSP boundary where the
    /// source buffer is in scope.
    pub fn into_location_link(self, origin_selection_range: Option<Range>) -> LocationLink {
        LocationLink {
            origin_selection_range,
            target_uri: self.uri,
            target_range: self.full_range,
            target_selection_range: self.focus_range,
        }
    }
}
