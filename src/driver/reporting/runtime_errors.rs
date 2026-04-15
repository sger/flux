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
}

#[cfg(feature = "llvm")]
pub(crate) fn split_native_panic_message(stderr: &str) -> Option<&str> {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("panic: ").map(str::trim))
}

#[cfg(feature = "llvm")]
fn parse_native_trace_frames(stderr: &str) -> Vec<NativeTraceFrame> {
    stderr
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("at ")?;
            if let Some((name, location)) = rest.split_once(" (") {
                let location = location.strip_suffix(')')?;
                if let Some((file, line_no)) = location.rsplit_once(':') {
                    return Some(NativeTraceFrame {
                        name: name.to_string(),
                        file: Some(file.to_string()),
                        line: line_no.parse().ok(),
                    });
                }
            }
            Some(NativeTraceFrame {
                name: rest.to_string(),
                file: None,
                line: None,
            })
        })
        .collect()
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
                .filter(|file| *file == path)
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
pub(crate) fn infer_native_source_frames(path: &str, span: Span) -> Vec<String> {
    if span.start.line == 0 {
        return vec!["main".to_string(), "<main>".to_string()];
    }
    let Ok(source) = std::fs::read_to_string(path) else {
        return vec!["main".to_string(), "<main>".to_string()];
    };
    let lines: Vec<&str> = source.lines().collect();
    let mut enclosing_fn = None;
    for idx in (0..span.start.line.saturating_sub(1)).rev() {
        let trimmed = lines.get(idx).map(|line| line.trim()).unwrap_or_default();
        if let Some(rest) = trimmed.strip_prefix("fn ") {
            let name = rest
                .split(['(', ' ', '{'])
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("main");
            enclosing_fn = Some(name.to_string());
            break;
        }
    }
    let top_frame = enclosing_fn.unwrap_or_else(|| "main".to_string());
    let mut frames = vec![format!(
        "{} ({}:{}:{})",
        top_frame,
        render_display_path(path),
        span.start.line,
        span.start.column + 1
    )];
    if top_frame != "main" {
        let call_site = lines
            .iter()
            .enumerate()
            .find(|(_, line)| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("fn ") && line.contains(&format!("{top_frame}("))
            })
            .map(|(idx, line)| {
                let col = line.find(&top_frame).unwrap_or(0) + 1;
                format!("<main> ({}:{}:{})", render_display_path(path), idx + 1, col)
            })
            .unwrap_or_else(|| "<main>".to_string());
        frames.push(call_site);
    } else {
        frames.push("<main>".to_string());
    }
    frames
}

#[cfg(feature = "llvm")]
pub fn render_native_runtime_error(path: &str, stderr: &str) -> Option<String> {
    let message = split_native_panic_message(stderr)?;
    let frames = parse_native_trace_frames(stderr);
    let span = infer_native_runtime_span(path, message, &frames);
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
    let frames = if !frames.is_empty() {
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
                    format!("{} ({}:{}:1)", frame.name, render_display_path(file), line)
                } else {
                    frame.name.clone()
                }
            })
            .collect()
    } else {
        infer_native_source_frames(path, span)
    };
    Some(render_runtime_diagnostic(
        &diag,
        path,
        source.as_deref(),
        &frames,
    ))
}

#[cfg(not(feature = "llvm"))]
#[allow(dead_code)]
pub fn render_native_runtime_error(_path: &str, _stderr: &str) -> Option<String> {
    None
}
