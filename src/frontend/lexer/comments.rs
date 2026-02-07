//! Comment handling (block comments and doc comments)

use crate::frontend::lexeme::Lexeme;
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
            self.current_byte() == Some(b'/') && self.peek_byte() == Some(b'*'),
            "skip_block_comment expects current_char == '/' and peek_char == '*'"
        );
        // We need to track nesting depth
        let mut nesting_depth = 1usize;

        // Consume the opening /*
        self.read_char(); // consume '/'
        self.read_char(); // consume '*'

        loop {
            match (self.current_byte(), self.peek_byte()) {
                // Found closing */
                (Some(b'*'), Some(b'/')) => {
                    self.read_char(); // consume '*'
                    self.read_char(); // consume '/'
                    nesting_depth -= 1;
                    if nesting_depth == 0 {
                        return true; // Successfully closed
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    // Found opening /* - increment nesting depth
                    self.read_char(); // consume '/'
                    self.read_char(); // consume '*'
                    nesting_depth += 1;
                }
                (Some(_), _) => self.read_char(),
                (None, _) => return false,
            }
        }
    }

    /// Read a line doc comment (///)
    /// Returns a DocComment token containing the documentation text.
    pub(super) fn read_doc_line_comment(&mut self) -> Token {
        let cursor = self.cursor_position();
        let line = cursor.line;
        let column = cursor.column;

        // Skip the three slashes
        self.read_char(); // first /
        self.read_char(); // second /
        self.read_char(); // third /

        // Skip leading space if present (common convention: "/// text")
        if self.current_byte() == Some(b' ') {
            self.read_char();
        }

        let content_start = self.current_index();
        self.reader.skip_line_comment_body();
        let content_end = self.current_index();

        Token::new_span_with_end(
            TokenType::DocComment,
            self.source_arc(),
            content_start,
            content_end,
            line,
            column,
            self.cursor_position(),
        )
    }

    /// Read a block doc comment (/** ... */)
    /// Returns a DocComment token or UnterminatedBlockComment on error.
    /// Preserves newlines and internal formatting.
    pub(super) fn read_doc_block_comment(&mut self) -> Token {
        let cursor = self.cursor_position();
        let line = cursor.line;
        let column = cursor.column;

        // Skip /** opening
        self.read_char(); // /
        self.read_char(); // *
        self.read_char(); // *

        let mut content = String::new();

        // Handle the empty doc comment `/**/` (overlaps opener/closer).
        if self.current_char() == Some('/') {
            self.read_char(); // consume '/'
            return Token::new_with_end(
                TokenType::DocComment,
                content,
                line,
                column,
                self.cursor_position(),
            );
        }

        // Track nesting for /** ... */ comments
        let mut nesting_depth = 1usize;

        while let Some(ch) = self.current_char() {
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
                        column,
                        self.cursor_position(),
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
            Lexeme::Static(""),
            line,
            column,
            self.cursor_position(),
        )
    }
}
