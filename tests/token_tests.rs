use flux::syntax::token::Token;
use flux::syntax::token_type::TokenType;

#[cfg(test)]
mod tests {
    use super::*;

    trait TokenTestExt {
        fn is_keyword(&self) -> bool;
    }

    impl TokenTestExt for Token {
        fn is_keyword(&self) -> bool {
            matches!(
                self.token_type,
                TokenType::Let
                    | TokenType::Fn
                    | TokenType::Fun
                    | TokenType::If
                    | TokenType::Else
                    | TokenType::Return
                    | TokenType::True
                    | TokenType::False
            )
        }
    }

    #[test]
    fn test_token_new() {
        let tok = Token::new(TokenType::Let, "let", 1, 5);
        assert_eq!(tok.token_type, TokenType::Let);
        assert_eq!(tok.literal, "let");
        assert_eq!(tok.position.line, 1);
        assert_eq!(tok.position.column, 5);
    }

    #[test]
    fn test_token_is_keyword() {
        let let_token = Token::new(TokenType::Let, "let", 1, 1);
        assert!(let_token.is_keyword());

        let ident_token = Token::new(TokenType::Ident, "foo", 1, 1);
        assert!(!ident_token.is_keyword());

        let plus_token = Token::new(TokenType::Plus, "+", 1, 1);
        assert!(!plus_token.is_keyword());
    }

    #[test]
    fn test_token_display() {
        let tok = Token::new(TokenType::Let, "let", 1, 5);
        let s = format!("{}", tok);
        assert!(s.contains("let"));
        assert!(s.contains("1:5"));
    }
}
