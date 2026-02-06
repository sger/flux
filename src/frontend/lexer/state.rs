//! Lexer state management and string interpolation tracking

use super::Lexer;

#[derive(Debug, Clone)]
pub(super) enum LexerState {
    Normal,
    /// Active interpolated-string context.
    /// Top depth entry tracks the current interpolation expression.
    InInterpolatedString {
        depth_stack: Vec<usize>,
    },
}

impl Lexer {
    pub(super) fn in_interpolated_string_context(&self) -> bool {
        matches!(
            &self.state,
            LexerState::InInterpolatedString { depth_stack } if !depth_stack.is_empty()
        )
    }

    pub(super) fn current_interpolation_depth(&self) -> usize {
        match &self.state {
            LexerState::InInterpolatedString { depth_stack } => {
                depth_stack.last().copied().unwrap_or(0)
            }
            LexerState::Normal => 0,
        }
    }

    pub(super) fn clear_interpolation_state(&mut self) {
        self.state = LexerState::Normal;
    }

    pub(super) fn enter_interpolated_string(&mut self) {
        match &mut self.state {
            LexerState::Normal => {
                self.state = LexerState::InInterpolatedString {
                    depth_stack: vec![1],
                };
            }
            LexerState::InInterpolatedString { depth_stack } => depth_stack.push(1),
        }
    }

    pub(super) fn exit_interpolated_string(&mut self) {
        let mut should_reset = false;
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state {
            depth_stack.pop();
            should_reset = depth_stack.is_empty();
        }
        if should_reset {
            self.clear_interpolation_state();
        }
    }

    pub(super) fn increment_current_interpolation_depth(&mut self) {
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state
            && let Some(depth) = depth_stack.last_mut()
        {
            *depth += 1;
        }
    }

    pub(super) fn decrement_current_interpolation_depth(&mut self) {
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state
            && let Some(depth) = depth_stack.last_mut()
        {
            *depth = depth.saturating_sub(1);
        }
    }

    pub(super) fn reset_current_interpolation_depth(&mut self) {
        if let LexerState::InInterpolatedString { depth_stack } = &mut self.state
            && let Some(depth) = depth_stack.last_mut()
        {
            *depth = 1;
        }
    }

    /// Check if we're currently inside an interpolation expression
    pub fn is_in_interpolation(&self) -> bool {
        self.in_interpolated_string_context() && self.current_interpolation_depth() > 0
    }
}
