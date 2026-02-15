use crate::{
    bytecode::op_code::OpCode,
    diagnostics::{
        DIVISION_BY_ZERO_RUNTIME, DiagnosticBuilder, DiagnosticsAggregator, HintChain,
        INVALID_OPERATION, invalid_operation,
        position::{Position, Span},
    },
    runtime::value::Value,
};

use super::VM;

impl VM {
    pub(super) fn execute_binary_operation(&mut self, op: OpCode) -> Result<(), String> {
        let (left, right) = self.pop_pair_untracked()?;

        match (&left, &right) {
            (Value::Integer(l), Value::Integer(r)) => {
                if *r == 0 && (op == OpCode::OpDiv || op == OpCode::OpMod) {
                    return Err(self.runtime_error_enhanced(&DIVISION_BY_ZERO_RUNTIME, &[]));
                }
                let result = match op {
                    OpCode::OpAdd => l + r,
                    OpCode::OpSub => l - r,
                    OpCode::OpMul => l * r,
                    OpCode::OpDiv => l / r,
                    OpCode::OpMod => l % r,
                    _ => return Err(format!("unknown integer operator: {:?}", op)),
                };
                self.push(Value::Integer(result))
            }
            (Value::Float(l), Value::Float(r)) => {
                let result = match op {
                    OpCode::OpAdd => l + r,
                    OpCode::OpSub => l - r,
                    OpCode::OpMul => l * r,
                    OpCode::OpDiv => l / r,
                    OpCode::OpMod => l % r,
                    _ => return Err(format!("unknown float operator: {:?}", op)),
                };
                self.push(Value::Float(result))
            }
            (Value::Integer(l), Value::Float(r)) => {
                let l = *l as f64;
                let result = match op {
                    OpCode::OpAdd => l + r,
                    OpCode::OpSub => l - r,
                    OpCode::OpMul => l * r,
                    OpCode::OpDiv => l / r,
                    OpCode::OpMod => l % r,
                    _ => return Err(format!("unknown float operator: {:?}", op)),
                };
                self.push(Value::Float(result))
            }
            (Value::Float(l), Value::Integer(r)) => {
                let r = *r as f64;
                let result = match op {
                    OpCode::OpAdd => l + r,
                    OpCode::OpSub => l - r,
                    OpCode::OpMul => l * r,
                    OpCode::OpDiv => l / r,
                    OpCode::OpMod => l % r,
                    _ => return Err(format!("unknown float operator: {:?}", op)),
                };
                self.push(Value::Float(result))
            }
            (Value::String(l), Value::String(r)) if op == OpCode::OpAdd => {
                self.push(Value::String(format!("{}{}", l, r).into()))
            }
            _ => {
                Err(self.invalid_binary_operation_error(op, &left, &right))
            }
        }
    }

    #[inline]
    fn binary_op_name(op: OpCode) -> &'static str {
        match op {
            OpCode::OpAdd => "add",
            OpCode::OpSub => "subtract",
            OpCode::OpMul => "multiply",
            OpCode::OpDiv => "divide",
            OpCode::OpMod => "modulo",
            _ => "operate on",
        }
    }

    #[cold]
    #[inline(never)]
    fn invalid_binary_operation_error(&self, op: OpCode, left: &Value, right: &Value) -> String {
        let op_name = Self::binary_op_name(op);

        // Special handling for String + Int/Float with hint chains
        if op == OpCode::OpAdd
            && ((left.type_name() == "String" && matches!(right, Value::Integer(_) | Value::Float(_)))
                || (right.type_name() == "String"
                    && matches!(left, Value::Integer(_) | Value::Float(_))))
        {
            let (file, span) = self.current_location().unwrap_or_else(|| {
                (
                    String::from("<unknown>"),
                    Span::new(Position::default(), Position::default()),
                )
            });

            let chain = HintChain::from_steps(vec![
                "Convert the number to String using to_string()",
                "Or parse the String to Int/Float if it contains a number",
                "Or use string interpolation: \"text ${value}\"",
            ])
            .with_conclusion("Flux requires explicit type conversions for safety");

            let diag = invalid_operation(
                op_name,
                left.type_name(),
                right.type_name(),
                file.clone(),
                span,
            )
            .with_hint_chain(chain);

            let source = std::fs::read_to_string(&file).ok();
            let mut rendered = if let Some(src) = source.as_deref() {
                DiagnosticsAggregator::new(std::slice::from_ref(&diag))
                    .with_file_headers(false)
                    .with_source(file.clone(), src)
                    .report()
                    .rendered
            } else {
                DiagnosticsAggregator::new(std::slice::from_ref(&diag))
                    .with_file_headers(false)
                    .report()
                    .rendered
            };

            if !self.frames.is_empty() {
                if rendered.ends_with('\n') {
                    rendered.push('\n');
                } else {
                    rendered.push_str("\n\n");
                }
                rendered.push_str("Stack trace:");
                for frame in self.frames[..=self.frame_index].iter().rev() {
                    rendered.push_str("\n  at ");
                    let (name, location) = self.format_frame(frame);
                    rendered.push_str(&name);
                    if let Some(loc) = location {
                        rendered.push_str(&format!(" ({})", loc));
                    }
                }
            }

            return rendered;
        }

        self.runtime_error_enhanced(
            &INVALID_OPERATION,
            &[op_name, left.type_name(), right.type_name()],
        )
    }
}
