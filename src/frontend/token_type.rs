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
        LBracket  => "[",
        RBracket  => "]",
        Colon     => ":",

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
