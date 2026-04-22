//! Pattern coverage (exhaustiveness and redundancy) checking.
//!
//! Matrix-based usefulness algorithm, in the style of Maranget's
//! "Warnings for pattern matching" (2007). Operates on a normalized
//! pattern representation `Pat` that is independent of the AST and
//! Core IR, so it can be driven from either layer.
//!
//! This module currently exposes:
//!
//! - [`Pat`] — normalized pattern
//! - [`Ctor`] — constructor head
//! - [`TyShape`] — minimal scrutinee-type descriptor for constructor
//!   splitting
//! - [`check_match`] — top-level entry point returning
//!   non-exhaustive witnesses and redundant arm indices
//!
//! It is intentionally not wired into the rest of the pipeline yet;
//! that is a follow-up step (Proposal 0166, incremental rollout).

/// Constructor head for a normalized pattern.
///
/// Literals are tracked as opaque [`LitKey`] values so the checker can
/// reason about distinctness without knowing the underlying type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ctor {
    /// Boolean true / false.
    Bool(bool),
    /// `None`.
    None,
    /// `Some(_)` (arity 1).
    Some,
    /// `Left(_)` (arity 1).
    Left,
    /// `Right(_)` (arity 1).
    Right,
    /// Empty list `[]`.
    Nil,
    /// Cons cell `[h | t]` (arity 2).
    Cons,
    /// Tuple of the given arity.
    Tuple(usize),
    /// User ADT constructor: name and arity.
    Adt(String, usize),
    /// Opaque literal (integer, float, string, char). We never split
    /// over an infinite literal domain; literals only participate in
    /// redundancy checks against other literals with the same key.
    Lit(LitKey),
}

/// Opaque literal identity. Two patterns with equal `LitKey` are
/// considered the same literal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LitKey(pub String);

/// Minimal descriptor of a scrutinee type for constructor splitting.
///
/// Only carries the information the coverage checker needs: the
/// complete constructor set for finite ADT-like domains, and a hint
/// that some domains (integers, strings) are treated as infinite.
#[derive(Debug, Clone)]
pub enum TyShape {
    /// `Bool` — constructors `Bool(true)` and `Bool(false)`.
    Bool,
    /// `Option<T>` — constructors `None` and `Some`.
    Option(Box<TyShape>),
    /// `Either<L, R>`.
    Either(Box<TyShape>, Box<TyShape>),
    /// `List<T>` — constructors `Nil` and `Cons`.
    List(Box<TyShape>),
    /// Tuple of `n` components.
    Tuple(Vec<TyShape>),
    /// User ADT with its complete set of `(ctor_name, field_shapes)`.
    Adt {
        /// ADT type name (used only for diagnostics).
        #[allow(dead_code)] // Used for witness rendering in future work.
        name: String,
        /// Full constructor set.
        ctors: Vec<(String, Vec<TyShape>)>,
    },
    /// Treated as infinite (integers, floats, strings). Only a
    /// wildcard can exhaust such a type.
    Opaque,
}

/// Normalized pattern.
#[derive(Debug, Clone)]
pub enum Pat {
    /// `_` or identifier binder — matches anything.
    Wild,
    /// Constructor pattern; sub-patterns correspond to constructor
    /// fields in declaration order.
    Ctor(Ctor, Vec<Pat>),
}

impl Pat {
    /// Convenience: `Wild`.
    #[allow(dead_code)]
    pub fn wild() -> Self {
        Pat::Wild
    }

    /// Convenience: nullary constructor.
    pub fn nullary(c: Ctor) -> Self {
        Pat::Ctor(c, vec![])
    }
}

/// Result of checking one match expression.
#[derive(Debug, Default)]
pub struct Coverage {
    /// Witness patterns that the match does not cover. Empty iff the
    /// match is exhaustive.
    pub missing: Vec<Pat>,
    /// Indices of arms (0-based) that are unreachable because earlier
    /// arms already cover them.
    pub redundant: Vec<usize>,
}

impl Coverage {
    /// True when every value of the scrutinee type is matched by some
    /// arm.
    pub fn is_exhaustive(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Check a match. `arms` is one pattern per arm (guards handled by
/// `guarded` — a `true` entry means "this arm has a guard and
/// therefore does not prove coverage").
///
/// Returns witnesses for missing patterns and indices of redundant
/// arms.
pub fn check_match(ty: &TyShape, arms: &[(Pat, bool)]) -> Coverage {
    let mut matrix: Vec<Vec<Pat>> = Vec::new();
    let mut redundant = Vec::new();

    for (idx, (pat, guarded)) in arms.iter().enumerate() {
        let row = vec![pat.clone()];
        // An arm is redundant iff the existing matrix already
        // exhausts its pattern — i.e. `is_useful` reports the row
        // adds nothing.
        if !is_useful(&matrix, &row, std::slice::from_ref(ty)) {
            redundant.push(idx);
        }
        // A guarded arm never contributes its pattern to coverage,
        // because the guard may fail at runtime. We still add the
        // row for redundancy analysis of *later* arms with the same
        // pattern, because a later identical *unguarded* arm is
        // still useful (the guard may succeed or fail).
        //
        // Concretely: we add guarded rows as wildcard-free rows so
        // they don't mask later coverage. Simplest correct choice:
        // skip guarded rows in the coverage matrix entirely.
        if !guarded {
            matrix.push(row);
        }
    }

    let missing = missing_witnesses(&matrix, std::slice::from_ref(ty));
    Coverage { missing, redundant }
}

/// Usefulness: does `row` match some value that `matrix` does not?
///
/// `tys` is the type of each column.
fn is_useful(matrix: &[Vec<Pat>], row: &[Pat], tys: &[TyShape]) -> bool {
    // Base: no more columns. A row of arity 0 is useful iff the
    // matrix has no rows (i.e. nothing covers the trivial value).
    if tys.is_empty() {
        return matrix.is_empty();
    }

    // Mismatched arity between row and tys can arise when the
    // adapter produces an over-approximating `Pat::Wild` for a
    // constructor whose declared arity differs from our model
    // (e.g. ADT with unresolved fields, named-field with rest).
    // Treat as "useful" conservatively — we never want to claim
    // false coverage.
    if row.is_empty() {
        return true;
    }

    match &row[0] {
        Pat::Ctor(c, args) => {
            let spec_matrix = specialize(matrix, c, args.len());
            let mut spec_row: Vec<Pat> = args.clone();
            spec_row.extend_from_slice(&row[1..]);
            let spec_tys = specialize_tys(tys, c);
            is_useful(&spec_matrix, &spec_row, &spec_tys)
        }
        Pat::Wild => is_useful_wild(matrix, row, tys),
    }
}

/// Usefulness for a row whose first column is a wildcard. Split out
/// for complexity-budget compliance.
fn is_useful_wild(matrix: &[Vec<Pat>], row: &[Pat], tys: &[TyShape]) -> bool {
    let column_ctors: Vec<Ctor> = collect_head_ctors(matrix);
    let Some(all) = all_ctors(&tys[0]) else {
        // Opaque (infinite) domain: a wildcard is useful unless
        // the matrix contains a wildcard row that covers the
        // remainder.
        let default_matrix = default_matrix(matrix);
        return is_useful(&default_matrix, &row[1..], &tys[1..]);
    };
    if let Some(c) = all.iter().find(|c| !column_ctors.contains(c)).cloned() {
        // Wildcard specializes to the missing constructor.
        let arity = ctor_arity(&c);
        let spec_matrix = specialize(matrix, &c, arity);
        let mut spec_row = vec![Pat::Wild; arity];
        spec_row.extend_from_slice(&row[1..]);
        let spec_tys = specialize_tys(tys, &c);
        return is_useful(&spec_matrix, &spec_row, &spec_tys);
    }
    // Complete set: wildcard is useful iff it is useful under
    // some constructor.
    all.iter().any(|c| {
        let arity = ctor_arity(c);
        let spec_matrix = specialize(matrix, c, arity);
        let mut spec_row = vec![Pat::Wild; arity];
        spec_row.extend_from_slice(&row[1..]);
        let spec_tys = specialize_tys(tys, c);
        is_useful(&spec_matrix, &spec_row, &spec_tys)
    })
}

/// Specialize `matrix` to rows whose first column matches constructor
/// `c`. Each such row is rewritten with `c`'s fields exposed as the
/// first `arity` columns, followed by its remaining columns. Rows
/// starting with a wildcard expand to `arity` fresh wildcards.
fn specialize(matrix: &[Vec<Pat>], c: &Ctor, arity: usize) -> Vec<Vec<Pat>> {
    let mut out = Vec::with_capacity(matrix.len());
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0] {
            Pat::Ctor(rc, args) if rc == c => {
                let mut new_row = args.clone();
                new_row.extend_from_slice(&row[1..]);
                out.push(new_row);
            }
            Pat::Ctor(_, _) => {} // Different ctor: drop row.
            Pat::Wild => {
                let mut new_row = vec![Pat::Wild; arity];
                new_row.extend_from_slice(&row[1..]);
                out.push(new_row);
            }
        }
    }
    out
}

/// Default matrix: rows that begin with a wildcard, with the leading
/// column dropped.
fn default_matrix(matrix: &[Vec<Pat>]) -> Vec<Vec<Pat>> {
    let mut out = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        if matches!(row[0], Pat::Wild) {
            out.push(row[1..].to_vec());
        }
    }
    out
}

/// Specialize the type vector: replace the first type with the field
/// types of `c`, keep the rest.
fn specialize_tys(tys: &[TyShape], c: &Ctor) -> Vec<TyShape> {
    let mut out = match c {
        Ctor::Bool(_) | Ctor::None | Ctor::Nil | Ctor::Lit(_) => Vec::new(),
        Ctor::Some => {
            if let TyShape::Option(inner) = &tys[0] {
                vec![(**inner).clone()]
            } else {
                vec![TyShape::Opaque]
            }
        }
        Ctor::Left => {
            if let TyShape::Either(l, _) = &tys[0] {
                vec![(**l).clone()]
            } else {
                vec![TyShape::Opaque]
            }
        }
        Ctor::Right => {
            if let TyShape::Either(_, r) = &tys[0] {
                vec![(**r).clone()]
            } else {
                vec![TyShape::Opaque]
            }
        }
        Ctor::Cons => {
            if let TyShape::List(inner) = &tys[0] {
                vec![(**inner).clone(), tys[0].clone()]
            } else {
                vec![TyShape::Opaque, TyShape::Opaque]
            }
        }
        Ctor::Tuple(n) => {
            if let TyShape::Tuple(fields) = &tys[0] {
                fields.clone()
            } else {
                vec![TyShape::Opaque; *n]
            }
        }
        Ctor::Adt(name, _) => {
            if let TyShape::Adt { ctors, .. } = &tys[0] {
                ctors
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, fs)| fs.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
    };
    out.extend_from_slice(&tys[1..]);
    out
}

/// Collect the constructor heads appearing in column 0.
fn collect_head_ctors(matrix: &[Vec<Pat>]) -> Vec<Ctor> {
    let mut seen: Vec<Ctor> = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        if let Pat::Ctor(c, _) = &row[0]
            && !seen.contains(c)
        {
            seen.push(c.clone());
        }
    }
    seen
}

/// The complete constructor set for a type, or `None` if the domain
/// is treated as infinite (integers, floats, strings, opaque).
fn all_ctors(ty: &TyShape) -> Option<Vec<Ctor>> {
    match ty {
        TyShape::Bool => Some(vec![Ctor::Bool(true), Ctor::Bool(false)]),
        TyShape::Option(_) => Some(vec![Ctor::None, Ctor::Some]),
        TyShape::Either(_, _) => Some(vec![Ctor::Left, Ctor::Right]),
        TyShape::List(_) => Some(vec![Ctor::Nil, Ctor::Cons]),
        TyShape::Tuple(fs) => Some(vec![Ctor::Tuple(fs.len())]),
        TyShape::Adt { ctors, .. } => Some(
            ctors
                .iter()
                .map(|(n, fs)| Ctor::Adt(n.clone(), fs.len()))
                .collect(),
        ),
        TyShape::Opaque => None,
    }
}

/// Arity of a constructor.
fn ctor_arity(c: &Ctor) -> usize {
    match c {
        Ctor::Bool(_) | Ctor::None | Ctor::Nil | Ctor::Lit(_) => 0,
        Ctor::Some | Ctor::Left | Ctor::Right => 1,
        Ctor::Cons => 2,
        Ctor::Tuple(n) => *n,
        Ctor::Adt(_, n) => *n,
    }
}

/// Generate witness patterns for missing cases. Returns up to a few
/// representative witnesses; empty when the matrix is exhaustive.
///
/// Implemented via the "U" algorithm of Maranget: find a row that
/// would be useful against the matrix and reconstruct it.
fn missing_witnesses(matrix: &[Vec<Pat>], tys: &[TyShape]) -> Vec<Pat> {
    if tys.is_empty() {
        return if matrix.is_empty() {
            vec![Pat::Wild] // unreachable in practice
        } else {
            Vec::new()
        };
    }
    let has_wild_row = matrix
        .iter()
        .any(|r| !r.is_empty() && matches!(r[0], Pat::Wild));
    match all_ctors(&tys[0]) {
        Some(all) => missing_witnesses_known(matrix, tys, &all, has_wild_row),
        None => missing_witnesses_opaque(matrix, tys, has_wild_row),
    }
}

/// Witness search when the column-0 type has a known constructor set.
fn missing_witnesses_known(
    matrix: &[Vec<Pat>],
    tys: &[TyShape],
    all: &[Ctor],
    has_wild_row: bool,
) -> Vec<Pat> {
    let column_ctors = collect_head_ctors(matrix);
    let missing_ctors: Vec<Ctor> = if has_wild_row {
        Vec::new()
    } else {
        all.iter()
            .filter(|c| !column_ctors.contains(c))
            .cloned()
            .collect()
    };
    if !missing_ctors.is_empty() {
        return missing_ctors
            .iter()
            .take(3)
            .map(|c| Pat::Ctor(c.clone(), vec![Pat::Wild; ctor_arity(c)]))
            .collect();
    }
    if has_wild_row {
        let dmat = default_matrix(matrix);
        let sub = missing_witnesses(&dmat, &tys[1..]);
        return if sub.is_empty() {
            Vec::new()
        } else {
            vec![Pat::Wild]
        };
    }
    // Recurse into each ctor to find nested missing patterns.
    for c in all {
        let arity = ctor_arity(c);
        let spec_matrix = specialize(matrix, c, arity);
        let spec_tys = specialize_tys(tys, c);
        let sub = missing_witnesses(&spec_matrix, &spec_tys);
        if !sub.is_empty() {
            return sub
                .into_iter()
                .take(3)
                .map(|w| {
                    let (fields, _) = split_witness(&w, arity);
                    Pat::Ctor(c.clone(), fields)
                })
                .collect();
        }
    }
    Vec::new()
}

/// Witness search on an opaque (unknown / infinite) domain. Conservative:
/// report missing only when the matrix is empty or when a trailing
/// wildcard row leaves later columns non-exhaustive.
fn missing_witnesses_opaque(matrix: &[Vec<Pat>], tys: &[TyShape], has_wild_row: bool) -> Vec<Pat> {
    if matrix.is_empty() {
        return vec![Pat::Wild];
    }
    if !has_wild_row {
        // Literal heads on an opaque (infinite) domain — e.g. `Int`
        // or `String` — can never exhaust the domain: only a
        // wildcard/identifier binding can. Report missing.
        let any_literal_head = matrix
            .iter()
            .any(|r| !r.is_empty() && matches!(&r[0], Pat::Ctor(Ctor::Lit(_), _)));
        if any_literal_head {
            return vec![Pat::Wild];
        }
        // All rows start with a non-literal constructor on an
        // opaque column. This happens when the adapter
        // over-approximates an ADT field as `Opaque` (e.g. the
        // field's surface type didn't resolve). We cannot enumerate
        // the domain, so report no missing pattern — the checker
        // stays sound and silent rather than flagging valid code.
        return Vec::new();
    }
    let dmat = default_matrix(matrix);
    let sub = missing_witnesses(&dmat, &tys[1..]);
    if sub.is_empty() {
        Vec::new()
    } else {
        vec![Pat::Wild]
    }
}

/// Take the first `arity` fields of a reconstructed witness, leaving
/// the remainder. Since our recursion always targets a single column
/// per step, this returns `(fields, rest)` = `(vec![w], vec![])`.
fn split_witness(w: &Pat, arity: usize) -> (Vec<Pat>, Vec<Pat>) {
    // Our witnesses always represent exactly one column, so we
    // expand to `arity` wildcards when the constructor is the witness
    // itself. This keeps the shape consistent for nested rebuilding.
    match w {
        Pat::Ctor(_, _) => (vec![w.clone()], vec![]),
        Pat::Wild => (vec![Pat::Wild; arity], vec![]),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn adt_shape() -> TyShape {
        TyShape::Adt {
            name: "Shape".into(),
            ctors: vec![
                ("Circle".into(), vec![TyShape::Opaque]),
                ("Rect".into(), vec![TyShape::Opaque, TyShape::Opaque]),
            ],
        }
    }

    #[test]
    fn adt_exhaustive_two_ctors() {
        let ty = adt_shape();
        let arms = vec![
            (
                Pat::Ctor(Ctor::Adt("Circle".into(), 1), vec![Pat::Wild]),
                false,
            ),
            (
                Pat::Ctor(Ctor::Adt("Rect".into(), 2), vec![Pat::Wild, Pat::Wild]),
                false,
            ),
        ];
        let cov = check_match(&ty, &arms);
        assert!(cov.is_exhaustive(), "missing: {:?}", cov.missing);
        assert!(cov.redundant.is_empty());
    }

    #[test]
    fn adt_missing_ctor() {
        let ty = adt_shape();
        let arms = vec![(
            Pat::Ctor(Ctor::Adt("Circle".into(), 1), vec![Pat::Wild]),
            false,
        )];
        let cov = check_match(&ty, &arms);
        assert!(!cov.is_exhaustive());
        assert_eq!(cov.missing.len(), 1);
        match &cov.missing[0] {
            Pat::Ctor(Ctor::Adt(n, _), _) => assert_eq!(n, "Rect"),
            _ => panic!("expected Rect witness, got {:?}", cov.missing[0]),
        }
    }

    #[test]
    fn wildcard_covers_all() {
        let ty = adt_shape();
        let arms = vec![(Pat::Wild, false)];
        let cov = check_match(&ty, &arms);
        assert!(cov.is_exhaustive());
    }

    #[test]
    fn redundant_arm_after_wildcard() {
        let ty = TyShape::Bool;
        let arms = vec![(Pat::Wild, false), (Pat::nullary(Ctor::Bool(true)), false)];
        let cov = check_match(&ty, &arms);
        assert!(cov.is_exhaustive());
        assert_eq!(cov.redundant, vec![1]);
    }

    #[test]
    fn bool_missing_false() {
        let ty = TyShape::Bool;
        let arms = vec![(Pat::nullary(Ctor::Bool(true)), false)];
        let cov = check_match(&ty, &arms);
        assert!(!cov.is_exhaustive());
        assert!(
            cov.missing
                .iter()
                .any(|p| matches!(p, Pat::Ctor(Ctor::Bool(false), _)))
        );
    }

    #[test]
    fn list_missing_empty() {
        let ty = TyShape::List(Box::new(TyShape::Opaque));
        // Only [h | t] — missing [].
        let arms = vec![(Pat::Ctor(Ctor::Cons, vec![Pat::Wild, Pat::Wild]), false)];
        let cov = check_match(&ty, &arms);
        assert!(!cov.is_exhaustive());
        assert!(
            cov.missing
                .iter()
                .any(|p| matches!(p, Pat::Ctor(Ctor::Nil, _)))
        );
    }

    #[test]
    fn list_exhaustive_nil_and_cons() {
        let ty = TyShape::List(Box::new(TyShape::Opaque));
        let arms = vec![
            (Pat::nullary(Ctor::Nil), false),
            (Pat::Ctor(Ctor::Cons, vec![Pat::Wild, Pat::Wild]), false),
        ];
        let cov = check_match(&ty, &arms);
        assert!(cov.is_exhaustive());
    }

    #[test]
    fn option_exhaustive() {
        let ty = TyShape::Option(Box::new(TyShape::Opaque));
        let arms = vec![
            (Pat::nullary(Ctor::None), false),
            (Pat::Ctor(Ctor::Some, vec![Pat::Wild]), false),
        ];
        let cov = check_match(&ty, &arms);
        assert!(cov.is_exhaustive());
    }

    #[test]
    fn option_missing_none() {
        let ty = TyShape::Option(Box::new(TyShape::Opaque));
        let arms = vec![(Pat::Ctor(Ctor::Some, vec![Pat::Wild]), false)];
        let cov = check_match(&ty, &arms);
        assert!(!cov.is_exhaustive());
        assert!(
            cov.missing
                .iter()
                .any(|p| matches!(p, Pat::Ctor(Ctor::None, _)))
        );
    }

    #[test]
    fn tuple_wildcard_exhaustive() {
        let ty = TyShape::Tuple(vec![TyShape::Bool, TyShape::Bool]);
        let arms = vec![(Pat::Ctor(Ctor::Tuple(2), vec![Pat::Wild, Pat::Wild]), false)];
        let cov = check_match(&ty, &arms);
        assert!(cov.is_exhaustive());
    }

    #[test]
    fn guarded_arm_does_not_cover() {
        let ty = TyShape::Bool;
        // `true if g -> _` (guarded) then no further arm — must
        // report missing `true` AND `false`.
        let arms = vec![(Pat::nullary(Ctor::Bool(true)), true)];
        let cov = check_match(&ty, &arms);
        assert!(!cov.is_exhaustive());
    }

    #[test]
    fn guarded_then_unguarded_same_pattern_is_useful() {
        let ty = TyShape::Bool;
        let arms = vec![
            (Pat::nullary(Ctor::Bool(true)), true),
            (Pat::nullary(Ctor::Bool(true)), false),
            (Pat::nullary(Ctor::Bool(false)), false),
        ];
        let cov = check_match(&ty, &arms);
        assert!(cov.is_exhaustive());
        assert!(cov.redundant.is_empty());
    }

    #[test]
    fn nested_adt_missing_inner_ctor() {
        // Outer: Option<Shape>; match Some(Circle(_)) and None
        // leaves Some(Rect(_, _)) uncovered.
        let ty = TyShape::Option(Box::new(adt_shape()));
        let arms = vec![
            (
                Pat::Ctor(
                    Ctor::Some,
                    vec![Pat::Ctor(Ctor::Adt("Circle".into(), 1), vec![Pat::Wild])],
                ),
                false,
            ),
            (Pat::nullary(Ctor::None), false),
        ];
        let cov = check_match(&ty, &arms);
        assert!(!cov.is_exhaustive(), "expected missing Some(Rect(_, _))");
    }
}
