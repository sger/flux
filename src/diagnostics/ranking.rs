//! Shared diagnostic ranking / overlap-suppression policy (Proposal 0167 Part 5).
//!
//! Before this module, each pass invented its own rules for deciding whether
//! an existing diagnostic should suppress a newly proposed one:
//!
//! - `hm_expr_typer` used "span overlap OR same line" (four-way check), hard
//!   coded to code `E300`.
//! - `static_type_validation` used "exact span match", hard coded to code
//!   `E430`.
//!
//! The inconsistency meant that identical diagnostic situations were
//! reported differently depending on which pass got there first. Same-line
//! suppression in particular was too broad — unrelated diagnostics at
//! different columns on the same source line would silently swallow each
//! other.
//!
//! This module centralizes the rule so both passes can ask the same
//! question: *"is there already a stronger, related diagnostic at this
//! place?"* The ranking policy is:
//!
//! 1. **Same span** — always a duplicate; suppress the follow-on.
//! 2. **Span containment** — if the existing diagnostic's span contains or
//!    is contained by the candidate's, suppress the follow-on. The wider
//!    diagnostic is treated as the root-cause summary; the narrower one as
//!    its detail. Either way, only one is reported.
//! 3. **Partial overlap** — also suppresses the follow-on.
//! 4. **Same-line suppression is gone.** Two diagnostics on the same line
//!    with disjoint spans are both reported.
//!
//! The filter can optionally be scoped to specific error codes — e.g.
//! "suppress me if any `E300` is already anchored here" — via
//! [`is_suppressed_by`] with a predicate on the existing code. If no
//! predicate is supplied, every existing diagnostic is considered.

use crate::diagnostics::{
    Diagnostic,
    position::{Position, Span},
    types::LabelStyle,
};

/// Returns `true` when the diagnostic at `candidate_span` in `candidate_file`
/// is suppressed by an existing diagnostic in `existing` per the shared
/// ranking policy.
///
/// `accept` filters which existing diagnostics are relevant — for example,
/// `|code| code == Some("E300")` for "suppress me if a type mismatch already
/// covers this region". Pass `|_| true` to consider every diagnostic.
pub fn is_suppressed_by<F>(
    existing: &[Diagnostic],
    candidate_file: &str,
    candidate_span: Span,
    accept: F,
) -> bool
where
    F: Fn(Option<&str>) -> bool,
{
    existing.iter().any(|diag| {
        if !accept(diag.code()) {
            return false;
        }
        // An empty candidate_file means "caller runs in a per-program
        // scope and does not need the file check". In that mode, accept
        // every diagnostic regardless of its recorded file.
        if !candidate_file.is_empty() {
            let diag_file = diag.file().unwrap_or(candidate_file);
            if diag_file != candidate_file {
                return false;
            }
        }
        if let Some(diag_span) = diag.span()
            && spans_related(diag_span, candidate_span)
        {
            return true;
        }
        diag.labels().iter().any(|label| {
            if label.style != LabelStyle::Primary {
                return false;
            }
            spans_related(label.span, candidate_span)
        })
    })
}

/// Two spans are "related" for suppression purposes if their line/column
/// ranges overlap. Same-line proximity alone no longer counts — a disjoint
/// pair on the same line is not related.
///
/// This is the only place the definition of "related spans" lives. Passes
/// should always go through this helper so a future refinement (e.g.
/// containment-aware ranking that always prefers the narrower span) needs
/// a single edit.
pub fn spans_related(a: Span, b: Span) -> bool {
    // Overlap ⇔ a.start ≤ b.end AND b.start ≤ a.end (strict inequality on
    // equality would treat touching spans as non-overlapping, which matches
    // how the legacy `hm_expr_typer::spans_overlap` behaved — both
    // endpoints-inclusive).
    position_leq(a.start, b.end) && position_leq(b.start, a.end)
}

fn position_leq(lhs: Position, rhs: Position) -> bool {
    lhs.line < rhs.line || (lhs.line == rhs.line && lhs.column <= rhs.column)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_line: usize, start_col: usize, end_line: usize, end_col: usize) -> Span {
        Span::new(
            Position::new(start_line, start_col),
            Position::new(end_line, end_col),
        )
    }

    #[test]
    fn overlapping_spans_are_related() {
        assert!(spans_related(span(1, 0, 1, 10), span(1, 5, 1, 15)));
        assert!(spans_related(span(1, 5, 1, 15), span(1, 0, 1, 10)));
    }

    #[test]
    fn containment_counts_as_related() {
        assert!(spans_related(span(1, 0, 3, 100), span(2, 10, 2, 20)));
        assert!(spans_related(span(2, 10, 2, 20), span(1, 0, 3, 100)));
    }

    #[test]
    fn identical_spans_are_related() {
        assert!(spans_related(span(1, 5, 1, 10), span(1, 5, 1, 10)));
    }

    #[test]
    fn same_line_disjoint_spans_are_not_related() {
        // Regression: the pre-unification policy treated these as related
        // because they share a source line. The shared policy does not.
        let a = span(7, 4, 7, 10);
        let b = span(7, 40, 7, 48);
        assert!(!spans_related(a, b));
    }

    #[test]
    fn disjoint_lines_are_not_related() {
        assert!(!spans_related(span(1, 1, 1, 5), span(2, 1, 2, 5)));
    }

    #[test]
    fn touching_spans_are_related() {
        // Inclusive endpoints: [1..5] and [5..10] share col 5.
        // Matches the legacy hm_expr_typer behaviour (position_leq is ≤).
        assert!(spans_related(span(1, 0, 1, 5), span(1, 5, 1, 10)));
    }
}
