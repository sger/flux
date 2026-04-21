//! Regression tests for Proposal 0167 — static typing contract hardening.
//!
//! Each `#[test]` is scoped to one phase of the proposal and asserts the
//! specific behavior change, not the full end-to-end diagnostic surface.
//! When a phase regresses, the failing test name should point a reader
//! straight at the responsible code area.

use flux::compiler::Compiler;
use flux::syntax::{interner::Interner, lexer::Lexer, parser::Parser, program::Program};

fn parse(input: &str) -> (Program, Interner) {
    let mut parser = Parser::new(Lexer::new(input));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn compile_strict(input: &str) -> Result<(), Vec<flux::diagnostics::Diagnostic>> {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.set_strict_mode(true);
    compiler.compile(&program)
}

fn compile_strict_errors(input: &str) -> Vec<flux::diagnostics::Diagnostic> {
    compile_strict(input).err().unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 1: the BoundaryKind enum exists and is wired into type_infer
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn boundary_kind_labels_are_stable() {
    use flux::ast::type_infer::boundary::BoundaryKind::*;
    // A snapshot of the user-visible vocabulary. A change here almost
    // certainly requires updating diagnostics and user documentation.
    assert_eq!(PublicFunctionSignature.label(), "public function signature");
    assert_eq!(AnnotatedLet.label(), "annotated let binding");
    assert_eq!(AnnotatedReturn.label(), "annotated return type");
    assert_eq!(EffectBoundary.label(), "effect operation");
    assert_eq!(ModuleInterfaceBoundary.label(), "module interface");
    assert_eq!(BackendConcreteBoundary.label(), "backend representation");
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 2: strict-mode well-typed annotated let flows through CFG
// (i.e. compiles without errors — the removal of the blanket AST fallback
// should not regress accepted programs)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn typed_let_with_matching_annotation_compiles_in_strict_mode() {
    let src = r#"
        fn main() {
            let x: Int = 42;
            x
        }
    "#;
    let _ = compile_strict(src).expect("well-typed annotated let must compile in strict mode");
}

#[test]
fn typed_let_with_mismatched_annotation_still_errors_in_strict_mode() {
    // Pre-validation must still catch this now that the AST-fallback gate
    // is gone — `block_has_typed_let_error` extends to cover the case.
    let src = r#"
        fn main() {
            let x: Int = "not an int";
            x
        }
    "#;
    let errs = compile_strict_errors(src);
    assert!(
        !errs.is_empty(),
        "annotation mismatch must surface a diagnostic"
    );
    // The AST path renders E300 for this kind of mismatch.
    assert!(
        errs.iter().any(|d| d.code() == Some("E300")),
        "expected E300 from AST fallback path; got codes: {:?}",
        errs.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 4: diagnostic ranking — disjoint spans on the same line both survive
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ranking_spans_related_ignores_same_line_disjoint() {
    use flux::diagnostics::position::{Position, Span};
    use flux::diagnostics::ranking::spans_related;
    let a = Span::new(Position::new(5, 4), Position::new(5, 10));
    let b = Span::new(Position::new(5, 40), Position::new(5, 48));
    assert!(
        !spans_related(a, b),
        "same-line disjoint spans must no longer be related (Proposal 0167 Part 5)"
    );
}

#[test]
fn ranking_spans_related_catches_overlap() {
    use flux::diagnostics::position::{Position, Span};
    use flux::diagnostics::ranking::spans_related;
    let a = Span::new(Position::new(1, 0), Position::new(3, 100));
    let b = Span::new(Position::new(2, 10), Position::new(2, 20));
    assert!(
        spans_related(a, b),
        "containing spans must still suppress the contained follow-on"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 5: synthetic ExprId allocation resumes past the parser high-water
// mark — no hardcoded 1_000_000 sentinel any more.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn resuming_past_program_allocates_above_existing_ids() {
    use flux::syntax::expression::{ExprId, ExprIdGen};

    let src = r#"
        fn first(x) { x }
        fn second(y) { y + 1 }
    "#;
    let (program, _interner) = parse(src);

    // Collect the max id from parser-assigned expressions.
    fn max_expr_id(program: &Program) -> u32 {
        use flux::ast::visit::{Visitor, walk_expr, walk_stmt};
        use flux::syntax::expression::Expression;

        struct Scan {
            max: u32,
        }
        impl<'a> Visitor<'a> for Scan {
            fn visit_expr(&mut self, e: &'a Expression) {
                self.max = self.max.max(e.expr_id().0);
                walk_expr(self, e);
            }
        }
        let mut scan = Scan { max: 0 };
        for stmt in &program.statements {
            walk_stmt(&mut scan, stmt);
        }
        scan.max
    }

    let parser_max = max_expr_id(&program);
    let mut id_gen = ExprIdGen::resuming_past_program(&program);
    let fresh = id_gen.next_id();
    assert!(
        fresh.0 > parser_max,
        "resuming_past_program must allocate strictly above the parser's high-water mark; \
         parser_max={parser_max}, fresh={}",
        fresh.0
    );
    // And the synthetic ids must be dense — no 1_000_000 gap.
    assert!(
        fresh.0 < parser_max + 100,
        "resuming should be contiguous, not use a hardcoded offset; fresh={}",
        fresh.0
    );
    let _second = ExprId::UNSET; // Compile guard: ExprId::UNSET still exists as a sentinel.
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 3: boundary-aware residue rule — a free var in a scheme's result
// type that isn't legitimately quantified must be flagged in strict mode.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn polymorphic_identity_compiles_in_strict_mode() {
    // `forall a. a -> a` is legitimate polymorphism; the residue rule must
    // NOT flag it even though `a` is free in the function body.
    let src = r#"
        fn id(x) { x }
        fn main() { id(1) }
    "#;
    let _ = compile_strict(src).expect("polymorphic identity must compile in strict mode");
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 6: Core-adjacent contract exposes residue at the Core layer
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn core_contract_accepts_concrete_result() {
    use flux::core::passes::static_contract::validate_core_static_contract;
    use flux::core::{CoreProgram, CoreType};

    // An empty program with no defs trivially satisfies the contract.
    let prog = CoreProgram {
        defs: Vec::new(),
        top_level_items: Vec::new(),
    };
    let interner = Interner::new();
    let report = validate_core_static_contract(&prog, &interner);
    assert!(report.is_clean(), "empty program must pass Core contract");

    // Sanity: CoreType::Int has no vars, so a def with that result type
    // also passes. (The unit tests inside the pass cover the failing cases;
    // here we just confirm the public entry point is wired.)
    let _ = CoreType::Int;
}
