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
                    let condition = self.pop()?;
                    if !condition.is_truthy() {
                        self.current_frame_mut().ip = pos - 1;
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
                OpCode::OpEqual | OpCode::OpNotEqual | OpCode::OpGreaterThan => {
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
                    _ => return Err(format!("unknown comparison: {:?}", opcode)),
                };
                self.push(Object::Boolean(result))
            }
            (Object::Float(l), Object::Float(r)) => {
                let result = match opcode {
                    OpCode::OpEqual => l == r,
                    OpCode::OpNotEqual => l != r,
                    OpCode::OpGreaterThan => l > r,
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
}
