use crate::frontend::position::Position;
use crate::frontend::token::Token;
use crate::frontend::token_type::{TokenType, lookup_ident};

/// Warning emitted during lexing
#[derive(Debug, Clone)]
pub struct LexerWarning {
    pub message: String,
    pub position: Position,
}

#[derive(Debug, Clone)]
enum LexerState {
    Normal,
    /// Active interpolated-string context.
    /// Top depth entry tracks the current interpolation expression.
    InInterpolatedString {
        depth_stack: Vec<usize>,
    },
}

/// The Flux lexer
#[derive(Debug, Clone)]
pub struct Lexer {
    input: Vec<char>,
    position: usize,
    read_position: usize,
    current_char: Option<char>,
    line: usize,
    column: usize,
    state: LexerState,
    warnings: Vec<LexerWarning>,
    /// Track unterminated block comment error (position where /* started)
    unterminated_block_comment_pos: Option<Position>,
}

impl Lexer {
    pub fn new(input: impl Into<String>) -> Self {
        let mut lexer = Self {
            input: input.into().chars().collect(),
            position: 0,
            read_position: 0,
            current_char: None,
            line: 1,
            column: 0,
            state: LexerState::Normal,
            warnings: Vec::new(),
            unterminated_block_comment_pos: None,
        };
        lexer.read_char();
        lexer
    }

    /// Get warnings collected during lexing
    pub fn warnings(&self) -> &[LexerWarning] {
        &self.warnings
    }

    /// Get the next token from the input
    pub fn next_token(&mut self) -> Token {
        // If we're in the middle of an interpolated string, continue reading it
        if self.in_interpolated_string_context() && !self.is_in_interpolation() {
            return self.continue_string();
        }

        self.skip_ignorable();

        // Check if we encountered an unterminated block comment
        if let Some(error_pos) = self.unterminated_block_comment_pos.take() {
            return Token::new_with_end(
                TokenType::UnterminatedBlockComment,
                "",
                error_pos.line,
                error_pos.column,
                Position::new(self.line, self.column),
            );
        }

        let line = self.line;
        let col = self.column;

        let token = match self.current_char {
            // Two-character operators
            Some('=') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::Eq, "==", line, col)
            }
            Some('!') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::NotEq, "!=", line, col)
            }
            Some('<') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::Lte, "<=", line, col)
            }
            Some('>') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::Gte, ">=", line, col)
            }
            Some('-') if self.peek_char() == Some('>') => {
                self.read_char();
                Token::new(TokenType::Arrow, "->", line, col)
            }
            // Logical operators
            Some('&') if self.peek_char() == Some('&') => {
                self.read_char();
                Token::new(TokenType::And, "&&", line, col)
            }
            Some('|') if self.peek_char() == Some('|') => {
                self.read_char();
                Token::new(TokenType::Or, "||", line, col)
            }
            // Pipe operator
            Some('|') if self.peek_char() == Some('>') => {
                self.read_char();
                Token::new(TokenType::Pipe, "|>", line, col)
            }
            // Single-character operators and delimiters
            Some('=') => Token::new(TokenType::Assign, "=", line, col),
            Some('!') => Token::new(TokenType::Bang, "!", line, col),
            Some('+') => Token::new(TokenType::Plus, "+", line, col),
            Some('-') => Token::new(TokenType::Minus, "-", line, col),
            Some('*') => Token::new(TokenType::Asterisk, "*", line, col),
            Some('/') => {
                // Doc comments (/// or /**) are tokens; non-doc comments are skipped in
                // skip_ignorable(), so the fallback here is always Slash.
                if self.peek_char() == Some('/') && self.peek_n(2) == Some('/') {
                    return self.read_doc_line_comment();
                }
                if self.peek_char() == Some('*') && self.peek_n(2) == Some('*') {
                    return self.read_doc_block_comment();
                }
                Token::new(TokenType::Slash, "/", line, col)
            }
            Some('%') => Token::new(TokenType::Percent, "%", line, col),
            Some('<') => Token::new(TokenType::Lt, "<", line, col),
            Some('>') => Token::new(TokenType::Gt, ">", line, col),
            Some('(') => Token::new(TokenType::LParen, "(", line, col),
            Some(')') => Token::new(TokenType::RParen, ")", line, col),
            Some('{') => {
                if self.is_in_interpolation() {
                    self.increment_current_interpolation_depth();
                }
                Token::new(TokenType::LBrace, "{", line, col)
            }
            Some('}') => {
                if self.is_in_interpolation() {
                    self.decrement_current_interpolation_depth();
                }
                Token::new(TokenType::RBrace, "}", line, col)
            }
            Some(',') => Token::new(TokenType::Comma, ",", line, col),
            Some(';') => Token::new(TokenType::Semicolon, ";", line, col),
            Some('[') => Token::new(TokenType::LBracket, "[", line, col),
            Some(']') => Token::new(TokenType::RBracket, "]", line, col),
            Some(':') => Token::new(TokenType::Colon, ":", line, col),
            Some('.') => Token::new(TokenType::Dot, ".", line, col),
            Some('\\') => Token::new(TokenType::Backslash, "\\", line, col),

            // String literals
            Some('"') => {
                return self.read_string_start();
            }

            // End of file
            None => {
                // Future improvement: if this is non-empty at EOF, emit a dedicated
                // unterminated interpolation/string diagnostic from the lexer.
                self.clear_interpolation_state();
                Token::new(TokenType::Eof, "", line, col)
            }

            // Identifiers and keywords
            Some(ch) if is_letter(ch) => {
                let ident = self.read_identifier();
                let token_type = lookup_ident(&ident);
                return Token::new(token_type, ident, line, col);
            }

            // Numbers
            Some(ch) if ch.is_ascii_digit() => {
                let (num, is_float) = self.read_number();
                let token_type = if is_float {
                    TokenType::Float
                } else {
                    TokenType::Int
                };
                return Token::new(token_type, num, line, col);
            }

            // Illegal character
            Some(ch) => Token::new(TokenType::Illegal, ch.to_string(), line, col),
        };

        self.read_char();
        token
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();

        loop {
            let token = self.next_token();
            let is_eof = token.token_type == TokenType::Eof;
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        tokens
    }

    fn read_char(&mut self) {
        // Update column BEFORE moving to the next character
        // This ensures column represents the position of current_char, not the next char
        if self.current_char == Some('\n') {
            self.line += 1;
            self.column = 0;
        } else if self.current_char.is_some() {
            self.column += 1;
        }

        self.current_char = if self.read_position >= self.input.len() {
            None
        } else {
            Some(self.input[self.read_position])
        };

        self.position = self.read_position;
        self.read_position += 1;
    }

    fn peek_char(&self) -> Option<char> {
        self.input.get(self.read_position).copied()
    }

    /// Look ahead n chars without advancing.
    /// n=1 is equivalent to peek_char() (next char), n=2 is the char after that.
    /// Returns None when peeking past EOF.
    fn peek_n(&self, n: usize) -> Option<char> {
        debug_assert!(n > 0, "peek_n expects n >= 1");
        self.input.get(self.read_position + (n - 1)).copied()
    }

    fn skip_ignorable(&mut self) {
        loop {
            // Whitespace
            while matches!(self.current_char, Some(' ' | '\t' | '\r' | '\n')) {
                self.read_char();
            }

            // Single-line comments: // (but not ///)
            if self.current_char == Some('/') && self.peek_char() == Some('/') {
                // Check if it's a doc comment ///
                if self.peek_n(2) != Some('/') {
                    // Regular // comment - skip it
                    while self.current_char.is_some() && self.current_char != Some('\n') {
                        self.read_char();
                    }
                    continue; // there may be whitespace/comments again
                }
                // It's a doc comment /// - don't skip, let next_token handle it
                break;
            }

            // Block comments: /* (but not /**)
            if self.current_char == Some('/') && self.peek_char() == Some('*') {
                // Check if it's a doc comment /**
                if self.peek_n(2) != Some('*') {
                    // Regular /* comment - skip it
                    let comment_start = Position::new(self.line, self.column);
                    if !self.skip_block_comment() {
                        // Unterminated block comment - we've hit EOF
                        // Store the error position
                        self.unterminated_block_comment_pos = Some(comment_start);
                        break;
                    }
                    continue; // there may be whitespace/comments again
                }
                // It's a doc comment /** - don't skip, let next_token handle it
                break;
            }

            break;
        }
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        while self
            .current_char
            .is_some_and(|c| is_letter(c) || c.is_ascii_digit())
        {
            self.read_char();
        }
        self.input[start..self.position].iter().collect()
    }

    fn read_number(&mut self) -> (String, bool) {
        let start = self.position;
        while self.current_char.is_some_and(|c| c.is_ascii_digit()) {
            self.read_char();
        }
        let mut is_float = false;
        if self.current_char == Some('.') && self.peek_char().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.read_char(); // consume '.'
            while self.current_char.is_some_and(|c| c.is_ascii_digit()) {
                self.read_char();
            }
        }
        let literal: String = self.input[start..self.position].iter().collect();
        (literal, is_float)
    }

    fn string_token_with_cursor_end(
        &self,
        token_type: TokenType,
        content: String,
        line: usize,
        col: usize,
    ) -> Token {
        // String-family tokens use source cursor end (raw span), not cooked literal length.
        Token::new_with_end(
            token_type,
            content,
            line,
            col,
            Position::new(self.line, self.column),
        )
    }

    fn in_interpolated_string_context(&self) -> bool {
        matches!(
            &self.state,
            LexerState::InInterpolatedString { depth_stack } if !depth_stack.is_empty()
        )
    }

    fn current_interpolation_depth(&self) -> usize {
        match &self.state {
            LexerState::InInterpolatedString { depth_stack } => {
                depth_stack.last().copied().unwrap_or(0)
            }
            LexerState::Normal => 0,
        }
    }

    fn clear_interpolation_state(&mut self) {
        self.state = LexerState::Normal;
    }

    fn enter_interpolated_string(&mut self) {
        match &mut self.state {
            LexerState::Normal => {
                self.state = LexerState::InInterpolatedString {
                    depth_stack: vec![1],
                };
            }
            LexerState::InInterpolatedString { depth_stack } => depth_stack.push(1),
        }
    }

    fn exit_interpolated_string(&mut self) {
        let mut should_reset = false;
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state {
            depth_stack.pop();
            should_reset = depth_stack.is_empty();
        }
        if should_reset {
            self.clear_interpolation_state();
        }
    }

    fn increment_current_interpolation_depth(&mut self) {
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state
            && let Some(depth) = depth_stack.last_mut()
        {
            *depth += 1;
        }
    }

    fn decrement_current_interpolation_depth(&mut self) {
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state
            && let Some(depth) = depth_stack.last_mut()
        {
            *depth = depth.saturating_sub(1);
        }
    }

    fn reset_current_interpolation_depth(&mut self) {
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state
            && let Some(depth) = depth_stack.last_mut()
        {
            *depth = 1;
        }
    }

    /// Read the start of a string (called when we see opening ")
    fn read_string_start(&mut self) -> Token {
        let line = self.line;
        let col = self.column;
        self.read_char(); // skip opening quote

        let (content, ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // String has interpolation - mark that we're in a string
            // Invariant: depth = 1 because we consumed the '{' of '#{' internally.
            self.enter_interpolated_string();
            // Return InterpolationStart instead of String to signal interpolation
            self.string_token_with_cursor_end(TokenType::InterpolationStart, content, line, col)
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.string_token_with_cursor_end(TokenType::UnterminatedString, content, line, col)
        } else {
            // Simple string with no interpolation
            self.string_token_with_cursor_end(TokenType::String, content, line, col)
        }
    }

    /// Continue reading a string after an interpolation expression
    fn continue_string(&mut self) -> Token {
        debug_assert!(self.in_interpolated_string_context());
        debug_assert!(!self.is_in_interpolation());

        let line = self.line;
        let col = self.column;

        let (content, ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // More interpolations to come - reset depth since we consumed the '{' of '#{'
            // Invariant: reset to 1 because '#{' consumed the '{' already.
            self.reset_current_interpolation_depth();
            // Return InterpolationStart to signal another interpolation
            self.string_token_with_cursor_end(TokenType::InterpolationStart, content, line, col)
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(TokenType::UnterminatedString, content, line, col)
        } else {
            // End of interpolated string
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(TokenType::StringEnd, content, line, col)
        }
    }

    /// Read string content until we hit closing quote or interpolation start
    /// Returns (content, ended_with_quote, has_interpolation)
    fn read_string_content(&mut self) -> (String, bool, bool) {
        let mut result = String::new();

        while let Some(c) = self.current_char {
            match c {
                '\n' | '\r' => {
                    // Strings cannot span lines
                    return (result, false, false);
                }
                '"' => {
                    // End of string
                    self.read_char(); // consume closing quote
                    return (result, true, false);
                }
                '#' if self.peek_char() == Some('{') => {
                    // Start of interpolation
                    self.read_char(); // consume '#'
                    self.read_char(); // consume '{'
                    return (result, false, true);
                }
                '\\' => {
                    // Escape sequence
                    self.read_char(); // consume backslash
                    match self.read_escape_sequence() {
                        Some(escaped) => result.push(escaped),
                        None => {
                            // EOF right after backslash inside a string.
                            // Keep the raw backslash in the token literal and terminate.
                            result.push('\\');
                            return (result, false, false);
                        }
                    }
                }
                _ => {
                    result.push(c);
                    self.read_char();
                }
            }
        }

        // Hit EOF without closing quote
        (result, false, false)
    }

    /// Process an escape sequence after seeing backslash
    fn read_escape_sequence(&mut self) -> Option<char> {
        let result = match self.current_char {
            Some('n') => Some('\n'),
            Some('t') => Some('\t'),
            Some('r') => Some('\r'),
            Some('\\') => Some('\\'),
            Some('"') => Some('"'),
            Some('#') => Some('#'), // \# for literal #
            Some(c) => {
                // Unknown escape - emit warning and return the character as-is
                self.warnings.push(LexerWarning {
                    message: format!(
                        "Unknown escape sequence '\\{}'. Valid escapes are: \\n \\t \\r \\\\ \\\" \\#",
                        c
                    ),
                    position: Position::new(self.line, self.column),
                });
                Some(c)
            }
            None => None,
        };
        if self.current_char.is_some() {
            self.read_char();
        }
        result
    }

    /// Check if we're currently inside an interpolation expression
    pub fn is_in_interpolation(&self) -> bool {
        self.in_interpolated_string_context() && self.current_interpolation_depth() > 0
    }

    /// Skip a block comment (/* ... */) with support for nesting.
    /// Entry: current_char is '/' and peek_char is '*' (this function consumes both).
    /// Returns true if the comment was properly closed, false if EOF was reached.
    /// The lexer position is left at the character after the closing */.
    fn skip_block_comment(&mut self) -> bool {
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
    fn read_doc_line_comment(&mut self) -> Token {
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
    fn read_doc_block_comment(&mut self) -> Token {
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

fn is_letter(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}
