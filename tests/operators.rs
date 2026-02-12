use flux::diagnostics::Diagnostic;
use flux::syntax::{
    expression::Expression,
    interner::Interner,
    lexer::Lexer,
    parser::Parser,
    precedence::{
        Assoc, Fixity, OPERATOR_TABLE, associativity_of, infix_op, postfix_op, precedence_of,
    },
    program::Program,
    statement::Statement,
    token_type::TokenType,
};
use std::panic;

fn lex_token_types(input: &str) -> Vec<TokenType> {
    let mut lexer = Lexer::new(input);
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token();
        let token_type = tok.token_type;
        tokens.push(token_type);
        if token_type == TokenType::Eof {
            break;
        }
    }
    tokens
}

fn parse_expr_to_string(input: &str) -> String {
    let (program, errors, interner) = parse_program_no_panic(input);
    assert!(
        errors.is_empty(),
        "parser errors for `{}`: {:?}",
        input,
        errors
    );
    program.display_with(&interner)
}

fn parse_program_no_panic(input: &str) -> (Program, Vec<Diagnostic>, Interner) {
    let parsed = panic::catch_unwind(|| {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();
        let errors = parser.errors;
        (program, errors, interner)
    });

    match parsed {
        Ok(result) => result,
        Err(_) => panic!("parser panicked for `{input}`"),
    }
}

fn first_expression(program: &Program) -> &Expression {
    assert_eq!(
        program.statements.len(),
        1,
        "expected exactly one statement, got {}",
        program.statements.len()
    );

    match &program.statements[0] {
        Statement::Expression { expression, .. } => expression,
        other => panic!("expected expression statement, got {:?}", other),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprShape {
    Infix,
    Call,
    Index,
    MemberAccess,
}

fn expression_shape(expr: &Expression) -> ExprShape {
    match expr {
        Expression::Infix { .. } => ExprShape::Infix,
        Expression::Call { .. } => ExprShape::Call,
        Expression::Index { .. } => ExprShape::Index,
        Expression::MemberAccess { .. } => ExprShape::MemberAccess,
        other => panic!("unexpected expression shape in operator test: {:?}", other),
    }
}

#[test]
fn lex_operator_longest_match_and_prefix_collisions() {
    let cases: [(&str, Vec<TokenType>); 9] = [
        (
            "a==b",
            vec![
                TokenType::Ident,
                TokenType::Eq,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "a>=b",
            vec![
                TokenType::Ident,
                TokenType::Gte,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "a<=b",
            vec![
                TokenType::Ident,
                TokenType::Lte,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "a!=b",
            vec![
                TokenType::Ident,
                TokenType::NotEq,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "a->b",
            vec![
                TokenType::Ident,
                TokenType::Arrow,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "x|>f",
            vec![
                TokenType::Ident,
                TokenType::Pipe,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "a&&b||c",
            vec![
                TokenType::Ident,
                TokenType::And,
                TokenType::Ident,
                TokenType::Or,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "a--b",
            vec![
                TokenType::Ident,
                TokenType::Minus,
                TokenType::Minus,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
        (
            "a = = b",
            vec![
                TokenType::Ident,
                TokenType::Assign,
                TokenType::Assign,
                TokenType::Ident,
                TokenType::Eof,
            ],
        ),
    ];

    for (input, expected) in cases {
        let got = lex_token_types(input);
        assert_eq!(got, expected, "tokenization mismatch for `{}`", input);
    }
}

#[test]
fn parse_operator_precedence_and_associativity() {
    let cases = [
        ("1 + 2 * 3;", "(1 + (2 * 3))"),
        ("1 * 2 + 3;", "((1 * 2) + 3)"),
        ("1 + 2 + 3;", "((1 + 2) + 3)"),
        ("1 - 2 - 3;", "((1 - 2) - 3)"),
        ("a < b == c;", "((a < b) == c)"),
        ("a && b || c;", "((a && b) || c)"),
        ("a || b && c;", "(a || (b && c))"),
        // Pipe is lowest precedence and desugars to calls.
        ("1 + 2 |> f;", "f((1 + 2))"),
        ("x |> f |> g;", "g(f(x))"),
        ("x |> add(1);", "add(x, 1)"),
    ];

    for (input, expected) in cases {
        let got = parse_expr_to_string(input);
        assert_eq!(got, expected, "parse grouping mismatch for `{}`", input);
    }
}

#[test]
fn parse_parentheses_override_precedence() {
    let cases = [
        ("(1 + 2) * 3;", "((1 + 2) * 3)"),
        ("1 * (2 + 3);", "(1 * (2 + 3))"),
        ("(a && b) || c;", "((a && b) || c)"),
    ];

    for (input, expected) in cases {
        let got = parse_expr_to_string(input);
        assert_eq!(
            got, expected,
            "parentheses grouping mismatch for `{}`",
            input
        );
    }
}

#[test]
fn parse_unary_vs_binary_operator_disambiguation() {
    let cases = [
        ("-1 * 2;", "((-1) * 2)"),
        ("1*-2;", "(1 * (-2))"),
        ("a - -b;", "(a - (-b))"),
        ("a--b;", "(a - (-b))"),
        ("!true == false;", "((!true) == false)"),
    ];

    for (input, expected) in cases {
        let got = parse_expr_to_string(input);
        assert_eq!(got, expected, "unary/binary mismatch for `{}`", input);
    }
}

#[test]
fn operator_registry_has_precedence_for_all_supported_infix_tokens() {
    let operators = [
        TokenType::Pipe,
        TokenType::Or,
        TokenType::And,
        TokenType::Eq,
        TokenType::NotEq,
        TokenType::Lt,
        TokenType::Gt,
        TokenType::Lte,
        TokenType::Gte,
        TokenType::Plus,
        TokenType::Minus,
        TokenType::Asterisk,
        TokenType::Slash,
        TokenType::Percent,
        TokenType::LParen,
        TokenType::LBracket,
        TokenType::Dot,
    ];

    for token in operators {
        assert!(
            precedence_of(&token).is_some(),
            "expected precedence entry for operator token {:?}",
            token
        );
    }
}

#[test]
fn operator_registry_associativity_matches_current_behavior() {
    let left_assoc = [
        TokenType::Pipe,
        TokenType::Or,
        TokenType::And,
        TokenType::Eq,
        TokenType::NotEq,
        TokenType::Lt,
        TokenType::Gt,
        TokenType::Lte,
        TokenType::Gte,
        TokenType::Plus,
        TokenType::Minus,
        TokenType::Asterisk,
        TokenType::Slash,
        TokenType::Percent,
        TokenType::LParen,
        TokenType::LBracket,
        TokenType::Dot,
    ];

    for token in left_assoc {
        assert_eq!(
            associativity_of(&token),
            Some(Assoc::Left),
            "expected left associativity for {:?}",
            token
        );
    }
}

#[test]
fn generic_infix_dispatch_is_in_sync_with_operator_registry() {
    // These stay specialized in parser dispatch: pipe is lowered, call/index/member are postfix.
    let specialized = [
        (TokenType::Pipe, Fixity::Infix),
        (TokenType::LParen, Fixity::Postfix),
        (TokenType::LBracket, Fixity::Postfix),
        (TokenType::Dot, Fixity::Postfix),
    ];

    for (token, expected_fixity) in specialized {
        match expected_fixity {
            Fixity::Infix => {
                let info = infix_op(&token).unwrap_or_else(|| {
                    panic!("specialized infix token {:?} missing in registry", token)
                });
                assert_eq!(
                    info.fixity, expected_fixity,
                    "specialized token {:?} must keep fixity {:?}",
                    token, expected_fixity
                );
            }
            Fixity::Postfix => {
                let info = postfix_op(&token).unwrap_or_else(|| {
                    panic!("specialized postfix token {:?} missing in registry", token)
                });
                assert_eq!(
                    info.fixity, expected_fixity,
                    "specialized token {:?} must keep fixity {:?}",
                    token, expected_fixity
                );
            }
            Fixity::Prefix => unreachable!(),
        }
    }

    let mut registry_generic = OPERATOR_TABLE
        .iter()
        .filter(|op| op.fixity == Fixity::Infix)
        .filter(|op| op.token != TokenType::Pipe)
        .map(|op| op.token)
        .collect::<Vec<_>>();
    registry_generic.sort_by_key(|token| token.as_usize());

    // For generic infix tokens, parser dispatch uses registry metadata directly.
    for token in registry_generic {
        let src = format!("a {} b;", token);
        let (program, errors, _interner) = parse_program_no_panic(&src);
        assert!(
            errors.is_empty(),
            "expected no parser diagnostics for generic infix {:?}, got {:?}",
            token,
            errors
        );
        let expr = first_expression(&program);
        assert_eq!(
            expression_shape(expr),
            ExprShape::Infix,
            "generic infix token {:?} should parse as Expression::Infix",
            token
        );
    }
}

#[test]
fn operator_registry_nonassoc_is_reserved_for_future_chaining_diagnostics() {
    assert!(
        !OPERATOR_TABLE
            .iter()
            .any(|op| op.associativity == Assoc::Nonassoc),
        "Nonassoc is reserved: add parser diagnostics for non-associative chains before registering Nonassoc operators"
    );
}

#[test]
fn parser_dispatch_is_compatible_with_registered_infix_and_postfix_operators() {
    for op in OPERATOR_TABLE
        .iter()
        .filter(|op| matches!(op.fixity, Fixity::Infix | Fixity::Postfix))
    {
        let (src, expected_shape) = match op.token {
            TokenType::Pipe => ("a |> f;", ExprShape::Call), // lowered by specialized branch
            TokenType::Or => ("a || b;", ExprShape::Infix),
            TokenType::And => ("a && b;", ExprShape::Infix),
            TokenType::Eq => ("a == b;", ExprShape::Infix),
            TokenType::NotEq => ("a != b;", ExprShape::Infix),
            TokenType::Lt => ("a < b;", ExprShape::Infix),
            TokenType::Gt => ("a > b;", ExprShape::Infix),
            TokenType::Lte => ("a <= b;", ExprShape::Infix),
            TokenType::Gte => ("a >= b;", ExprShape::Infix),
            TokenType::Plus => ("a + b;", ExprShape::Infix),
            TokenType::Minus => ("a - b;", ExprShape::Infix),
            TokenType::Asterisk => ("a * b;", ExprShape::Infix),
            TokenType::Slash => ("a / b;", ExprShape::Infix),
            TokenType::Percent => ("a % b;", ExprShape::Infix),
            TokenType::LParen => ("a(1);", ExprShape::Call),
            TokenType::LBracket => ("a[1];", ExprShape::Index),
            TokenType::Dot => ("a.b;", ExprShape::MemberAccess),
            other => panic!("missing parser compatibility fixture for operator token {other:?}"),
        };

        let (program, errors, _interner) = parse_program_no_panic(src);
        assert!(
            errors.is_empty(),
            "operator {:?} emitted parser diagnostics for `{}`: {:?}",
            op.token,
            src,
            errors
        );

        let expr = first_expression(&program);
        assert_eq!(
            expression_shape(expr),
            expected_shape,
            "operator {:?} parsed into unexpected AST shape for `{}`",
            op.token,
            src
        );
    }
}
