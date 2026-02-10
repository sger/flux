use std::{collections::HashMap, rc::Rc};

use crate::{
    bytecode::{bytecode::Bytecode, op_code::OpCode},
    runtime::{
        closure::Closure, compiled_function::CompiledFunction, frame::Frame, leak_detector,
        value::Value,
    },
};

mod binary_ops;
mod comparison_ops;
mod dispatch;
mod function_call;
mod index_ops;
mod trace;

const STACK_SIZE: usize = 2048;
const GLOBALS_SIZE: usize = 65536;

pub struct VM {
    constants: Vec<Value>,
    stack: Vec<Value>,
    sp: usize,
    last_popped: Value,
    pub globals: Vec<Value>,
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
            stack: vec![Value::None; STACK_SIZE],
            sp: 0,
            last_popped: Value::None,
            globals: vec![Value::None; GLOBALS_SIZE],
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
            Err(err) => {
                let normalized = trace::strip_ansi(&err);
                // Check if error is already formatted (from runtime_error_enhanced / aggregator)
                // Formatted errors may start with "-- " or a file header "-->" and include an error code.
                let has_code = normalized.contains("[E") || normalized.contains("[e");
                let looks_formatted = has_code
                    && (normalized.starts_with("-- ")
                        || normalized.starts_with("--> ")
                        || normalized.contains("\n-- "));
                if looks_formatted {
                    Err(err)
                } else {
                    // Format unmigrated errors through Diagnostic system
                    Err(self.runtime_error_from_string(&err))
                }
            }
        }
    }

    fn run_inner(&mut self) -> Result<(), String> {
        while self.current_frame().ip < self.current_frame().instructions().len() {
            let ip = self.current_frame().ip;
            let op = OpCode::from(self.current_frame().instructions()[ip]);
            if self.trace {
                self.trace_instruction(ip, op);
            }

            let advance_ip = self.dispatch_instruction(ip, op)?;
            if advance_ip {
                self.current_frame_mut().ip += 1;
            }
        }
        Ok(())
    }

    fn build_array(&self, start: usize, end: usize) -> Value {
        let elements: Vec<Value> = self.stack[start..end].to_vec();
        leak_detector::record_array();
        Value::Array(elements.into())
    }

    fn build_hash(&self, start: usize, end: usize) -> Result<Value, String> {
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
        Ok(Value::Hash(hash.into()))
    }

    fn current_frame(&self) -> &Frame {
        &self.frames[self.frame_index]
    }

    fn current_frame_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.frame_index]
    }

    fn push(&mut self, obj: Value) -> Result<(), String> {
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

    fn pop(&mut self) -> Result<Value, String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        self.sp -= 1;
        let value = std::mem::replace(&mut self.stack[self.sp], Value::None);
        self.last_popped = value.clone();
        Ok(value)
    }

    pub fn last_popped_stack_elem(&self) -> &Value {
        &self.last_popped
    }
}

#[cfg(test)]
mod binary_ops_test;
#[cfg(test)]
mod comparison_ops_test;
#[cfg(test)]
mod dispatch_test;
#[cfg(test)]
mod function_call_test;
#[cfg(test)]
mod index_ops_test;
#[cfg(test)]
mod trace_test;
