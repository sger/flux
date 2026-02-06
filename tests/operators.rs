use flux::frontend::{
    lexer::Lexer,
    parser::Parser,
    precedence::{Assoc, Fixity, OPERATOR_TABLE, associativity_of, infix_op, precedence_of},
    token_type::TokenType,
};

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
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors for `{}`: {:?}",
        input,
        parser.errors
    );
    program.to_string()
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
    let generic_dispatch_tokens = [
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
    ];
    let excluded_from_generic_dispatch = [
        TokenType::Pipe,
        TokenType::LParen,
        TokenType::LBracket,
        TokenType::Dot,
    ];

    let mut registry_generic = OPERATOR_TABLE
        .iter()
        .filter(|op| op.fixity == Fixity::Infix)
        .filter(|op| !excluded_from_generic_dispatch.contains(&op.token))
        .map(|op| op.token)
        .collect::<Vec<_>>();
    registry_generic.sort_by_key(|token| token.as_usize());

    let mut parser_generic = generic_dispatch_tokens.to_vec();
    parser_generic.sort_by_key(|token| token.as_usize());

    assert_eq!(
        parser_generic, registry_generic,
        "generic infix parser tokens must exactly match registry infix tokens (excluding special handlers)"
    );

    for token in generic_dispatch_tokens {
        let info = infix_op(&token).expect("generic infix token must exist in operator registry");
        assert_eq!(
            info.fixity,
            Fixity::Infix,
            "generic parser token {:?} must use infix fixity",
            token
        );

        let src = format!("a {} b;", token);
        let expected = format!("(a {} b)", token);
        let got = parse_expr_to_string(&src);
        assert_eq!(
            got, expected,
            "registry token {:?} should be parsed by generic infix dispatch",
            token
        );
    }
}
