use flux::syntax::token_type::TokenType;
use flux::syntax::token_type::lookup_ident;

#[cfg(test)]
mod tests {
    use super::*;

    trait TokenTypeTestExt {
        fn is_keyword(&self) -> bool;
    }

    impl TokenTypeTestExt for TokenType {
        fn is_keyword(&self) -> bool {
            matches!(
                self,
                TokenType::Let
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
    fn test_lookup_keywords() {
        assert_eq!(lookup_ident("let"), TokenType::Let);
        assert_eq!(lookup_ident("fun"), TokenType::Fun);
        assert_eq!(lookup_ident("if"), TokenType::If);
        assert_eq!(lookup_ident("else"), TokenType::Else);
        assert_eq!(lookup_ident("return"), TokenType::Return);
        assert_eq!(lookup_ident("true"), TokenType::True);
        assert_eq!(lookup_ident("false"), TokenType::False);
    }

    #[test]
    fn test_lookup_identifiers() {
        assert_eq!(lookup_ident("foo"), TokenType::Ident);
        assert_eq!(lookup_ident("letter"), TokenType::Ident);
        assert_eq!(lookup_ident("funky"), TokenType::Ident);
    }

    #[test]
    fn test_is_keyword() {
        assert!(TokenType::Let.is_keyword());
        assert!(TokenType::Fun.is_keyword());
        assert!(!TokenType::Ident.is_keyword());
        assert!(!TokenType::Plus.is_keyword());
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", TokenType::Plus), "+");
        assert_eq!(format!("{}", TokenType::Let), "let");
        assert_eq!(format!("{}", TokenType::Ident), "IDENT");
    }
}
