use flux::frontend::lexer::Lexer;
use flux::frontend::token_type::TokenType;

#[test]
fn identifier_symbols_are_stable_for_same_name() {
    let mut lexer = Lexer::new("alpha alpha beta");

    let a1 = lexer.next_token();
    let a2 = lexer.next_token();
    let b = lexer.next_token();

    assert_eq!(a1.token_type, TokenType::Ident);
    assert_eq!(a2.token_type, TokenType::Ident);
    assert_eq!(b.token_type, TokenType::Ident);

    assert_eq!(a1.symbol, a2.symbol);
    assert_ne!(a1.symbol, b.symbol);

    let sym = a1.symbol.expect("identifier should be interned");
    assert_eq!(lexer.resolve_symbol(sym), "alpha");
}

#[test]
fn token_spans_slice_source_for_ident_number_and_string() {
    let mut lexer = Lexer::new(r#"foo 123 "a\n""#);

    let ident = lexer.next_token();
    let number = lexer.next_token();
    let string = lexer.next_token();

    assert_eq!(ident.token_type, TokenType::Ident);
    assert_eq!(number.token_type, TokenType::Int);
    assert_eq!(string.token_type, TokenType::String);

    assert_eq!(ident.token_text(lexer.interner()), "foo");
    assert_eq!(number.number_text(), "123");
    assert_eq!(string.string_text(), r#"a\n"#);
}
