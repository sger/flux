use crate::{
    bytecode::op_code::{OpCode, operand_widths, read_u8, read_u16, read_u32},
    diagnostics::{
        Diagnostic, DiagnosticPhase, ErrorCode, ErrorType, RUNTIME_TYPE_ERROR,
        position::{Position, Span},
        render_display_path, render_runtime_diagnostic, runtime_type_error,
    },
    runtime::frame::Frame,
};

use super::VM;

impl VM {
    pub(super) fn runtime_error_from_string(&self, message: &str) -> String {
        let (message, hint) = split_hint(message);
        let (title, details) = split_first_line(message);

        let (file, span) = self.current_location().unwrap_or_else(|| {
            (
                String::from("<unknown>"),
                Span::new(Position::default(), Position::default()),
            )
        });

        // Determine error code based on error message pattern
        let error_code = if title.contains("wrong number of arguments") {
            "E1000" // WRONG_NUMBER_OF_ARGUMENTS
        } else if title.contains("division by zero") {
            "E1008" // DIVISION_BY_ZERO_RUNTIME
        } else if title.contains("not a function") {
            "E1001" // NOT_A_FUNCTION
        } else if title.contains("expected") || title.contains("expects") {
            "E1004" // RUNTIME_TYPE_ERROR
        } else {
            "E1009" // INVALID_OPERATION fallback for unmapped runtime failures
        };

        // Create a dynamic runtime error using Diagnostic::make_error_dynamic
        let diag = Diagnostic::make_error_dynamic(
            error_code,
            title.trim(),
            ErrorType::Runtime,
            details.trim(),
            hint.map(|h| h.trim().to_string()),
            file.clone(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime);
        self.render_runtime_diagnostic(&diag, &file)
    }

    pub(super) fn runtime_error_enhanced(
        &self,
        error_code: &'static ErrorCode,
        values: &[&str],
    ) -> String {
        let (file, span) = self.current_location().unwrap_or_else(|| {
            (
                String::from("<unknown>"),
                Span::new(Position::default(), Position::default()),
            )
        });

        let diag = if error_code.code == RUNTIME_TYPE_ERROR.code && values.len() >= 2 {
            runtime_type_error(
                values[0],
                values[1],
                values.get(2).copied(),
                file.clone(),
                span,
            )
        } else {
            Diagnostic::make_error(error_code, values, file.clone(), span)
                .with_phase(DiagnosticPhase::Runtime)
        };
        self.render_runtime_diagnostic(&diag, &file)
    }

    pub(super) fn runtime_type_error_enhanced(
        &self,
        expected: &str,
        actual: &str,
        value_preview: Option<&str>,
    ) -> String {
        let mut values = vec![expected, actual];
        if let Some(value_preview) = value_preview {
            values.push(value_preview);
        }
        self.runtime_error_enhanced(&RUNTIME_TYPE_ERROR, &values)
    }

    pub(super) fn current_location(&self) -> Option<(String, Span)> {
        let debug_info = self.current_frame().closure.function.debug_info.as_ref()?;
        let location = debug_info.location_at(self.current_frame().ip)?;
        let file = debug_info.file_for(location.file_id)?;
        Some((file.to_string(), location.span))
    }

    pub(super) fn format_frame(&self, frame: &Frame) -> (String, Option<String>) {
        let debug_info = frame.closure.function.debug_info.as_ref();
        let name = debug_info
            .and_then(|info| info.name.clone())
            .unwrap_or_else(|| "<anonymous>".to_string());
        let location = debug_info.and_then(|info| {
            info.location_at(frame.ip).and_then(|loc| {
                info.file_for(loc.file_id).map(|file| {
                    format!(
                        "{}:{}:{}",
                        render_display_path(file),
                        loc.span.start.line,
                        loc.span.start.column + 1
                    )
                })
            })
        });
        (name, location)
    }

    fn render_runtime_diagnostic(&self, diag: &Diagnostic, file: &str) -> String {
        let source = std::fs::read_to_string(file).ok();
        let stack_frames = self.frames[..=self.frame_index]
            .iter()
            .rev()
            .map(|frame| {
                let (name, location) = self.format_frame(frame);
                match location {
                    Some(loc) => format!("{name} ({loc})"),
                    None => name,
                }
            })
            .collect::<Vec<_>>();
        render_runtime_diagnostic(diag, file, source.as_deref(), &stack_frames)
    }

    pub(super) fn trace_instruction(&self, ip: usize, op: OpCode) {
        let instructions = self.current_frame().instructions();
        let widths = operand_widths(op);
        let mut operands = Vec::new();
        let mut offset = ip + 1;
        for width in widths {
            match width {
                1 => {
                    operands.push(read_u8(instructions, offset).to_string());
                    offset += 1;
                }
                2 => {
                    operands.push((read_u16(instructions, offset) as usize).to_string());
                    offset += 2;
                }
                4 => {
                    operands.push((read_u32(instructions, offset) as usize).to_string());
                    offset += 4;
                }
                _ => {}
            }
        }
        let operand_str = if operands.is_empty() {
            "".to_string()
        } else {
            format!(" {}", operands.join(" "))
        };
        println!("IP={:04} {}{}", ip, op, operand_str);
        self.trace_stack();
        self.trace_locals();
    }

    fn trace_stack(&self) {
        let items: Vec<String> = self.stack[..self.sp]
            .iter()
            .map(|obj| obj.to_string())
            .collect();
        println!("  stack: [{}]", items.join(", "));
    }

    fn trace_locals(&self) {
        let frame = self.current_frame();
        let bp = frame.base_pointer;
        let locals = frame.closure.function.num_locals;
        if locals == 0 {
            return;
        }
        let end = (bp + locals).min(self.stack.len());
        let items: Vec<String> = self.stack[bp..end]
            .iter()
            .map(|obj| obj.to_string())
            .collect();
        println!("  locals: [{}]", items.join(", "));
    }
}

pub(super) fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}'
            && let Some('[') = chars.peek().copied()
        {
            chars.next();
            for seq_ch in chars.by_ref() {
                if ('@'..='~').contains(&seq_ch) {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn split_hint(message: &str) -> (&str, Option<&str>) {
    if let Some(index) = message.find("\nHint:") {
        // Skip past the "Hint:" prefix since the diagnostic renderer adds its own
        let hint_start = index + "\nHint:".len();
        let hint_content = message[hint_start..].trim_start();
        if hint_content.is_empty() {
            (&message[..index], None)
        } else {
            (&message[..index], Some(hint_content))
        }
    } else {
        (message, None)
    }
}

fn split_first_line(message: &str) -> (&str, &str) {
    if let Some(index) = message.find('\n') {
        (&message[..index], &message[index + 1..])
    } else {
        (message, "")
    }
}
