use std::fmt;

macro_rules! define_tokens {
    (
        symbols { $($sym_name:ident => $sym_str:literal),* $(,)? }
        keywords { $($kw_name:ident => $kw_str:literal),* $(,)? }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum TokenType {
            // Special
            Illegal,
            Eof,

            // Identifiers & Literals
            Ident,
            Int,
            String,

            // Symbols (operators & delimiters)
            $($sym_name,)*

            // Keywords (auto-generated from macro)
            $($kw_name,)*
        }

        impl fmt::Display for TokenType {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let s = match self {
                    TokenType::Illegal => "ILLEGAL",
                    TokenType::Eof => "EOF",
                    TokenType::Ident => "IDENT",
                    TokenType::Int => "INT",
                    TokenType::String => "STRING",
                    $(TokenType::$sym_name => $sym_str,)*
                    $(TokenType::$kw_name => $kw_str,)*
                };
                write!(f, "{}", s)
            }
        }

        /// Called by the lexer to check if an identifier is a keyword
        pub fn lookup_ident(ident: &str) -> TokenType {
            match ident {
                $($kw_str => TokenType::$kw_name,)*
                _ => TokenType::Ident,
            }
        }
    };
}

// ════════════════════════════════════════════════════════════════════════════
//  TOKEN DEFINITIONS
// ════════════════════════════════════════════════════════════════════════════

define_tokens! {
    symbols {
        // Operators
        Plus     => "+",
        Minus    => "-",
        Asterisk => "*",
        Slash    => "/",
        Bang     => "!",
        Lt       => "<",
        Gt       => ">",
        Eq       => "==",
        NotEq    => "!=",
        Assign   => "=",

        // Delimiters
        LParen    => "(",
        RParen    => ")",
        LBrace    => "{",
        RBrace    => "}",
        Comma     => ",",
        Semicolon => ";",
    }

    keywords {
        Let    => "let",
        Fun    => "fun",
        If     => "if",
        Else   => "else",
        Return => "return",
        True   => "true",
        False  => "false",

        // ↓ Add new keywords here ↓
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  TESTS
// ════════════════════════════════════════════════════════════════════════════

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
