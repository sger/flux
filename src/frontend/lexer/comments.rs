//! Comment handling (block comments and doc comments)

use crate::frontend::position::Position;
use crate::frontend::token::Token;
use crate::frontend::token_type::TokenType;

use super::Lexer;

impl Lexer {
    /// Skip a block comment (/* ... */) with support for nesting.
    /// Entry: current_char is '/' and peek_char is '*' (this function consumes both).
    /// Returns true if the comment was properly closed, false if EOF was reached.
    /// The lexer position is left at the character after the closing */.
    pub(super) fn skip_block_comment(&mut self) -> bool {
        debug_assert!(
            self.current_char == Some('/') && self.peek_char() == Some('*'),
            "skip_block_comment expects current_char == '/' and peek_char == '*'"
        );
        // We need to track nesting depth
        let mut nesting_depth = 1;

        // Consume the opening /*
        self.read_char(); // consume '/'
        self.read_char(); // consume '*'

        while self.current_char.is_some() {
            if self.current_char == Some('*') && self.peek_char() == Some('/') {
                // Found closing */
                self.read_char(); // consume '*'
                self.read_char(); // consume '/'
                nesting_depth -= 1;
                if nesting_depth == 0 {
                    return true; // Successfully closed
                }
            } else if self.current_char == Some('/') && self.peek_char() == Some('*') {
                // Found opening /* - increment nesting depth
                self.read_char(); // consume '/'
                self.read_char(); // consume '*'
                nesting_depth += 1;
            } else {
                self.read_char();
            }
        }

        // Reached EOF without closing all comments
        false
    }

    /// Read a line doc comment (///)
    /// Returns a DocComment token containing the documentation text.
    pub(super) fn read_doc_line_comment(&mut self) -> Token {
        let line = self.line;
        let col = self.column;

        // Skip the three slashes
        self.read_char(); // first /
        self.read_char(); // second /
        self.read_char(); // third /

        // Skip leading space if present (common convention: "/// text")
        if self.current_char == Some(' ') {
            self.read_char();
        }

        let mut content = String::new();

        // Read until newline or EOF
        while let Some(ch) = self.current_char {
            if ch == '\n' {
                break;
            }
            content.push(ch);
            self.read_char();
        }

        // Use the lexer cursor end to keep spans correct even for multi-line inputs.
        Token::new_with_end(
            TokenType::DocComment,
            content,
            line,
            col,
            Position::new(self.line, self.column),
        )
    }

    /// Read a block doc comment (/** ... */)
    /// Returns a DocComment token or UnterminatedBlockComment on error.
    /// Preserves newlines and internal formatting.
    pub(super) fn read_doc_block_comment(&mut self) -> Token {
        let line = self.line;
        let col = self.column;

        // Skip /** opening
        self.read_char(); // /
        self.read_char(); // *
        self.read_char(); // *

        let mut content = String::new();

        // Handle the empty doc comment `/**/` (overlaps opener/closer).
        if self.current_char == Some('/') {
            self.read_char(); // consume '/'
            return Token::new_with_end(
                TokenType::DocComment,
                content,
                line,
                col,
                Position::new(self.line, self.column),
            );
        }

        // Track nesting for /** ... */ comments
        let mut nesting_depth = 1;

        while let Some(ch) = self.current_char {
            if ch == '*' && self.peek_char() == Some('/') {
                // Found closing */
                self.read_char(); // consume '*'
                self.read_char(); // consume '/'
                nesting_depth -= 1;
                if nesting_depth == 0 {
                    // Successfully closed - return the doc comment
                    return Token::new_with_end(
                        TokenType::DocComment,
                        content,
                        line,
                        col,
                        Position::new(self.line, self.column),
                    );
                }
                // Nested closing delimiter intentionally omitted from doc content.
            } else if ch == '/' && self.peek_char() == Some('*') {
                // Found opening /* - treat as nested for depth, but omit delimiters from content.
                self.read_char(); // consume '/'
                self.read_char(); // consume '*'
                nesting_depth += 1;
            } else {
                content.push(ch);
                self.read_char();
            }
        }

        // Reached EOF without closing the comment
        Token::new_with_end(
            TokenType::UnterminatedBlockComment,
            "", // Use empty literal for all UnterminatedBlockComment tokens (consistency).
            line,
            col,
            Position::new(self.line, self.column),
        )
    }
}
