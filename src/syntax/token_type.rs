use std::fmt;

macro_rules! define_tokens {
    (
        symbols { $($sym_name:ident => $sym_str:literal),* $(,)? }
        keywords { $($kw_name:ident => $kw_str:literal),* $(,)? }
    ) => {
        #[repr(u16)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum TokenType {
            // Special
            Illegal,
            Eof,

            // Identifiers & Literals
            Ident,
            Int,
            Float,
            String,
            UnterminatedString,
            DocComment,
            UnterminatedBlockComment,

            // Symbols (operators & delimiters)
            $($sym_name,)*

            // Keywords (auto-generated from macro)
            $($kw_name,)*

            // Keep this as the final variant so it always reflects the enum size.
            __Count,
        }

        impl TokenType {
            pub const COUNT: usize = TokenType::__Count as usize;

            pub const fn as_usize(self) -> usize {
                self as usize
            }
        }

        impl fmt::Display for TokenType {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let s = match self {
                    TokenType::Illegal => "ILLEGAL",
                    TokenType::Eof => "EOF",
                    TokenType::Ident => "IDENT",
                    TokenType::Int => "INT",
                    TokenType::Float => "FLOAT",
                    TokenType::String => "STRING",
                    TokenType::UnterminatedString => "UNTERMINATED_STRING",
                    TokenType::DocComment => "DOC_COMMENT",
                    TokenType::UnterminatedBlockComment => "UNTERMINATED_BLOCK_COMMENT",
                    $(TokenType::$sym_name => $sym_str,)*
                    $(TokenType::$kw_name => $kw_str,)*
                    TokenType::__Count => "__COUNT",
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
        Percent => "%",
        Bang     => "!",
        Lt       => "<",
        Gt       => ">",
        Lte      => "<=",
        Gte      => ">=",
        Eq       => "==",
        NotEq    => "!=",
        Assign   => "=",

        // Logical operators
        And => "&&",
        Or => "||",

        // Pipe operator
        Pipe => "|>",
        Bar  => "|",

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
        Dot       => ".",
        Hash      => "#",
        Arrow     => "->",
        Backslash => "\\",
        InterpolationStart => "#{",
        StringEnd => "STRING_END",
    }

    keywords {
        Let    => "let",
        Do     => "do",
        Fn     => "fn",
        Fun    => "fun",
        If     => "if",
        Else   => "else",
        Return => "return",
        True   => "true",
        False  => "false",
        Module => "module",
        Import => "import",
        As     => "as",
        Some   => "Some",
        None   => "None",
        Match  => "match",
        Left   => "Left",
        Right  => "Right",
    }
}
