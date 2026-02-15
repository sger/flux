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
const STACK_PREGROW_HEADROOM: usize = 256;
const STACK_GROW_MIN_CHUNK: usize = 4096;

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
    tail_arg_scratch: Vec<Value>,
}

impl VM {
    pub fn new(bytecode: Bytecode) -> Self {
        let main_fn = CompiledFunction::new(bytecode.instructions, 0, 0, bytecode.debug_info);
        let main_closure = Closure::new(Rc::new(main_fn), vec![]);
        let main_frame = Frame::new(Rc::new(main_closure), 0);

        Self {
            constants: bytecode.constants,
            stack: vec![Value::Uninit; INITIAL_STACK_SIZE],
            sp: 0,
            last_popped: Value::None,
            globals: vec![Value::None; GLOBALS_SIZE],
            frames: vec![main_frame],
            frame_index: 0,
            trace: false,
            gc_heap: GcHeap::new(),
            tail_arg_scratch: Vec::new(),
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
        let mut closure = self.frames[self.frame_index].closure.clone();
        let mut instructions: &[u8] = &closure.function.instructions;

        loop {
            let ip = self.frames[self.frame_index].ip;
            if ip >= instructions.len() {
                break;
            }

            let op = OpCode::from(instructions[ip]);
            if self.trace {
                self.trace_instruction(ip, op);
            }

            let frame_before = self.frame_index;
            let ip_delta = self.dispatch_instruction(instructions, ip, op)?;
            self.apply_ip_delta(frame_before, ip_delta, None);

            if self.frame_index != frame_before || matches!(op, OpCode::OpTailCall) {
                closure = self.frames[self.frame_index].closure.clone();
                instructions = &closure.function.instructions;
            }
        }
        Ok(())
    }

    fn execute_current_instruction(
        &mut self,
        invoke_target_frame: Option<usize>,
    ) -> Result<(), String> {
        let frame_index = self.frame_index;
        let ip = self.frames[frame_index].ip;
        let closure = self.frames[frame_index].closure.clone();
        let instructions: &[u8] = &closure.function.instructions;
        let op = OpCode::from(instructions[ip]);
        if self.trace {
            self.trace_instruction(ip, op);
        }

        let frame_before = self.frame_index;
        let ip_delta = self.dispatch_instruction(instructions, ip, op)?;
        self.apply_ip_delta(frame_before, ip_delta, invoke_target_frame);
        Ok(())
    }

    #[inline(always)]
    fn apply_ip_delta(
        &mut self,
        frame_before: usize,
        ip_delta: usize,
        invoke_target_frame: Option<usize>,
    ) {
        if ip_delta == 0 {
            return;
        }

        match invoke_target_frame {
            None => {
                if self.frame_index > frame_before {
                    // New frame was pushed; advance caller frame IP.
                    self.frames[frame_before].ip += ip_delta;
                } else {
                    self.frames[self.frame_index].ip += ip_delta;
                }
            }
            Some(target) => {
                if self.frame_index > frame_before {
                    // New frame was pushed; advance caller frame IP.
                    self.frames[frame_before].ip += ip_delta;
                } else if self.frame_index == frame_before {
                    self.frames[self.frame_index].ip += ip_delta;
                } else if self.frame_index >= target {
                    // Deeper frame returned; advance resumed frame IP.
                    self.frames[self.frame_index].ip += ip_delta;
                }
                // If frame_index < target, target frame returned; do not advance caller IP.
            }
        }
    }

    fn build_array(&mut self, start: usize, end: usize) -> Value {
        // Move values out of stack to avoid Rc refcount overhead
        let mut elements = Vec::with_capacity(end - start);
        for i in start..end {
            elements.push(std::mem::replace(&mut self.stack[i], Value::Uninit));
        }
        leak_detector::record_array();
        Value::Array(Rc::new(elements))
    }

    fn build_hash(&mut self, start: usize, end: usize) -> Result<Value, String> {
        let mut root = hamt_empty(&mut self.gc_heap);
        let mut i = start;
        while i < end {
            let key = std::mem::replace(&mut self.stack[i], Value::Uninit);
            let value = std::mem::replace(&mut self.stack[i + 1], Value::Uninit);

            let hash_key = key
                .to_hash_key()
                .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;

            root = hamt_insert(&mut self.gc_heap, root, hash_key, value);
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
        self.ensure_stack_capacity_with_headroom(needed_top, 0)
    }

    fn ensure_stack_capacity_with_headroom(
        &mut self,
        needed_top: usize,
        extra_headroom: usize,
    ) -> Result<(), String> {
        if needed_top <= self.stack.len() {
            return Ok(());
        }
        if needed_top > MAX_STACK_SIZE {
            return Err("stack overflow".to_string());
        }

        let target_top = needed_top.saturating_add(extra_headroom).min(MAX_STACK_SIZE);
        let mut new_len = self.stack.len().max(1);
        while new_len < target_top {
            let grow_15 = new_len + (new_len / 2);
            let grow_chunk = new_len.saturating_add(STACK_GROW_MIN_CHUNK);
            new_len = grow_15.max(grow_chunk).min(MAX_STACK_SIZE);
        }
        if new_len < needed_top {
            return Err("stack overflow".to_string());
        }

        self.stack.resize(new_len, Value::Uninit);
        Ok(())
    }

    #[inline(always)]
    fn clear_stack_range(&mut self, new_sp: usize, old_sp: usize) {
        debug_assert!(new_sp <= old_sp);
        debug_assert!(old_sp <= self.stack.len());
        for i in new_sp..old_sp {
            let _ = std::mem::replace(&mut self.stack[i], Value::Uninit);
        }
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
    }

    #[inline(always)]
    fn reset_sp(&mut self, new_sp: usize) -> Result<(), String> {
        if new_sp > MAX_STACK_SIZE {
            return Err("stack overflow".to_string());
        }
        if new_sp > self.stack.len() {
            self.ensure_stack_capacity(new_sp)?;
        }
        let old_sp = self.sp;
        if new_sp < old_sp {
            self.clear_stack_range(new_sp, old_sp);
        }
        self.sp = new_sp;
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(())
    }

    #[cfg(debug_assertions)]
    #[inline(always)]
    fn debug_assert_stack_invariant(&self) {
        // Dead stack slots may contain stale values until they are reused.
        // In debug mode, only enforce structural bounds to keep checks O(1).
        debug_assert!(self.sp <= self.stack.len());
    }

    #[inline(always)]
    fn push(&mut self, obj: Value) -> Result<(), String> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.sp <= self.stack.len());
        }
        if self.sp < self.stack.len() {
            self.stack[self.sp] = obj;
            self.sp += 1;
            #[cfg(debug_assertions)]
            self.debug_assert_stack_invariant();
            return Ok(());
        }
        self.push_slow(obj)
    }

    #[cold]
    #[inline(never)]
    fn push_slow(&mut self, obj: Value) -> Result<(), String> {
        self.ensure_stack_capacity(self.sp + 1)?;
        self.stack[self.sp] = obj;
        self.sp += 1;
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
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

    fn pop_frame_bp(&mut self) -> usize {
        let bp = self.frames[self.frame_index].base_pointer;
        self.frame_index -= 1;
        bp
    }

    #[inline(always)]
    fn pop(&mut self) -> Result<Value, String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        let new_sp = self.sp - 1;
        self.sp = new_sp;
        // Move out + overwrite without mem::replace drop glue on the hot pop path.
        let value = unsafe {
            let slot = self.stack.as_mut_ptr().add(new_sp);
            let out = std::ptr::read(slot);
            std::ptr::write(slot, Value::Uninit);
            out
        };
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(value)
    }

    #[inline(always)]
    fn pop_and_track(&mut self) -> Result<Value, String> {
        let value = self.pop()?;
        self.last_popped = value.clone();
        Ok(value)
    }

    #[inline(always)]
    fn peek(&self, back: usize) -> Result<&Value, String> {
        if back >= self.sp {
            return Err("stack underflow".to_string());
        }
        let idx = self.sp - 1 - back;
        let value = &self.stack[idx];
        if matches!(value, Value::Uninit) {
            return Err("read from uninitialized stack slot".to_string());
        }
        Ok(value)
    }

    fn pop_untracked(&mut self) -> Result<Value, String> {
        let value = self.pop()?;
        self.last_popped = Value::None;
        Ok(value)
    }

    #[inline(always)]
    fn discard_top(&mut self) -> Result<(), String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        self.sp -= 1;
        // Drop the value in-place without the clear_stack_range loop overhead.
        // SAFETY: sp was > 0, so self.sp is now a valid index holding a live Value.
        unsafe {
            let slot = self.stack.as_mut_ptr().add(self.sp);
            let _old = std::ptr::read(slot);
            std::ptr::write(slot, Value::Uninit);
        }
        self.last_popped = Value::None;
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(())
    }

    fn pop_pair_untracked(&mut self) -> Result<(Value, Value), String> {
        if self.sp < 2 {
            return Err("stack underflow".to_string());
        }
        let new_sp = self.sp - 2;
        self.sp = new_sp;
        // Move both values out in one pass and overwrite dead slots with Uninit.
        // SAFETY: old sp >= 2 guarantees both slots are initialized and in-bounds.
        let (left, right) = unsafe {
            let base = self.stack.as_mut_ptr().add(new_sp);
            let left = std::ptr::read(base);
            let right = std::ptr::read(base.add(1));
            std::ptr::write(base, Value::Uninit);
            std::ptr::write(base.add(1), Value::Uninit);
            (left, right)
        };
        self.last_popped = Value::None;
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok((left, right))
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
