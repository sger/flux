//! `textDocument/formatting` and `textDocument/rangeFormatting`.
//!
//! Flux's formatter ([`format_source`]) is whole-file: it pretty-prints the
//! entire buffer. Full-document formatting returns one replace-everything edit.
//! Range formatting reuses that — it formats the whole buffer, diffs the result
//! against the original at line granularity, and returns only the change hunks
//! that intersect the requested range, so text outside the selection is left
//! untouched.

use std::ops::Range as StdRange;

use flux::syntax::formatter::format_source;
use line_index::TextSize;
use lsp_types::{Range, TextEdit};

use crate::snapshot::Snapshot;

pub fn format(snapshot: &Snapshot) -> Vec<TextEdit> {
    let formatted = format_source(snapshot.text.as_ref());
    if formatted == snapshot.text.as_ref() {
        return Vec::new();
    }
    vec![TextEdit {
        range: snapshot.position_map.full_document_range(),
        new_text: formatted,
    }]
}

/// Beyond this many lines on either side we skip the O(n·m) line diff and just
/// reformat the whole document — range diffing a file this large is rare and not
/// worth the memory.
const MAX_DIFF_LINES: usize = 20_000;

/// Format only the part of the buffer covered by `range`: format the whole
/// buffer, then keep just the diff hunks whose original lines intersect the
/// selection.
pub fn format_range(snapshot: &Snapshot, range: Range) -> Vec<TextEdit> {
    let original = snapshot.text.as_ref();
    let formatted = format_source(original);
    if formatted == original {
        return Vec::new();
    }

    let old_lines: Vec<&str> = original.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = formatted.split_inclusive('\n').collect();

    // Pathologically large buffer: fall back to a single full-document edit.
    if old_lines.len() > MAX_DIFF_LINES || new_lines.len() > MAX_DIFF_LINES {
        return vec![TextEdit {
            range: snapshot.position_map.full_document_range(),
            new_text: formatted,
        }];
    }

    // Byte offset of the start of each original line (plus end-of-file sentinel).
    let mut line_start = Vec::with_capacity(old_lines.len() + 1);
    let mut off = 0usize;
    for line in &old_lines {
        line_start.push(off);
        off += line.len();
    }
    line_start.push(off);

    let req_first = range.start.line as usize;
    let req_last = range.end.line as usize;

    let mut edits = Vec::new();
    for hunk in line_diff(&old_lines, &new_lines) {
        if !hunk_intersects(&hunk.old, req_first, req_last) {
            continue;
        }
        let start = snapshot
            .position_map
            .offset_to_lsp(TextSize::from(line_start[hunk.old.start] as u32));
        let end = snapshot
            .position_map
            .offset_to_lsp(TextSize::from(line_start[hunk.old.end] as u32));
        edits.push(TextEdit {
            range: Range { start, end },
            new_text: new_lines[hunk.new].concat(),
        });
    }
    edits
}

/// A contiguous block where the original and formatted line streams diverge:
/// original lines `old` are replaced by formatted lines `new`.
struct Hunk {
    old: StdRange<usize>,
    new: StdRange<usize>,
}

/// Whether a hunk's original line span touches the inclusive line range
/// `[first, last]`. A pure insertion (empty `old`) is a point between lines.
fn hunk_intersects(old: &StdRange<usize>, first: usize, last: usize) -> bool {
    if old.start == old.end {
        old.start >= first && old.start <= last
    } else {
        old.start <= last && old.end > first
    }
}

/// Line-level diff via an LCS table, coalesced into replacement hunks. Compares
/// lines by exact text (newline included), so only genuinely changed regions
/// become hunks.
fn line_diff(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let (n, m) = (a.len(), b.len());
    // lcs[i][j] = length of the longest common subsequence of a[i..], b[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut hunks = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (si, sj) = (i, j);
        // Walk the optimal path through the divergent region until lines realign.
        while i < n && j < m && a[i] != b[j] {
            if lcs[i + 1][j] >= lcs[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        hunks.push(Hunk {
            old: si..i,
            new: sj..j,
        });
    }
    if i < n || j < m {
        hunks.push(Hunk {
            old: i..n,
            new: j..m,
        });
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<&str> {
        s.split_inclusive('\n').collect()
    }

    #[test]
    fn diff_isolates_changed_line() {
        let a = lines("one\ntwo\nthree\n");
        let b = lines("one\nTWO\nthree\n");
        let hunks = line_diff(&a, &b);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old, 1..2);
        assert_eq!(hunks[0].new, 1..2);
    }

    #[test]
    fn diff_handles_insertion_and_deletion() {
        let a = lines("a\nb\nc\n");
        let b = lines("a\nc\n");
        let hunks = line_diff(&a, &b);
        // `b` is deleted: original lines 1..2 replaced by nothing.
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old, 1..2);
        assert_eq!(hunks[0].new, 1..1);
    }
}
