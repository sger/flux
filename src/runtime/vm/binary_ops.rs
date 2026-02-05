use crate::{
    bytecode::op_code::OpCode,
    frontend::{
        diagnostics::{
            DIVISION_BY_ZERO_RUNTIME, DiagnosticBuilder, DiagnosticsAggregator, HintChain,
            INVALID_OPERATION, runtime_errors::invalid_operation,
        },
        position::{Position, Span},
    },
    runtime::object::Object,
};

use super::VM;

impl VM {
    pub(super) fn execute_binary_operation(&mut self, op: OpCode) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;

        match (&left, &right) {
            (Object::Integer(l), Object::Integer(r)) => {
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
                self.push(Object::Integer(result))
            }
            (Object::Float(l), Object::Float(r)) => {
                let result = match op {
                    OpCode::OpAdd => l + r,
                    OpCode::OpSub => l - r,
                    OpCode::OpMul => l * r,
                    OpCode::OpDiv => l / r,
                    OpCode::OpMod => l % r,
                    _ => return Err(format!("unknown float operator: {:?}", op)),
                };
                self.push(Object::Float(result))
            }
            (Object::Integer(l), Object::Float(r)) => {
                let l = *l as f64;
                let result = match op {
                    OpCode::OpAdd => l + r,
                    OpCode::OpSub => l - r,
                    OpCode::OpMul => l * r,
                    OpCode::OpDiv => l / r,
                    OpCode::OpMod => l % r,
                    _ => return Err(format!("unknown float operator: {:?}", op)),
                };
                self.push(Object::Float(result))
            }
            (Object::Float(l), Object::Integer(r)) => {
                let r = *r as f64;
                let result = match op {
                    OpCode::OpAdd => l + r,
                    OpCode::OpSub => l - r,
                    OpCode::OpMul => l * r,
                    OpCode::OpDiv => l / r,
                    OpCode::OpMod => l % r,
                    _ => return Err(format!("unknown float operator: {:?}", op)),
                };
                self.push(Object::Float(result))
            }
            (Object::String(l), Object::String(r)) if op == OpCode::OpAdd => {
                self.push(Object::String(format!("{}{}", l, r)))
            }
            _ => {
                let op_name = match op {
                    OpCode::OpAdd => "add",
                    OpCode::OpSub => "subtract",
                    OpCode::OpMul => "multiply",
                    OpCode::OpDiv => "divide",
                    OpCode::OpMod => "modulo",
                    _ => "operate on",
                };

                // Special handling for String + Int/Float with hint chains
                if op == OpCode::OpAdd
                    && ((left.type_name() == "String"
                        && matches!(right, Object::Integer(_) | Object::Float(_)))
                        || (right.type_name() == "String"
                            && matches!(left, Object::Integer(_) | Object::Float(_))))
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
                            .with_source(file.clone(), src)
                            .report()
                            .rendered
                    } else {
                        DiagnosticsAggregator::new(std::slice::from_ref(&diag))
                            .report()
                            .rendered
                    };

                    // Add stack trace
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

                    return Err(rendered);
                }

                Err(self.runtime_error_enhanced(
                    &INVALID_OPERATION,
                    &[op_name, left.type_name(), right.type_name()],
                ))
            }
        }
    }
}
