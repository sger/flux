use std::rc::Rc;

use crate::{
    bytecode::{bytecode::Bytecode, op_code::OpCode},
    runtime::{
        closure::Closure,
        compiled_function::CompiledFunction,
        frame::Frame,
        gc::{
            GcHandle, GcHeap, HeapObject,
            hamt::{hamt_empty, hamt_insert},
        },
        leak_detector,
        value::Value,
    },
};

mod binary_ops;
mod comparison_ops;
mod dispatch;
mod function_call;
mod index_ops;
mod trace;

const INITIAL_STACK_SIZE: usize = 2048;
const MAX_STACK_SIZE: usize = 1 << 20; // 1,048,576 slots
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
    pub gc_heap: GcHeap,
}

impl VM {
    pub fn new(bytecode: Bytecode) -> Self {
        let main_fn = CompiledFunction::new(bytecode.instructions, 0, 0, bytecode.debug_info);
        let main_closure = Closure::new(Rc::new(main_fn), vec![]);
        let main_frame = Frame::new(Rc::new(main_closure), 0);

        Self {
            constants: bytecode.constants,
            stack: vec![Value::None; INITIAL_STACK_SIZE],
            sp: 0,
            last_popped: Value::None,
            globals: vec![Value::None; GLOBALS_SIZE],
            frames: vec![main_frame],
            frame_index: 0,
            trace: false,
            gc_heap: GcHeap::new(),
        }
    }

    pub fn set_trace(&mut self, enabled: bool) {
        self.trace = enabled;
    }

    pub fn set_gc_enabled(&mut self, enabled: bool) {
        self.gc_heap.set_enabled(enabled);
    }

    pub fn set_gc_threshold(&mut self, threshold: usize) {
        self.gc_heap.set_threshold(threshold);
    }

    /// Returns the GC telemetry report, if compiled with the `gc-telemetry` feature.
    #[cfg(feature = "gc-telemetry")]
    pub fn gc_telemetry_report(&self) -> String {
        self.gc_heap.telemetry_report()
    }

    /// Allocates a heap object, triggering GC if the threshold is reached.
    pub(crate) fn gc_alloc(&mut self, object: HeapObject) -> GcHandle {
        if self.gc_heap.should_collect() {
            self.collect_gc();
        }
        self.gc_heap.alloc(object)
    }

    fn collect_gc(&mut self) {
        self.gc_heap.collect(
            &self.stack,
            self.sp,
            &self.globals,
            &self.constants,
            &self.last_popped,
            &self.frames,
            self.frame_index,
        );
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
            self.execute_current_instruction(None)?;
        }
        Ok(())
    }

    fn execute_current_instruction(
        &mut self,
        invoke_target_frame: Option<usize>,
    ) -> Result<(), String> {
        let ip = self.current_frame().ip;
        let op = OpCode::from(self.current_frame().instructions()[ip]);
        if self.trace {
            self.trace_instruction(ip, op);
        }

        let frame_before = self.frame_index;
        let advance_ip = self.dispatch_instruction(ip, op)?;
        if advance_ip {
            match invoke_target_frame {
                None => {
                    self.current_frame_mut().ip += 1;
                }
                Some(target) => {
                    if self.frame_index > frame_before {
                        // New frame was pushed; advance caller frame IP.
                        self.frames[frame_before].ip += 1;
                    } else if self.frame_index == frame_before {
                        self.current_frame_mut().ip += 1;
                    } else if self.frame_index >= target {
                        // Deeper frame returned; advance resumed frame IP.
                        self.current_frame_mut().ip += 1;
                    }
                    // If frame_index < target, target frame returned; do not advance caller IP.
                }
            }
        }
        Ok(())
    }

    fn build_array(&mut self, start: usize, end: usize) -> Value {
        // Move values out of stack to avoid Rc refcount overhead
        let mut elements = Vec::with_capacity(end - start);
        for i in start..end {
            elements.push(std::mem::replace(&mut self.stack[i], Value::None));
        }
        leak_detector::record_array();
        Value::Array(Rc::new(elements))
    }

    fn build_hash(&mut self, start: usize, end: usize) -> Result<Value, String> {
        let mut root = hamt_empty(&mut self.gc_heap);
        let mut i = start;
        while i < end {
            let key = &self.stack[i];
            let value = &self.stack[i + 1];

            let hash_key = key
                .to_hash_key()
                .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;

            root = hamt_insert(&mut self.gc_heap, root, hash_key, value.clone());
            i += 2;
        }
        leak_detector::record_hash();
        Ok(Value::Gc(root))
    }

    fn current_frame(&self) -> &Frame {
        &self.frames[self.frame_index]
    }

    fn current_frame_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.frame_index]
    }

    fn ensure_stack_capacity(&mut self, needed_top: usize) -> Result<(), String> {
        if needed_top <= self.stack.len() {
            return Ok(());
        }
        if needed_top > MAX_STACK_SIZE {
            return Err("stack overflow".to_string());
        }

        let mut new_len = self.stack.len().max(1);
        while new_len < needed_top {
            new_len = (new_len.saturating_mul(2)).min(MAX_STACK_SIZE);
            if new_len == MAX_STACK_SIZE {
                break;
            }
        }
        if new_len < needed_top {
            return Err("stack overflow".to_string());
        }

        self.stack.resize(new_len, Value::None);
        Ok(())
    }

    fn push(&mut self, obj: Value) -> Result<(), String> {
        self.ensure_stack_capacity(self.sp + 1)?;
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
        // Store in last_popped before moving out. For Rc types this is just a refcount bump.
        self.last_popped = std::mem::replace(&mut self.stack[self.sp], Value::None);
        Ok(self.last_popped.clone())
    }

    /// Returns the last popped value from the stack.
    ///
    /// After a program completes execution, this returns the final result.
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
