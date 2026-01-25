use crate::frontend::token::{Token, TokenType, lookup_ident};

#[derive(Debug, Clone)]
pub struct Lexer {
    input: Vec<char>,
    position: usize,
    read_position: usize,
    ch: Option<char>,
    line: usize,
    column: usize,
}

impl Lexer {
    pub fn new(input: impl Into<String>) -> Self {
        let mut lexer = Self {
            input: input.into().chars().collect(),
            position: 0,
            read_position: 0,
            ch: None,
            line: 1,
            column: 0,
        };
        lexer.read_char();
        lexer
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace();

        let (token_type, literal, token_line, token_column) = match self.ch {
            Some('=') => {
                let (line, column) = (self.line, self.column);

                if self.peek_char() == Some('=') {
                    self.read_char(); // consume second '='
                    (TokenType::Eq, "==".to_string(), line, column)
                } else {
                    (TokenType::Assign, "=".to_string(), line, column)
                }
            }

            Some('!') => {
                let (line, column) = (self.line, self.column);

                if self.peek_char() == Some('=') {
                    self.read_char(); // consume '='
                    (TokenType::NotEq, "!=".to_string(), line, column)
                } else {
                    (TokenType::Bang, "!".to_string(), line, column)
                }
            }

            Some('+') => self.simple(TokenType::Plus, "+"),
            Some('-') => self.simple(TokenType::Minus, "-"),
            Some('*') => self.simple(TokenType::Asterisk, "*"),
            Some('/') => self.simple(TokenType::Slash, "/"),

            Some('<') => self.simple(TokenType::Lt, "<"),
            Some('>') => self.simple(TokenType::Gt, ">"),

            Some('(') => self.simple(TokenType::LParen, "("),
            Some(')') => self.simple(TokenType::RParen, ")"),
            Some('{') => self.simple(TokenType::LBrace, "{"),
            Some('}') => self.simple(TokenType::RBrace, "}"),
            Some(',') => self.simple(TokenType::Comma, ","),
            Some(';') => self.simple(TokenType::Semicolon, ";"),

            Some('"') => {
                let (line, column) = (self.line, self.column);

                let string = self.read_string();
                (TokenType::String, string, line, column)
            }

            None => (TokenType::Eof, "".to_string(), self.line, self.column),

            Some(ch) if is_letter(ch) => {
                let (line, column) = (self.line, self.column);
                let ident = self.read_identifier();
                let token_type = lookup_ident(&ident);

                return Token::new(token_type, ident, line, column);
            }

            Some(ch) if ch.is_ascii_digit() => {
                let (line, column) = (self.line, self.column);
                let num = self.read_number();
                return Token::new(TokenType::Int, num, line, column);
            }

            Some(ch) => {
                let (line, column) = (self.line, self.column);
                (TokenType::Illegal, ch.to_string(), line, column)
            }
        };

        self.read_char();
        Token::new(token_type, literal, token_line, token_column)
    }

    fn read_number(&mut self) -> String {
        let start = self.position;
        while self.ch.is_some_and(|c| c.is_ascii_digit()) {
            self.read_char();
        }
        self.input[start..self.position].iter().collect()
    }

    fn read_string(&mut self) -> String {
        self.read_char();
        let start = self.position;

        while let Some(c) = self.ch {
            if c == '"' {
                break;
            }
            if c == '\0' {
                break;
            }
            self.read_char();
        }

        let out: String = self.input[start..self.position].iter().collect();
        out
    }

    fn simple(&self, token_type: TokenType, literal: &str) -> (TokenType, String, usize, usize) {
        (token_type, literal.to_string(), self.line, self.column)
    }

    fn peek_char(&self) -> Option<char> {
        if self.read_position >= self.input.len() {
            None
        } else {
            Some(self.input[self.read_position])
        }
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;

        while self.ch.is_some_and(|c| is_letter(c) || c.is_ascii_digit()) {
            self.read_char();
        }
        self.input[start..self.position].iter().collect()
    }

    fn read_char(&mut self) {
        self.ch = if self.read_position >= self.input.len() {
            None
        } else {
            Some(self.input[self.read_position])
        };

        self.position = self.read_position;
        self.read_position += 1;

        match self.ch {
            Some('\n') => {
                self.line += 1;
                self.column = 0;
            }
            Some(_) => {
                self.column += 1;
            }
            None => {}
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.ch, Some(' ' | '\t' | '\r' | '\n')) {
            self.read_char();
        }
    }
}

fn is_letter(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

#[cfg(test)]
mod tests {
    use crate::frontend::token::TokenType;

    use super::*;

    #[test]
    fn next_token() {
        let input = r#"
let five = 5;
let ten = 10;

fun add(x, y) {
  x + y;
}

let result = add(five, ten);

if (5 < 10) { return true; } else { return false; }

10 == 10;
10 != 9;

"hello";
"#;
        let mut lexer = Lexer::new(input);
        let next_token = lexer.next_token();
        assert_eq!(next_token.token_type, TokenType::Let);
        assert_eq!(next_token.literal, "let");

        let next_token_identifier = lexer.next_token();
        assert_eq!(next_token_identifier.token_type, TokenType::Ident);
        assert_eq!(next_token_identifier.literal, "five");

        loop {
            let token = lexer.next_token();

            if token.token_type == TokenType::String {
                assert_eq!(token.literal, "hello");
                break;
            }

            if token.token_type == TokenType::Eof {
                panic!("expected string token before EOF");
            }
        }
    }

    #[test]
    fn test_token_positions_basic() {
        let input = "let x = 5;\nreturn x;\n\"hi\";";
        let mut l = Lexer::new(input);

        let toks: Vec<Token> = (0..12).map(|_| l.next_token()).collect();

        // toks[0] = let
        assert_eq!(toks[0].token_type, TokenType::Let);
        assert_eq!(toks[0].position.line, 1);
        assert_eq!(toks[0].position.column, 1);

        // toks[1] = x
        assert_eq!(toks[1].token_type, TokenType::Ident);
        assert_eq!(toks[1].literal, "x");
        assert_eq!(toks[1].position.line, 1);
        assert_eq!(toks[1].position.column, 5);

        // return should be line 2 col 1
        let return_tok = toks
            .iter()
            .find(|t| t.token_type == TokenType::Return)
            .unwrap();
        assert_eq!(return_tok.position.line, 2);
        assert_eq!(return_tok.position.column, 1);

        // string token should be line 3 col 1
        let str_tok = toks
            .iter()
            .find(|t| t.token_type == TokenType::String)
            .unwrap();
        assert_eq!(str_tok.literal, "hi");
        assert_eq!(str_tok.position.line, 3);
        assert_eq!(str_tok.position.column, 1);
    }

    #[test]
    fn test_illegal_characters() {
        let input = "@ let x = 1; #";
        let mut l = Lexer::new(input);

        let t0 = l.next_token();
        assert_eq!(t0.token_type, TokenType::Illegal);
        assert_eq!(t0.literal, "@");

        // then normal tokens continue
        let t1 = l.next_token();
        assert_eq!(t1.token_type, TokenType::Let);

        // last '#'
        while l.next_token().token_type != TokenType::Illegal {}
    }

    #[test]
    fn test_strings() {
        let input = "\"\"; \"a b c\";";
        let mut l = Lexer::new(input);

        let t1 = l.next_token();
        assert_eq!(t1.token_type, TokenType::String);
        assert_eq!(t1.literal, "");
        assert_eq!(l.next_token().token_type, TokenType::Semicolon);

        let t2 = l.next_token();
        assert_eq!(t2.token_type, TokenType::String);
        assert_eq!(t2.literal, "a b c");
        assert_eq!(l.next_token().token_type, TokenType::Semicolon);
    }
}
