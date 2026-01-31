use std::{collections::HashMap, rc::Rc};

use crate::{
    bytecode::{
        bytecode::Bytecode,
        op_code::{OpCode, operand_widths, read_u8, read_u16},
    },
    runtime::{
        builtins::BUILTINS, closure::Closure, compiled_function::CompiledFunction, frame::Frame,
        hash_key::HashKey, leak_detector, object::Object,
    },
};

const STACK_SIZE: usize = 2048;
const GLOBALS_SIZE: usize = 65536;

pub struct VM {
    constants: Vec<Object>,
    stack: Vec<Object>,
    sp: usize,
    pub globals: Vec<Object>,
    frames: Vec<Frame>,
    frame_index: usize,
    trace: bool,
}

impl VM {
    pub fn new(bytecode: Bytecode) -> Self {
        let main_fn = CompiledFunction::new(bytecode.instructions, 0, 0, bytecode.debug_info);
        let main_closure = Closure::new(Rc::new(main_fn), vec![]);
        let main_frame = Frame::new(Rc::new(main_closure), 0);

        Self {
            constants: bytecode.constants,
            stack: vec![Object::None; STACK_SIZE],
            sp: 0,
            globals: vec![Object::None; GLOBALS_SIZE],
            frames: vec![main_frame],
            frame_index: 0,
            trace: false,
        }
    }

    pub fn set_trace(&mut self, enabled: bool) {
        self.trace = enabled;
    }

    pub fn run(&mut self) -> Result<(), String> {
        match self.run_inner() {
            Ok(()) => Ok(()),
            Err(err) => Err(self.format_runtime_error(&err)),
        }
    }

    fn run_inner(&mut self) -> Result<(), String> {
        while self.current_frame().ip < self.current_frame().instructions().len() {
            let ip = self.current_frame().ip;
            let op = OpCode::from(self.current_frame().instructions()[ip]);
            if self.trace {
                self.trace_instruction(ip, op);
            }

            match op {
                OpCode::OpCurrentClosure => {
                    let closure = self.current_frame().closure.clone();
                    self.push(Object::Closure(closure))?;
                }
                OpCode::OpReturnValue => {
                    let return_value = self.pop()?;
                    let bp = self.pop_frame().base_pointer;
                    self.sp = bp - 1;
                    self.push(return_value)?;
                }
                OpCode::OpReturn => {
                    let bp = self.pop_frame().base_pointer;
                    self.sp = bp - 1;
                    self.push(Object::None)?;
                }
                OpCode::OpGetLocal => {
                    let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 1;
                    let bp = self.current_frame().base_pointer;
                    self.push(self.stack[bp + idx].clone())?;
                }
                OpCode::OpSetLocal => {
                    let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 1;
                    let bp = self.current_frame().base_pointer;
                    self.stack[bp + idx] = self.pop()?;
                }
                OpCode::OpGetFree => {
                    let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 1;
                    let value = self.current_frame().closure.free[idx].clone();
                    self.push(value)?;
                }
                OpCode::OpClosure => {
                    let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    let num_free = read_u8(self.current_frame().instructions(), ip + 3) as usize;
                    self.current_frame_mut().ip += 3;
                    self.push_closure(idx, num_free)?;
                }
                OpCode::OpJump => {
                    let pos = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip = pos - 1;
                }
                OpCode::OpJumpNotTruthy => {
                    let pos = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 2;
                    // Peek instead of pop - value stays on stack for short-circuit operators
                    let condition = self.stack[self.sp - 1].clone();
                    if !condition.is_truthy() {
                        self.current_frame_mut().ip = pos - 1;
                    } else {
                        // Only pop if we're NOT jumping (for && operator)
                        self.sp -= 1;
                    }
                }
                OpCode::OpJumpTruthy => {
                    let pos = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 2;
                    // Peek instead of pop - value stays on stack for short-circuit operators
                    let condition = self.stack[self.sp - 1].clone();
                    if condition.is_truthy() {
                        self.current_frame_mut().ip = pos - 1;
                    } else {
                        // Only pop if we're NOT jumping (for || operator)
                        self.sp -= 1;
                    }
                }
                OpCode::OpGetGlobal => {
                    let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 2;
                    self.push(self.globals[idx].clone())?;
                }
                OpCode::OpSetGlobal => {
                    let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 2;
                    self.globals[idx] = self.pop()?;
                }
                OpCode::OpConstant => {
                    let idx = read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 2;
                    self.push(self.constants[idx].clone())?;
                }
                OpCode::OpAdd | OpCode::OpSub | OpCode::OpMul | OpCode::OpDiv => {
                    self.execute_binary_operation(op)?;
                }
                OpCode::OpMod => {
                    self.execute_binary_operation(op)?;
                }
                OpCode::OpEqual | OpCode::OpNotEqual | OpCode::OpGreaterThan => {
                    self.execute_comparison(op)?;
                }
                OpCode::OpLessThanOrEqual | OpCode::OpGreaterThanOrEqual => {
                    self.execute_comparison(op)?;
                }
                OpCode::OpBang => {
                    let operand = self.pop()?;
                    self.push(Object::Boolean(!operand.is_truthy()))?;
                }
                OpCode::OpMinus => {
                    let operand = self.pop()?;
                    match operand {
                        Object::Integer(val) => self.push(Object::Integer(-val))?,
                        Object::Float(val) => self.push(Object::Float(-val))?,
                        _ => {
                            return Err(format!(
                                "unsupported type for negation: {}",
                                operand.type_name()
                            ));
                        }
                    }
                }
                OpCode::OpTrue => self.push(Object::Boolean(true))?,
                OpCode::OpFalse => self.push(Object::Boolean(false))?,
                OpCode::OpNull => self.push(Object::None)?,
                OpCode::OpIsSome => {
                    let value = self.pop()?;
                    self.push(Object::Boolean(matches!(value, Object::Some(_))))?;
                }
                OpCode::OpUnwrapSome => {
                    let value = self.pop()?;
                    match value {
                        Object::Some(inner) => self.push(*inner)?,
                        _ => {
                            return Err(format!(
                                "expected Some(..) but found {}",
                                value.type_name()
                            ));
                        }
                    }
                }
                OpCode::OpGetBuiltin => {
                    let idx = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 1;
                    let builtin = BUILTINS[idx].clone();
                    self.push(Object::Builtin(builtin))?;
                }
                OpCode::OpCall => {
                    let num_args = read_u8(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 1;
                    self.execute_call(num_args)?;
                    continue;
                }
                OpCode::OpPop => {
                    self.pop()?;
                }
                OpCode::OpArray => {
                    let num_elements =
                        read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 2;
                    let array = self.build_array(self.sp - num_elements, self.sp);
                    self.sp -= num_elements;
                    self.push(array)?;
                }
                OpCode::OpHash => {
                    let num_elements =
                        read_u16(self.current_frame().instructions(), ip + 1) as usize;
                    self.current_frame_mut().ip += 2;
                    let hash = self.build_hash(self.sp - num_elements, self.sp)?;
                    self.sp -= num_elements;
                    self.push(hash)?;
                }
                OpCode::OpIndex => {
                    let index = self.pop()?;
                    let left = self.pop()?;
                    self.execute_index_expression(left, index)?;
                }
                OpCode::OpNone => self.push(Object::None)?,
                OpCode::OpSome => {
                    let value = self.pop()?;
                    leak_detector::record_some();
                    self.push(Object::Some(Box::new(value)))?;
                }
                // Either type operations
                OpCode::OpLeft => {
                    let value = self.pop()?;
                    self.push(Object::Left(Box::new(value)))?;
                }
                OpCode::OpRight => {
                    let value = self.pop()?;
                    self.push(Object::Right(Box::new(value)))?;
                }
                OpCode::OpIsLeft => {
                    let value = self.pop()?;
                    let result = matches!(value, Object::Left(_));
                    self.push(Object::Boolean(result))?;
                }
                OpCode::OpIsRight => {
                    let value = self.pop()?;
                    let result = matches!(value, Object::Right(_));
                    self.push(Object::Boolean(result))?;
                }
                OpCode::OpUnwrapLeft => {
                    let value = self.pop()?;
                    match value {
                        Object::Left(inner) => self.push(*inner)?,
                        _ => return Err(self.format_runtime_error("Cannot unwrap non-Left value")),
                    }
                }
                OpCode::OpUnwrapRight => {
                    let value = self.pop()?;
                    match value {
                        Object::Right(inner) => self.push(*inner)?,
                        _ => return Err(self.format_runtime_error("Cannot unwrap non-Right value")),
                    }
                }
                OpCode::OpToString => {
                    let value = self.pop()?;
                    self.push(Object::String(value.to_string_value()))?;
                }
            }
            self.current_frame_mut().ip += 1;
        }
        Ok(())
    }

    fn format_runtime_error(&self, message: &str) -> String {
        let mut out = String::new();
        out.push_str(message);

        if self.frames.is_empty() {
            return out;
        }

        out.push_str("\nStack trace:");
        for frame in self.frames[..=self.frame_index].iter().rev() {
            out.push_str("\n  at ");
            let (name, location) = self.format_frame(frame);
            out.push_str(&name);
            if let Some(loc) = location {
                out.push_str(" (");
                out.push_str(&loc);
                out.push(')');
            }
        }
        out
    }

    fn format_frame(&self, frame: &Frame) -> (String, Option<String>) {
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
                        loc.span.start.column
                    )
                })
            })
        });
        (name, location)
    }

    fn trace_instruction(&self, ip: usize, op: OpCode) {
        let instructions = self.current_frame().instructions();
        let widths = operand_widths(op);
        let mut operands = Vec::new();
        let mut offset = ip + 1;
        for width in widths {
            match width {
                1 => {
                    operands.push(read_u8(instructions, offset) as usize);
                    offset += 1;
                }
                2 => {
                    operands.push(read_u16(instructions, offset) as usize);
                    offset += 2;
                }
                _ => {}
            }
        }
        let operand_str = if operands.is_empty() {
            "".to_string()
        } else {
            format!(
                " {}",
                operands
                    .iter()
                    .map(|o| o.to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
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

    fn build_array(&self, start: usize, end: usize) -> Object {
        let elements: Vec<Object> = self.stack[start..end].to_vec();
        leak_detector::record_array();
        Object::Array(elements)
    }

    fn build_hash(&self, start: usize, end: usize) -> Result<Object, String> {
        let mut hash = HashMap::new();
        let mut i = start;
        while i < end {
            let key = &self.stack[i];
            let value = &self.stack[i + 1];

            let hash_key = key
                .to_hash_key()
                .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;

            hash.insert(hash_key, value.clone());
            i += 2;
        }
        leak_detector::record_hash();
        Ok(Object::Hash(hash))
    }

    fn execute_index_expression(&mut self, left: Object, index: Object) -> Result<(), String> {
        match (&left, &index) {
            (Object::Array(elements), Object::Integer(idx)) => {
                self.execute_array_index(elements, *idx)
            }
            (Object::Hash(hash), _) => self.execute_hash_index(hash, &index),
            _ => Err(format!(
                "index operator not supported: {}",
                left.type_name()
            )),
        }
    }

    fn execute_array_index(&mut self, elements: &[Object], index: i64) -> Result<(), String> {
        if index < 0 || index as usize >= elements.len() {
            self.push(Object::None)
        } else {
            self.push(Object::Some(Box::new(elements[index as usize].clone())))
        }
    }

    fn execute_hash_index(
        &mut self,
        hash: &HashMap<HashKey, Object>,
        key: &Object,
    ) -> Result<(), String> {
        let hash_key = key
            .to_hash_key()
            .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;

        match hash.get(&hash_key) {
            Some(value) => self.push(Object::Some(Box::new(value.clone()))),
            None => self.push(Object::None),
        }
    }

    fn execute_call(&mut self, num_args: usize) -> Result<(), String> {
        let callee = self.stack[self.sp - 1 - num_args].clone();
        match callee {
            Object::Closure(closure) => self.call_closure(closure, num_args),
            Object::Builtin(builtin) => {
                let args: Vec<Object> = self.stack[self.sp - num_args..self.sp].to_vec();
                self.sp -= num_args + 1;
                let result = (builtin.func)(args)?;
                self.push(result)?;
                // Advance past the OpCall operand since builtins don't push a new frame.
                self.current_frame_mut().ip += 1;
                Ok(())
            }
            _ => Err(format!("calling non-function: {}", callee.type_name())),
        }
    }

    fn execute_binary_operation(&mut self, op: OpCode) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;

        match (&left, &right) {
            (Object::Integer(l), Object::Integer(r)) => {
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
            _ => Err(format!(
                "unsupported types: {} and {}",
                left.type_name(),
                right.type_name()
            )),
        }
    }

    fn execute_comparison(&mut self, opcode: OpCode) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;

        match (&left, &right) {
            (Object::Integer(l), Object::Integer(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Float(l), Object::Float(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Integer(l), Object::Float(r)) => {
                let l = *l as f64;
                let result = match opcode {
                    OpCode::OpEqual => l == *r,
                    OpCode::OpNotEqual => l != *r,
                    OpCode::OpGreaterThan => l > *r,
                    OpCode::OpLessThanOrEqual => l <= *r,
                    OpCode::OpGreaterThanOrEqual => l >= *r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Float(l), Object::Integer(r)) => {
                let r = *r as f64;
                let result = match opcode {
                    OpCode::OpEqual => *l == r,
                    OpCode::OpNotEqual => *l != r,
                    OpCode::OpGreaterThan => *l > r,
                    OpCode::OpLessThanOrEqual => *l <= r,
                    OpCode::OpGreaterThanOrEqual => *l >= r,
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Boolean(l), Object::Boolean(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("unknown boolean comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::String(l), Object::String(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
                    OpCode::OpLessThanOrEqual => l <= r,
                    OpCode::OpGreaterThanOrEqual => l >= r,
                    _ => return Err(format!("unknown string comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::None, Object::None) => {
                let result = match opcode {
                    OpCode::OpEqual => true,
                    OpCode::OpNotEqual => false,
                    _ => return Err(format!("cannot compare None with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::None, _) | (_, Object::None) => {
                let result = match opcode {
                    OpCode::OpEqual => false,
                    OpCode::OpNotEqual => true,
                    _ => return Err(format!("cannot compare None with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Some comparison
            (Object::Some(l), Object::Some(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Some with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Left comparison
            (Object::Left(l), Object::Left(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Left with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Right comparison
            (Object::Right(l), Object::Right(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    _ => return Err(format!("cannot compare Right with {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            // Left vs Right (always not equal)
            (Object::Left(_), Object::Right(_)) | (Object::Right(_), Object::Left(_)) => {
                let result = match opcode {
                    OpCode::OpEqual => false,
                    OpCode::OpNotEqual => true,
                    _ => return Err(format!("cannot compare Left with Right using {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            _ => Err(format!(
                "unsupported comparison: {} and {}",
                left.type_name(),
                right.type_name()
            )),
        }
    }

    fn call_closure(&mut self, closure: Rc<Closure>, num_args: usize) -> Result<(), String> {
        if num_args != closure.function.num_parameters {
            return Err(format!(
                "wrong number of arguments: want={}, got={}",
                closure.function.num_parameters, num_args
            ));
        }
        let frame = Frame::new(closure, self.sp - num_args);
        let num_locals = frame.closure.function.num_locals;
        self.push_frame(frame);
        self.sp += num_locals;
        Ok(())
    }

    fn push_closure(&mut self, const_index: usize, num_free: usize) -> Result<(), String> {
        match &self.constants[const_index] {
            Object::Function(func) => {
                let mut free = Vec::with_capacity(num_free);
                for i in 0..num_free {
                    free.push(self.stack[self.sp - num_free + i].clone());
                }
                self.sp -= num_free;
                let closure = Closure::new(func.clone(), free);
                self.push(Object::Closure(Rc::new(closure)))
            }
            _ => Err("not a function".to_string()),
        }
    }

    fn current_frame(&self) -> &Frame {
        &self.frames[self.frame_index]
    }

    fn current_frame_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.frame_index]
    }

    fn push(&mut self, obj: Object) -> Result<(), String> {
        if self.sp >= STACK_SIZE {
            return Err("stack overflow".to_string());
        }

        self.stack[self.sp] = obj;
        self.sp += 1;
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame) {
        self.frame_index += 1;
        if self.frame_index >= self.frames.len() {
            self.frames.push(frame);
        } else {
            self.frames[self.frame_index] = frame;
        }
    }

    fn pop_frame(&mut self) -> Frame {
        let frame = self.frames[self.frame_index].clone();
        self.frame_index -= 1;
        frame
    }

    fn pop(&mut self) -> Result<Object, String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        self.sp -= 1;
        Ok(self.stack[self.sp].clone())
    }

    pub fn last_popped_stack_elem(&self) -> &Object {
        &self.stack[self.sp]
    }
}

fn render_display_path(file: &str) -> String {
    let path = std::path::Path::new(file);
    if path.is_absolute()
        && let Ok(cwd) = std::env::current_dir()
        && let Ok(stripped) = path.strip_prefix(&cwd)
    {
        return stripped.to_string_lossy().to_string();
    }
    file.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::compiler::Compiler;
    use crate::frontend::diagnostic::render_diagnostics;
    use crate::frontend::lexer::Lexer;
    use crate::frontend::parser::Parser;

    fn run(input: &str) -> Object {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let mut compiler = Compiler::new();
        compiler
            .compile(&program)
            .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
        let mut vm = VM::new(compiler.bytecode());
        vm.run().unwrap();
        vm.last_popped_stack_elem().clone()
    }

    #[test]
    fn test_integer_arithmetic() {
        assert_eq!(run("1 + 2;"), Object::Integer(3));
        assert_eq!(run("5 * 2 + 10;"), Object::Integer(20));
        assert_eq!(run("-5;"), Object::Integer(-5));
    }

    #[test]
    fn test_float_arithmetic() {
        assert_eq!(run("1.5 + 2.25;"), Object::Float(3.75));
        assert_eq!(run("2.0 * 3.5;"), Object::Float(7.0));
        assert_eq!(run("-0.5;"), Object::Float(-0.5));
        assert_eq!(run("1 + 2.5;"), Object::Float(3.5));
        assert_eq!(run("2.5 + 1;"), Object::Float(3.5));
    }

    #[test]
    fn test_boolean_expressions() {
        assert_eq!(run("true;"), Object::Boolean(true));
        assert_eq!(run("1 < 2;"), Object::Boolean(true));
        assert_eq!(run("!true;"), Object::Boolean(false));
    }

    #[test]
    fn test_conditionals() {
        assert_eq!(run("if true { 10; };"), Object::Integer(10));
        assert_eq!(run("if false { 10; } else { 20; };"), Object::Integer(20));
    }

    #[test]
    fn test_global_variables() {
        assert_eq!(run("let x = 5; x;"), Object::Integer(5));
        assert_eq!(run("let x = 5; let y = x; y;"), Object::Integer(5));
    }

    #[test]
    fn test_functions() {
        assert_eq!(run("let f = fun() { 5 + 10; }; f();"), Object::Integer(15));
        assert_eq!(
            run("let sum = fun(a, b) { a + b; }; sum(1, 2);"),
            Object::Integer(3)
        );
    }

    #[test]
    fn test_closures() {
        let input = r#"
            let newClosure = fun(a) { fun() { a; }; };
            let closure = newClosure(99);
            closure();
        "#;
        assert_eq!(run(input), Object::Integer(99));
    }

    #[test]
    fn test_recursive_fibonacci() {
        let input = r#"
            let fib = fun(n) {
                if n < 2 { return n; };
                fib(n - 1) + fib(n - 2);
            };
            fib(10);
        "#;
        assert_eq!(run(input), Object::Integer(55));
    }

    #[test]
    fn test_array_literals() {
        assert_eq!(
            run("[1, 2, 3];"),
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(2),
                Object::Integer(3),
            ])
        );
        assert_eq!(run("[];"), Object::Array(vec![]));
    }

    #[test]
    fn test_array_index() {
        assert_eq!(
            run("[1, 2, 3][0];"),
            Object::Some(Box::new(Object::Integer(1)))
        );
        assert_eq!(
            run("[1, 2, 3][1];"),
            Object::Some(Box::new(Object::Integer(2)))
        );
        assert_eq!(
            run("[1, 2, 3][2];"),
            Object::Some(Box::new(Object::Integer(3)))
        );
        assert_eq!(run("[1, 2, 3][3];"), Object::None);
        assert_eq!(run("[1, 2, 3][-1];"), Object::None);
    }

    #[test]
    fn test_hash_literals() {
        let result = run(r#"{"a": 1};"#);
        match result {
            Object::Hash(h) => {
                assert_eq!(h.len(), 1);
            }
            _ => panic!("expected hash"),
        }
    }

    #[test]
    fn test_hash_index() {
        assert_eq!(
            run(r#"{"a": 1}["a"];"#),
            Object::Some(Box::new(Object::Integer(1)))
        );
        assert_eq!(run(r#"{"a": 1}["b"];"#), Object::None);
        assert_eq!(
            run(r#"{1: "one"}[1];"#),
            Object::Some(Box::new(Object::String("one".to_string())))
        );
    }

    #[test]
    fn test_builtin_len() {
        assert_eq!(run(r#"len("hello");"#), Object::Integer(5));
        assert_eq!(run("len([1, 2, 3]);"), Object::Integer(3));
    }

    #[test]
    fn test_builtin_array_functions() {
        assert_eq!(run("first([1, 2, 3]);"), Object::Integer(1));
        assert_eq!(run("last([1, 2, 3]);"), Object::Integer(3));
        assert_eq!(
            run("rest([1, 2, 3]);"),
            Object::Array(vec![Object::Integer(2), Object::Integer(3),])
        );
        assert_eq!(
            run("push([1, 2], 3);"),
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(2),
                Object::Integer(3),
            ])
        );
    }

    #[test]
    fn test_less_than_or_equal_operator() {
        assert_eq!(run("5 <= 10;"), Object::Boolean(true));
        assert_eq!(run("10 <= 5;"), Object::Boolean(false));
        assert_eq!(run("5 <= 5;"), Object::Boolean(true));
        assert_eq!(run("5.5 <= 10.5;"), Object::Boolean(true));
        assert_eq!(run("10.5 <= 5.5;"), Object::Boolean(false));
        assert_eq!(run("5.5 <= 5.5;"), Object::Boolean(true));
        assert_eq!(run(r#""apple" <= "banana";"#), Object::Boolean(true));
        assert_eq!(run(r#""banana" <= "apple";"#), Object::Boolean(false));
        assert_eq!(run(r#""apple" <= "apple";"#), Object::Boolean(true));
    }

    #[test]
    fn test_greater_than_or_equal_operator() {
        assert_eq!(run("10 >= 5;"), Object::Boolean(true));
        assert_eq!(run("5 >= 10;"), Object::Boolean(false));
        assert_eq!(run("5 >= 5;"), Object::Boolean(true));
        assert_eq!(run("10.5 >= 5.5;"), Object::Boolean(true));
        assert_eq!(run("5.5 >= 10.5;"), Object::Boolean(false));
        assert_eq!(run("5.5 >= 5.5;"), Object::Boolean(true));
        assert_eq!(run(r#""banana" >= "apple";"#), Object::Boolean(true));
        assert_eq!(run(r#""apple" >= "banana";"#), Object::Boolean(false));
        assert_eq!(run(r#""apple" >= "apple";"#), Object::Boolean(true));
    }

    #[test]
    fn test_modulo_operator() {
        // Integer modulo
        assert_eq!(run("10 % 3;"), Object::Integer(1));
        assert_eq!(run("7 % 2;"), Object::Integer(1)); // odd check
        assert_eq!(run("8 % 2;"), Object::Integer(0)); // even check
        assert_eq!(run("15 % 4;"), Object::Integer(3));
        assert_eq!(run("100 % 7;"), Object::Integer(2));
        assert_eq!(run("5 % 5;"), Object::Integer(0));

        // Float modulo
        assert_eq!(run("10.5 % 3.0;"), Object::Float(1.5));
        assert_eq!(run("7.5 % 2.0;"), Object::Float(1.5));
        assert_eq!(run("10.0 % 3.0;"), Object::Float(1.0));
        assert_eq!(run("5.5 % 2.5;"), Object::Float(0.5));

        // Mixed integer-float modulo
        assert_eq!(run("10 % 3.0;"), Object::Float(1.0));
        assert_eq!(run("7 % 2.5;"), Object::Float(2.0));

        // Mixed float-integer modulo
        assert_eq!(run("10.5 % 3;"), Object::Float(1.5));
        assert_eq!(run("7.5 % 2;"), Object::Float(1.5));

        // Edge cases
        assert_eq!(run("1 % 10;"), Object::Integer(1)); // smaller % larger
        assert_eq!(run("0 % 5;"), Object::Integer(0)); // zero % n
    }

    #[test]
    fn test_pipe_operator() {
        // Basic pipe: value |> function
        assert_eq!(
            run("let double = fun(x) { x * 2; }; 5 |> double;"),
            Object::Integer(10)
        );

        // Chained pipes: value |> f |> g
        assert_eq!(
            run(
                "let double = fun(x) { x * 2; }; let triple = fun(x) { x * 3; }; 5 |> double |> triple;"
            ),
            Object::Integer(30)
        );

        // Pipe with additional arguments: value |> function(arg)
        assert_eq!(
            run("let add = fun(x, y) { x + y; }; 5 |> add(3);"),
            Object::Integer(8)
        );

        // Pipe with multiple additional arguments
        assert_eq!(
            run("let sum3 = fun(a, b, c) { a + b + c; }; 1 |> sum3(2, 3);"),
            Object::Integer(6)
        );

        // Complex chain with mixed calls
        assert_eq!(
            run(r#"
                let double = fun(x) { x * 2; };
                let add = fun(x, y) { x + y; };
                let square = fun(x) { x * x; };
                2 |> double |> add(10) |> square;
            "#),
            Object::Integer(196) // ((2*2) + 10)^2 = 14^2 = 196
        );

        // Pipe preserves argument order (left side becomes first arg)
        assert_eq!(
            run("let subtract = fun(a, b) { a - b; }; 10 |> subtract(3);"),
            Object::Integer(7) // 10 - 3 = 7
        );

        // Pipe with string operations
        assert_eq!(
            run(r#"
                let greet = fun(name) { "Hello, " + name; };
                let exclaim = fun(s) { s + "!"; };
                "World" |> greet |> exclaim;
            "#),
            Object::String("Hello, World!".to_string())
        );

        // Pipe with array operations
        assert_eq!(
            run("let getFirst = fun(arr) { first(arr); }; [1, 2, 3] |> getFirst;"),
            Object::Integer(1)
        );

        // Nested pipe expressions
        assert_eq!(
            run(r#"
                let inc = fun(x) { x + 1; };
                let double = fun(x) { x * 2; };
                (3 |> inc) |> double;
            "#),
            Object::Integer(8) // (3+1) * 2 = 8
        );
    }

    #[test]
    fn test_either_left_right() {
        // Basic Left creation
        assert_eq!(
            run("Left(42);"),
            Object::Left(Box::new(Object::Integer(42)))
        );

        // Basic Right creation
        assert_eq!(
            run("Right(42);"),
            Object::Right(Box::new(Object::Integer(42)))
        );

        // Left with string
        assert_eq!(
            run(r#"Left("error");"#),
            Object::Left(Box::new(Object::String("error".to_string())))
        );

        // Right with string
        assert_eq!(
            run(r#"Right("success");"#),
            Object::Right(Box::new(Object::String("success".to_string())))
        );

        // Nested Left
        assert_eq!(
            run("Left(Left(1));"),
            Object::Left(Box::new(Object::Left(Box::new(Object::Integer(1)))))
        );

        // Nested Right
        assert_eq!(
            run("Right(Right(1));"),
            Object::Right(Box::new(Object::Right(Box::new(Object::Integer(1)))))
        );

        // Left containing Right
        assert_eq!(
            run("Left(Right(42));"),
            Object::Left(Box::new(Object::Right(Box::new(Object::Integer(42)))))
        );

        // Right containing Left
        assert_eq!(
            run("Right(Left(42));"),
            Object::Right(Box::new(Object::Left(Box::new(Object::Integer(42)))))
        );
    }

    #[test]
    fn test_either_pattern_matching() {
        // Simple Left match with wildcard
        assert_eq!(
            run(r#"
                let x = Left(1);
                match x {
                    Left(_) -> true;
                    _ -> false;
                };
            "#),
            Object::Boolean(true)
        );

        // Simple Right match with wildcard
        assert_eq!(
            run(r#"
                let x = Right(1);
                match x {
                    Right(_) -> true;
                    _ -> false;
                };
            "#),
            Object::Boolean(true)
        );

        // Left doesn't match Right pattern
        assert_eq!(
            run(r#"
                let x = Left(1);
                match x {
                    Right(_) -> true;
                    _ -> false;
                };
            "#),
            Object::Boolean(false)
        );

        // Right doesn't match Left pattern
        assert_eq!(
            run(r#"
                let x = Right(1);
                match x {
                    Left(_) -> true;
                    _ -> false;
                };
            "#),
            Object::Boolean(false)
        );

        // Match on Left with binding
        assert_eq!(
            run(r#"
                let x = Left(42);
                match x {
                    Left(v) -> v;
                    _ -> 0;
                };
            "#),
            Object::Integer(42)
        );

        // Match on Right with binding
        assert_eq!(
            run(r#"
                let x = Right(42);
                match x {
                    Right(v) -> v;
                    _ -> 0;
                };
            "#),
            Object::Integer(42)
        );
    }

    #[test]
    fn test_either_in_functions() {
        // Function returning Left
        assert_eq!(
            run(r#"
                fun fail(msg) { Left(msg) }
                fail("oops");
            "#),
            Object::Left(Box::new(Object::String("oops".to_string())))
        );

        // Function returning Right
        assert_eq!(
            run(r#"
                fun succeed(val) { Right(val) }
                succeed(100);
            "#),
            Object::Right(Box::new(Object::Integer(100)))
        );

        // Safe divide function
        assert_eq!(
            run(r#"
                fun safeDivide(a, b) {
                    if b == 0 {
                        Left("division by zero")
                    } else {
                        Right(a / b)
                    }
                }
                safeDivide(10, 2);
            "#),
            Object::Right(Box::new(Object::Integer(5)))
        );

        assert_eq!(
            run(r#"
                fun safeDivide(a, b) {
                    if b == 0 {
                        Left("division by zero")
                    } else {
                        Right(a / b)
                    }
                }
                safeDivide(10, 0);
            "#),
            Object::Left(Box::new(Object::String("division by zero".to_string())))
        );
    }

    #[test]
    fn test_either_equality() {
        // Left equality
        assert_eq!(run("Left(1) == Left(1);"), Object::Boolean(true));
        assert_eq!(run("Left(1) == Left(2);"), Object::Boolean(false));
        assert_eq!(run("Left(1) != Left(2);"), Object::Boolean(true));

        // Right equality
        assert_eq!(run("Right(1) == Right(1);"), Object::Boolean(true));
        assert_eq!(run("Right(1) == Right(2);"), Object::Boolean(false));
        assert_eq!(run("Right(1) != Right(2);"), Object::Boolean(true));

        // Left vs Right
        assert_eq!(run("Left(1) == Right(1);"), Object::Boolean(false));
        assert_eq!(run("Left(1) != Right(1);"), Object::Boolean(true));
    }

    #[test]
    fn test_either_with_option() {
        // Left containing Some
        assert_eq!(
            run("Left(Some(42));"),
            Object::Left(Box::new(Object::Some(Box::new(Object::Integer(42)))))
        );

        // Right containing None
        assert_eq!(
            run("Right(None);"),
            Object::Right(Box::new(Object::None))
        );

        // Some containing Left
        assert_eq!(
            run("Some(Left(1));"),
            Object::Some(Box::new(Object::Left(Box::new(Object::Integer(1)))))
        );

        // Some containing Right
        assert_eq!(
            run("Some(Right(1));"),
            Object::Some(Box::new(Object::Right(Box::new(Object::Integer(1)))))
        );
    }

    #[test]
    fn test_either_in_arrays() {
        // Array of Either values
        assert_eq!(
            run("[Left(1), Right(2), Left(3)];"),
            Object::Array(vec![
                Object::Left(Box::new(Object::Integer(1))),
                Object::Right(Box::new(Object::Integer(2))),
                Object::Left(Box::new(Object::Integer(3))),
            ])
        );
    }

    #[test]
    fn test_either_in_hash() {
        // Hash with Either values
        assert_eq!(
            run(r#"let h = {"ok": Right(1), "err": Left("fail")}; h["ok"];"#),
            Object::Some(Box::new(Object::Right(Box::new(Object::Integer(1)))))
        );

        assert_eq!(
            run(r#"let h = {"ok": Right(1), "err": Left("fail")}; h["err"];"#),
            Object::Some(Box::new(Object::Left(Box::new(Object::String("fail".to_string())))))
        );
    }
}
