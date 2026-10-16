#[cfg(feature = "llvm")]
use crate::diagnostics::{
    self, Diagnostic, DiagnosticPhase,
    position::{Position, Span},
    quality::render_runtime_diagnostic,
    render_display_path,
};

#[cfg(feature = "llvm")]
struct NativeTraceFrame {
    name: String,
    file: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
}

#[cfg(feature = "llvm")]
pub(crate) fn split_native_panic_message(stderr: &str) -> Option<&str> {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("panic: ").map(str::trim))
}

/// The frames of the trace a native program printed (0188 S10a): `at name
/// (file:line:col)`, `at <main>`, and `... N more frames ...` where it cut a
/// long one.
#[cfg(feature = "llvm")]
fn parse_native_trace_frames(stderr: &str) -> Vec<NativeTraceFrame> {
    let named = |name: &str| NativeTraceFrame {
        name: name.to_string(),
        file: None,
        line: None,
        column: None,
    };
    stderr
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with("... ") && line.ends_with(" more frames ...") {
                return Some(named(line));
            }
            let rest = line.strip_prefix("at ")?;
            if let Some((name, location)) = rest.split_once(" (") {
                let location = location.strip_suffix(')')?;
                let (file, column) = match location.rsplit_once(':') {
                    Some((file, column)) if file.contains(':') => (file, column.parse().ok()),
                    _ => (location, None),
                };
                if let Some((file, line_no)) = file.rsplit_once(':') {
                    return Some(NativeTraceFrame {
                        name: name.to_string(),
                        file: Some(file.to_string()),
                        line: line_no.parse().ok(),
                        column,
                    });
                }
            }
            Some(named(rest))
        })
        .collect()
}

/// The runtime's ` @L:C` names where an expression starts (`a / b`): the
/// span runs on to its end, as the VM's does, stopping at a `)` or `,` it
/// did not open.
#[cfg(feature = "llvm")]
fn expression_span(path: &str, span: Span) -> Span {
    let Some(text) = std::fs::read_to_string(path).ok().and_then(|source| {
        source
            .lines()
            .nth(span.start.line.checked_sub(1)?)
            .map(str::to_string)
    }) else {
        return span;
    };
    let Some(rest) = text.get(span.start.column..) else {
        return span;
    };
    let mut depth = 0usize;
    let mut end = rest.len();
    for (offset, c) in rest.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' | ',' if depth == 0 => {
                end = offset;
                break;
            }
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    let end = span.start.column + rest[..end].trim_end().trim_end_matches(';').len();
    if end <= span.start.column {
        return span;
    }
    Span::new(span.start, Position::new(span.start.line, end))
}

/// Where the error happened, from the innermost frame of the program's own
/// trace (0188 S10a), when a call starts there (`panic(…)`, `unwrap(…)`).
/// Elsewhere (an operator) the span is inferred, on that frame's line.
#[cfg(feature = "llvm")]
fn traced_span(path: &str, frames: &[NativeTraceFrame]) -> Option<Span> {
    let frame = frames.first()?;
    if !frame
        .file
        .as_deref()
        .is_some_and(|file| same_file(file, path))
    {
        return None;
    }
    let (line, column) = (frame.line?, frame.column?.checked_sub(1)?);
    let source = std::fs::read_to_string(path).ok()?;
    let text = source.lines().nth(line.checked_sub(1)?)?;
    let rest = text.get(column..)?;
    let name = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(rest.len());
    if name == 0 || !rest[name..].starts_with('(') {
        return None;
    }
    let mut end = column + name;
    {
        let mut depth = 0usize;
        for (offset, c) in rest[name..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = column + name + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    Some(Span::new(
        Position::new(line, column),
        Position::new(line, end),
    ))
}

/// Whether `file`, as a native program's line table names it, is `path`.
#[cfg(feature = "llvm")]
fn same_file(file: &str, path: &str) -> bool {
    file == path
        || match (std::fs::canonicalize(file), std::fs::canonicalize(path)) {
            (Ok(file), Ok(path)) => file == path,
            _ => false,
        }
}

#[cfg(feature = "llvm")]
fn split_native_panic_message_and_span(stderr: &str) -> Option<(&str, Option<Span>)> {
    let message = split_native_panic_message(stderr)?;
    let Some((base, location)) = message.rsplit_once(" @") else {
        return Some((message, None));
    };
    let Some((line, column)) = location.split_once(':') else {
        return Some((message, None));
    };
    let Ok(line) = line.parse::<usize>() else {
        return Some((message, None));
    };
    let Ok(column_1_based) = column.parse::<usize>() else {
        return Some((message, None));
    };
    if line == 0 || column_1_based == 0 {
        return Some((message, None));
    }
    let column = column_1_based - 1;
    Some((
        base,
        Some(Span::new(
            Position::new(line, column),
            Position::new(line, column + 1),
        )),
    ))
}

#[cfg(feature = "llvm")]
fn infer_native_runtime_span(path: &str, message: &str, frames: &[NativeTraceFrame]) -> Span {
    let Ok(source) = std::fs::read_to_string(path) else {
        return Span::new(Position::default(), Position::default());
    };
    let lines: Vec<&str> = source.lines().collect();
    let preferred_line = frames
        .first()
        .and_then(|frame| {
            frame
                .file
                .as_deref()
                .filter(|file| same_file(file, path))
                .and(frame.line)
        })
        .filter(|line| *line > 0);

    let find_needle = |needle: &str| -> Option<Span> {
        if let Some(line_no) = preferred_line
            && let Some(line) = lines.get(line_no.saturating_sub(1))
            && let Some(col) = line.find(needle)
        {
            let mut end = col + needle.len();
            if needle.ends_with('(')
                && let Some(close_off) = line[col..].find(')')
            {
                end = col + close_off + 1;
            }
            return Some(Span::new(
                Position::new(line_no, col),
                Position::new(line_no, end),
            ));
        }
        for (idx, line) in lines.iter().enumerate() {
            if let Some(col) = line.find(needle) {
                let mut end = col + needle.len();
                if needle.ends_with('(')
                    && let Some(close_off) = line[col..].find(')')
                {
                    end = col + close_off + 1;
                }
                return Some(Span::new(
                    Position::new(idx + 1, col),
                    Position::new(idx + 1, end),
                ));
            }
        }
        None
    };
    let find_rhs_span = |operator: char| -> Option<Span> {
        if let Some(line_no) = preferred_line
            && let Some(line) = lines.get(line_no.saturating_sub(1))
            && let Some(op_col) = line.find(operator)
        {
            let start = line
                .chars()
                .position(|c| !c.is_whitespace())
                .unwrap_or(op_col);
            let end = line.trim_end().trim_end_matches(';').len();
            return Some(Span::new(
                Position::new(line_no, start),
                Position::new(line_no, end),
            ));
        }
        for (idx, line) in lines.iter().enumerate() {
            if let Some(op_col) = line.find(operator) {
                let start = line
                    .find('=')
                    .and_then(|eq_col| {
                        line[eq_col + 1..]
                            .chars()
                            .position(|c| !c.is_whitespace())
                            .map(|off| eq_col + 1 + off)
                    })
                    .unwrap_or_else(|| {
                        line.chars()
                            .position(|c| !c.is_whitespace())
                            .unwrap_or(op_col)
                    });
                let end = line.trim_end().trim_end_matches(';').len();
                return Some(Span::new(
                    Position::new(idx + 1, start),
                    Position::new(idx + 1, end),
                ));
            }
        }
        None
    };

    if let Some(rest) = message.strip_prefix("primop ")
        && let Some((primop, _)) = rest.split_once(" expected ")
        && let Some(span) = find_needle(&format!("{primop}("))
    {
        return span;
    }
    if message.contains("wrong number of arguments")
        || message.contains("Cannot call non-function value")
    {
        for (idx, line) in lines.iter().enumerate().rev() {
            let trimmed = line.trim();
            if trimmed.starts_with("fn ")
                || trimmed.starts_with("//")
                || trimmed == "{"
                || trimmed == "}"
                || trimmed.is_empty()
            {
                continue;
            }
            if let Some(open) = line.find('(') {
                let mut start = open;
                while start > 0 {
                    let ch = line.as_bytes()[start - 1] as char;
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                        start -= 1;
                    } else {
                        break;
                    }
                }
                let end = line[open..]
                    .find(')')
                    .map(|off| open + off + 1)
                    .unwrap_or(open + 1);
                return Span::new(Position::new(idx + 1, start), Position::new(idx + 1, end));
            }
        }
    }
    if message.contains("Division by zero") {
        if let Some(span) = find_rhs_span('/') {
            return span;
        }
        if let Some(span) = find_rhs_span('%') {
            return span;
        }
    }
    if message.contains("modulo by zero")
        && let Some(span) = find_rhs_span('%')
    {
        return span;
    }
    if let Some(span) = find_rhs_span('+') {
        return span;
    }
    if let Some(span) = find_rhs_span('-') {
        return span;
    }
    if let Some(span) = find_rhs_span('*') {
        return span;
    }
    Span::new(Position::default(), Position::default())
}

#[cfg(feature = "llvm")]
pub fn render_native_runtime_error(path: &str, stderr: &str) -> Option<String> {
    let (message, encoded_span) = split_native_panic_message_and_span(stderr)?;
    let frames = parse_native_trace_frames(stderr);
    // The error is in the innermost frame's file, which may be another
    // module than the program's (0188 S10a).
    let error_file = frames
        .first()
        .and_then(|frame| frame.file.as_deref())
        .filter(|file| !same_file(file, path))
        .map(|file| render_display_path(file).into_owned());
    let path = error_file.as_deref().unwrap_or(path);
    let span = encoded_span
        .map(|span| expression_span(path, span))
        .or_else(|| traced_span(path, &frames))
        .unwrap_or_else(|| infer_native_runtime_span(path, message, &frames));
    let diag = if message.contains("Cannot call non-function value") {
        let actual = message
            .split("(got ")
            .nth(1)
            .and_then(|s| s.split(')').next())
            .map(|s| s.trim_end_matches('.'))
            .unwrap_or("Unknown");
        Diagnostic::make_error(
            &diagnostics::NOT_A_FUNCTION,
            &[actual],
            path.to_string(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime)
    } else if message.contains("Division by zero") {
        Diagnostic::make_error(
            &diagnostics::DIVISION_BY_ZERO_RUNTIME,
            &[],
            path.to_string(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime)
    } else if message.contains("modulo by zero") {
        Diagnostic::make_error(
            &diagnostics::MODULO_BY_ZERO_RUNTIME,
            &[],
            path.to_string(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime)
    } else if let Some(rest) = message.strip_prefix("Cannot ")
        && let Some((op, tail)) = rest.split_once(' ')
        && let Some((lhs, rhs_tail)) = tail.split_once(" and ")
        && let Some(rhs) = rhs_tail.strip_suffix(" values.")
    {
        Diagnostic::make_error(
            &diagnostics::INVALID_OPERATION,
            &[op, lhs, rhs],
            path.to_string(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime)
    } else {
        let error_code = if message.contains("wrong number of arguments") {
            "E1000"
        } else if message.contains(" expected ") || message.contains("expects ") {
            "E1004"
        } else {
            "E1009"
        };
        Diagnostic::make_error_dynamic(
            error_code,
            message,
            diagnostics::types::ErrorType::Runtime,
            "",
            None,
            path.to_string(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime)
    };
    let source = std::fs::read_to_string(path).ok();
    let stack_frames = if !frames.is_empty() {
        frames
            .iter()
            .enumerate()
            .map(|(idx, frame)| {
                if idx == 0 && span.start.line > 0 {
                    format!(
                        "{} ({}:{}:{})",
                        frame.name,
                        render_display_path(path),
                        span.start.line,
                        span.start.column + 1
                    )
                } else if let (Some(file), Some(line)) = (&frame.file, frame.line) {
                    let file = if same_file(file, path) {
                        path
                    } else {
                        file.as_str()
                    };
                    format!(
                        "{} ({}:{}:{})",
                        frame.name,
                        render_display_path(file),
                        line,
                        frame.column.unwrap_or(1)
                    )
                } else {
                    frame.name.clone()
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(render_runtime_diagnostic(
        &diag,
        path,
        source.as_deref(),
        &stack_frames,
    ))
}

#[cfg(not(feature = "llvm"))]
#[allow(dead_code)]
pub fn render_native_runtime_error(_path: &str, _stderr: &str) -> Option<String> {
    None
}
