use flux::ast::fold::Folder;
use flux::syntax::{
    Identifier, expression::Expression, interner::Interner, lexer::Lexer, parser::Parser,
    program::Program,
};

/// A folder that rewrites identifier `a` to `b` (by symbol).
struct RenameIdent {
    from: Identifier,
    to: Identifier,
}

impl Folder for RenameIdent {
    fn fold_identifier(&mut self, ident: Identifier) -> Identifier {
        if ident == self.from { self.to } else { ident }
    }
}

fn parse(input: &str) -> (Program, Interner) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

#[test]
fn rename_ident_in_let() {
    let (program, mut interner) = parse("let a = 1;");

    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let mut folder = RenameIdent {
        from: sym_a,
        to: sym_b,
    };

    let rewritten = folder.fold_program(program);

    // The let binding name should now be `b`
    let stmt = &rewritten.statements[0];
    match stmt {
        flux::syntax::statement::Statement::Let { name, .. } => {
            assert_eq!(*name, sym_b, "expected identifier 'b' after rename");
        }
        other => panic!("expected Let statement, got {:?}", other),
    }
}

#[test]
fn rename_ident_in_expression() {
    let (program, mut interner) = parse("a + 1;");

    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let mut folder = RenameIdent {
        from: sym_a,
        to: sym_b,
    };

    let rewritten = folder.fold_program(program);

    // The expression statement contains an Infix whose left should be Ident(b)
    let stmt = &rewritten.statements[0];
    match stmt {
        flux::syntax::statement::Statement::Expression { expression, .. } => match expression {
            Expression::Infix { left, .. } => match left.as_ref() {
                Expression::Identifier { name, .. } => {
                    assert_eq!(*name, sym_b, "expected identifier 'b' after rename");
                }
                other => panic!("expected Identifier, got {:?}", other),
            },
            other => panic!("expected Infix expression, got {:?}", other),
        },
        other => panic!("expected Expression statement, got {:?}", other),
    }
}

#[test]
fn rename_preserves_other_idents() {
    let (program, mut interner) = parse("let x = a + y;");

    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");
    let sym_x = interner.intern("x");
    let sym_y = interner.intern("y");

    let mut folder = RenameIdent {
        from: sym_a,
        to: sym_b,
    };

    let rewritten = folder.fold_program(program);

    // `let x = b + y;` — x and y should be unchanged
    let stmt = &rewritten.statements[0];
    match stmt {
        flux::syntax::statement::Statement::Let { name, value, .. } => {
            assert_eq!(*name, sym_x, "let binding name should stay 'x'");
            match value {
                Expression::Infix { left, right, .. } => {
                    match left.as_ref() {
                        Expression::Identifier { name, .. } => {
                            assert_eq!(*name, sym_b, "left operand should be renamed to 'b'");
                        }
                        other => panic!("expected Identifier, got {:?}", other),
                    }
                    match right.as_ref() {
                        Expression::Identifier { name, .. } => {
                            assert_eq!(*name, sym_y, "right operand should stay 'y'");
                        }
                        other => panic!("expected Identifier, got {:?}", other),
                    }
                }
                other => panic!("expected Infix expression, got {:?}", other),
            }
        }
        other => panic!("expected Let statement, got {:?}", other),
    }
}

#[test]
fn rename_in_function_parameters() {
    let (program, mut interner) = parse("fn f(a) { return a; }");

    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let mut folder = RenameIdent {
        from: sym_a,
        to: sym_b,
    };

    let rewritten = folder.fold_program(program);

    match &rewritten.statements[0] {
        flux::syntax::statement::Statement::Function {
            parameters, body, ..
        } => {
            // Parameter `a` should be renamed to `b`
            assert_eq!(parameters[0], sym_b);

            // Return value `a` should also be renamed to `b`
            match &body.statements[0] {
                flux::syntax::statement::Statement::Return {
                    value: Some(expr), ..
                } => match expr {
                    Expression::Identifier { name, .. } => {
                        assert_eq!(*name, sym_b);
                    }
                    other => panic!("expected Identifier, got {:?}", other),
                },
                other => panic!("expected Return statement, got {:?}", other),
            }
        }
        other => panic!("expected Function statement, got {:?}", other),
    }
}

/// Verify that the default Folder implementation is a no-op identity transform.
struct IdentityFolder;

impl Folder for IdentityFolder {}

#[test]
fn identity_fold_preserves_structure() {
    let (program, interner) = parse("let x = 1 + 2;\nfn f(a) { return a; }");

    let original_display = program.display_with(&interner);

    let mut folder = IdentityFolder;
    let rewritten = folder.fold_program(program);

    let rewritten_display = rewritten.display_with(&interner);
    assert_eq!(original_display, rewritten_display);
}
