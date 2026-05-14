//! Bidirectional position conversion between LSP, Flux, and byte offsets.
//!
//! Flux source positions are 1-based line + 0-based **codepoint** column
//! (counted by `Lexer` walking `char` units). LSP positions are 0-based line +
//! 0-based code-unit offset within a negotiated encoding (UTF-16 by default).
//! Byte offsets are what `line_index::LineIndex` is built around.
//!
//! `PositionMap` owns the source text plus the line index so it can translate
//! freely between all three representations.

use std::sync::Arc;

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use line_index::{LineCol, LineIndex, TextSize, WideEncoding, WideLineCol};
use lsp_types::{Position as LspPosition, Range};

/// Which character offset units the LSP client speaks.
///
/// Default is UTF-16 (the LSP default when the client doesn't advertise
/// `general.position_encodings`). UTF-8 is preferred when the client opts in
/// — it lets us skip the wide-char accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
    Utf32,
}

impl PositionEncoding {
    pub fn as_lsp(self) -> lsp_types::PositionEncodingKind {
        match self {
            Self::Utf8 => lsp_types::PositionEncodingKind::UTF8,
            Self::Utf16 => lsp_types::PositionEncodingKind::UTF16,
            Self::Utf32 => lsp_types::PositionEncodingKind::UTF32,
        }
    }
}

pub struct PositionMap {
    text: Arc<str>,
    index: LineIndex,
    encoding: PositionEncoding,
}

impl PositionMap {
    pub fn new(text: Arc<str>, encoding: PositionEncoding) -> Self {
        let index = LineIndex::new(&text);
        Self {
            text,
            index,
            encoding,
        }
    }

    pub fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    // ── LSP ↔ byte offset ─────────────────────────────────────────────────

    pub fn lsp_to_offset(&self, p: LspPosition) -> Option<TextSize> {
        let line_col = match self.encoding {
            PositionEncoding::Utf8 => LineCol {
                line: p.line,
                col: p.character,
            },
            PositionEncoding::Utf16 => self.index.to_utf8(
                WideEncoding::Utf16,
                WideLineCol {
                    line: p.line,
                    col: p.character,
                },
            )?,
            PositionEncoding::Utf32 => self.index.to_utf8(
                WideEncoding::Utf32,
                WideLineCol {
                    line: p.line,
                    col: p.character,
                },
            )?,
        };
        self.index.offset(line_col)
    }

    pub fn offset_to_lsp(&self, offset: TextSize) -> LspPosition {
        let line_col = self.index.line_col(offset);
        match self.encoding {
            PositionEncoding::Utf8 => LspPosition {
                line: line_col.line,
                character: line_col.col,
            },
            PositionEncoding::Utf16 => self
                .index
                .to_wide(WideEncoding::Utf16, line_col)
                .map(|w| LspPosition {
                    line: w.line,
                    character: w.col,
                })
                .unwrap_or(LspPosition {
                    line: line_col.line,
                    character: line_col.col,
                }),
            PositionEncoding::Utf32 => self
                .index
                .to_wide(WideEncoding::Utf32, line_col)
                .map(|w| LspPosition {
                    line: w.line,
                    character: w.col,
                })
                .unwrap_or(LspPosition {
                    line: line_col.line,
                    character: line_col.col,
                }),
        }
    }

    // ── Flux ↔ byte offset ────────────────────────────────────────────────

    /// Convert a Flux position (1-based line, 0-based codepoint column) into a
    /// UTF-8 byte offset. Walks codepoints from the line start.
    pub fn flux_to_offset(&self, p: FluxPosition) -> Option<TextSize> {
        let line = p.line.checked_sub(1)? as u32;
        let line_range = self.index.line(line)?;
        let line_start: usize = u32::from(line_range.start()) as usize;
        let line_end: usize = u32::from(line_range.end()) as usize;
        let line_text = self.text.get(line_start..line_end)?;
        for (codepoint_idx, (byte_idx, _)) in line_text.char_indices().enumerate() {
            if codepoint_idx == p.column {
                return Some(TextSize::from((line_start + byte_idx) as u32));
            }
        }
        // Column past last codepoint on the line — return end of line.
        Some(TextSize::from((line_start + line_text.len()) as u32))
    }

    /// Convert a UTF-8 byte offset to a Flux position.
    pub fn offset_to_flux(&self, offset: TextSize) -> FluxPosition {
        let line_col = self.index.line_col(offset);
        let line_start = self
            .index
            .line(line_col.line)
            .map(|r| u32::from(r.start()) as usize)
            .unwrap_or(0);
        let target_byte = u32::from(offset) as usize;
        let prefix = self.text.get(line_start..target_byte).unwrap_or("");
        let codepoint_col = prefix.chars().count();
        FluxPosition {
            line: line_col.line as usize + 1,
            column: codepoint_col,
        }
    }

    // ── Composed conversions ──────────────────────────────────────────────

    pub fn flux_to_lsp(&self, p: FluxPosition) -> LspPosition {
        match self.flux_to_offset(p) {
            Some(off) => self.offset_to_lsp(off),
            None => LspPosition {
                line: p.line.saturating_sub(1) as u32,
                character: p.column as u32,
            },
        }
    }

    pub fn lsp_to_flux(&self, p: LspPosition) -> Option<FluxPosition> {
        let off = self.lsp_to_offset(p)?;
        Some(self.offset_to_flux(off))
    }

    pub fn flux_span_to_range(&self, s: FluxSpan) -> Range {
        Range {
            start: self.flux_to_lsp(s.start),
            end: self.flux_to_lsp(s.end),
        }
    }

    pub fn full_document_range(&self) -> Range {
        let end = self.offset_to_lsp(self.index.len());
        Range {
            start: LspPosition {
                line: 0,
                character: 0,
            },
            end,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Encoding negotiation from client `InitializeParams`
// ─────────────────────────────────────────────────────────────────────────────

/// Pick the best mutually-supported encoding. Prefer UTF-8, fall back to UTF-32
/// (rare), then UTF-16 (the LSP default).
pub fn negotiate_encoding(client: Option<&[lsp_types::PositionEncodingKind]>) -> PositionEncoding {
    let Some(supported) = client else {
        return PositionEncoding::Utf16;
    };
    if supported.contains(&lsp_types::PositionEncodingKind::UTF8) {
        PositionEncoding::Utf8
    } else if supported.contains(&lsp_types::PositionEncodingKind::UTF32) {
        PositionEncoding::Utf32
    } else {
        PositionEncoding::Utf16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_round_trip_utf8() {
        let text: Arc<str> = "let x = 1\nlet y = 2\n".into();
        let m = PositionMap::new(text, PositionEncoding::Utf8);
        // `1` is at line 0, column 8 in LSP (and Flux line 1, col 8).
        let lsp = LspPosition {
            line: 0,
            character: 8,
        };
        let off = m.lsp_to_offset(lsp).unwrap();
        assert_eq!(m.offset_to_lsp(off), lsp);
        let flux = m.offset_to_flux(off);
        assert_eq!(flux.line, 1);
        assert_eq!(flux.column, 8);
    }

    #[test]
    fn non_ascii_utf16() {
        // "// é\nlet x = 1\n" — `é` is 2 UTF-8 bytes, 1 UTF-16 code unit, 1 codepoint.
        let text: Arc<str> = "// é\nlet x = 1\n".into();
        let m = PositionMap::new(text.clone(), PositionEncoding::Utf16);
        // Cursor on the `1` on line 1.
        let lsp = LspPosition {
            line: 1,
            character: 8,
        };
        let flux = m.lsp_to_flux(lsp).unwrap();
        assert_eq!(flux.line, 2);
        assert_eq!(flux.column, 8);
    }

    #[test]
    fn full_document_range_matches_text_end() {
        let text: Arc<str> = "abc\nde".into();
        let m = PositionMap::new(text, PositionEncoding::Utf16);
        let r = m.full_document_range();
        assert_eq!(
            r.end,
            LspPosition {
                line: 1,
                character: 2
            }
        );
    }
}
