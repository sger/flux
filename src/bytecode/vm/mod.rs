use std::rc::Rc;

use crate::{
    bytecode::{bytecode::Bytecode, op_code::OpCode},
    runtime::{
        closure::Closure, compiled_function::CompiledFunction, frame::Frame, hamt,
        handler_frame::HandlerFrame, leak_detector, value::Value,
    },
};

mod binary_ops;
mod comparison_ops;
mod dispatch;
mod function_call;
mod index_ops;
mod primop;
pub mod test_runner;
mod trace;

const INITIAL_STACK_SIZE: usize = 2048;
const MAX_STACK_SIZE: usize = 1 << 20; // 1,048,576 slots
const GLOBALS_SIZE: usize = 65536;
const STACK_PREGROW_HEADROOM: usize = 256;
const STACK_GROW_MIN_CHUNK: usize = 4096;

// ── Slot-type abstraction ─────────────────────────────────────────────────────
//
// `Slot` is the element type used in the VM's stack, globals, and constants.
//
// When `nan-boxing` is enabled every slot is a `NanBox` (8 bytes).
// When `nan-boxing` is disabled every slot is a `Value` (no overhead).
//
// All conversions between `Value` and `Slot` go through `slot::to_slot` /
// `slot::from_slot` / `slot::from_slot_ref`.  Callers that only need to
// read-then-own a slot should use `from_slot`; callers that need a clone
// without consuming the slot should use `from_slot_ref`.

mod slot {
    use crate::runtime::nanbox::NanBox;
    use crate::runtime::value::Value;

    pub type Slot = NanBox;

    #[inline(always)]
    pub fn uninit() -> Slot {
        NanBox::from_uninit()
    }

    #[inline(always)]
    pub fn to_slot(v: Value) -> Slot {
        NanBox::from_value(v)
    }

    #[inline(always)]
    pub fn from_slot(s: Slot) -> Value {
        s.to_value()
    }

    #[inline(always)]
    pub fn from_slot_ref(s: &Slot) -> Value {
        s.clone().to_value()
    }
}

use slot::Slot;

pub struct VM {
    constants: Vec<Slot>,
    stack: Vec<Slot>,
    sp: usize,
    last_popped: Slot,
    pub globals: Vec<Slot>,
    frames: Vec<Frame>,
    frame_index: usize,
    trace: bool,
    tail_arg_scratch: Vec<Slot>,
    /// Active effect handlers pushed by OpHandle / popped by OpEndHandle.
    pub(crate) handler_stack: Vec<HandlerFrame>,
}

impl VM {
    pub fn new(bytecode: Bytecode) -> Self {
        let main_fn = CompiledFunction::new(bytecode.instructions, 0, 0, bytecode.debug_info);
        let main_closure = Closure::new(Rc::new(main_fn), vec![]);
        let main_frame = Frame::new(Rc::new(main_closure), 0);

        Self {
            constants: bytecode.constants.into_iter().map(slot::to_slot).collect(),
            stack: vec![slot::uninit(); INITIAL_STACK_SIZE],
            sp: 0,
            last_popped: slot::to_slot(Value::None),
            globals: vec![slot::to_slot(Value::None); GLOBALS_SIZE],
            frames: vec![main_frame],
            frame_index: 0,
            trace: false,
            tail_arg_scratch: Vec::new(),
            handler_stack: Vec::new(),
        }
    }

    pub fn set_trace(&mut self, enabled: bool) {
        self.trace = enabled;
    }

    /// Create a closure that acts as the identity function: `fn(x) -> x`.
    /// Used as the `resume` parameter for tail-resumptive `OpPerformDirect`,
    /// so that `resume(v)` simply returns `v`.
    pub(crate) fn make_identity_closure(&self) -> Value {
        // Bytecode: OpReturnLocal(0) — return the first argument.
        let instructions = vec![OpCode::OpReturnLocal as u8, 0];
        let func = Rc::new(CompiledFunction::new(instructions, 1, 1, None));
        Value::Closure(Rc::new(Closure::new(func, vec![])))
    }

    pub fn run(&mut self) -> Result<(), String> {
        match self.run_inner() {
            Ok(()) => Ok(()),
            Err(err) => {
                let normalized = trace::strip_ansi(&err);
                // Check if error is already formatted (from runtime_error_enhanced / aggregator)
                // Formatted errors may start with a rendered severity header and include an error code.
                let has_code = normalized.contains("[E") || normalized.contains("[e");
                let looks_formatted = has_code
                    && (normalized.starts_with("Error[")
                        || normalized.starts_with("error[")
                        || normalized.starts_with("Warning[")
                        || normalized.starts_with("Note[")
                        || normalized.starts_with("Help[")
                        || normalized.contains("\nError[")
                        || normalized.contains("\nerror[")
                        || normalized.contains("\nWarning[")
                        || normalized.contains("\nNote[")
                        || normalized.contains("\nHelp["));
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
                if self.frame_index > 0 {
                    let fn_name = "<function>";
                    return Err(format!(
                        "VM bug: IP {} overran instruction boundary ({} bytes) in function '{}' at frame depth {}",
                        ip,
                        instructions.len(),
                        fn_name,
                        self.frame_index,
                    ));
                }
                break;
            }

            let op = OpCode::from(instructions[ip]);
            if self.trace {
                self.trace_instruction(ip, op);
            }

            let frame_before = self.frame_index;
            // Track closure identity so continuation resume which leaves
            // frame_index unchanged numerically but swaps in a different frame
            // triggers an instruction-pointer refresh.
            let closure_ptr_before = Rc::as_ptr(&closure);
            let ip_delta = self.dispatch_instruction(instructions, ip, op)?;
            self.apply_ip_delta(frame_before, ip_delta, None);

            let closure_changed =
                Rc::as_ptr(&self.frames[self.frame_index].closure) != closure_ptr_before;
            if self.frame_index != frame_before
                || matches!(op, OpCode::OpTailCall)
                || closure_changed
            {
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
            let s = std::mem::replace(&mut self.stack[i], slot::uninit());
            elements.push(slot::from_slot(s));
        }
        leak_detector::record_array();
        Value::Array(Rc::new(elements))
    }

    fn build_tuple(&mut self, start: usize, end: usize) -> Value {
        let mut elements = Vec::with_capacity(end - start);
        for i in start..end {
            let s = std::mem::replace(&mut self.stack[i], slot::uninit());
            elements.push(slot::from_slot(s));
        }
        leak_detector::record_tuple();
        Value::Tuple(Rc::new(elements))
    }

    fn build_hash(&mut self, start: usize, end: usize) -> Result<Value, String> {
        let mut root = hamt::hamt_empty();
        let mut i = start;
        while i < end {
            let key = slot::from_slot(std::mem::replace(&mut self.stack[i], slot::uninit()));
            let value = slot::from_slot(std::mem::replace(&mut self.stack[i + 1], slot::uninit()));

            let hash_key = key
                .to_hash_key()
                .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;

            root = hamt::hamt_insert(&root, hash_key, value);
            i += 2;
        }
        leak_detector::record_hash();
        Ok(Value::HashMap(root))
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

        let target_top = needed_top
            .saturating_add(extra_headroom)
            .min(MAX_STACK_SIZE);
        let mut new_len = self.stack.len().max(1);
        while new_len < target_top {
            let grow_15 = new_len + (new_len / 2);
            let grow_chunk = new_len.saturating_add(STACK_GROW_MIN_CHUNK);
            new_len = grow_15.max(grow_chunk).min(MAX_STACK_SIZE);
        }
        if new_len < needed_top {
            return Err("stack overflow".to_string());
        }

        self.stack.resize_with(new_len, slot::uninit);
        Ok(())
    }

    #[inline(always)]
    fn clear_stack_range(&mut self, new_sp: usize, old_sp: usize) {
        debug_assert!(new_sp <= old_sp);
        debug_assert!(old_sp <= self.stack.len());
        for i in new_sp..old_sp {
            let _ = std::mem::replace(&mut self.stack[i], slot::uninit());
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
            self.stack[self.sp] = slot::to_slot(obj);
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
        self.stack[self.sp] = slot::to_slot(obj);
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

    fn pop_frame_return_slot(&mut self) -> usize {
        let return_slot = self.frames[self.frame_index].return_slot;
        self.frame_index -= 1;
        return_slot
    }

    #[inline(always)]
    fn pop(&mut self) -> Result<Value, String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        let new_sp = self.sp - 1;
        self.sp = new_sp;
        // Move out + overwrite without drop glue on the hot pop path.
        let value = unsafe {
            let slot_ptr = self.stack.as_mut_ptr().add(new_sp);
            let out = std::ptr::read(slot_ptr);
            std::ptr::write(slot_ptr, slot::uninit());
            slot::from_slot(out)
        };
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok(value)
    }

    #[inline(always)]
    fn pop_and_track(&mut self) -> Result<Value, String> {
        let value = self.pop()?;
        self.last_popped = slot::to_slot(value.clone());
        Ok(value)
    }

    #[inline(always)]
    fn peek(&self, back: usize) -> Result<Value, String> {
        if back >= self.sp {
            return Err("stack underflow".to_string());
        }
        let idx = self.sp - 1 - back;
        let value = slot::from_slot_ref(&self.stack[idx]);
        if matches!(value, Value::Uninit) {
            return Err("read from uninitialized stack slot".to_string());
        }
        Ok(value)
    }

    fn pop_untracked(&mut self) -> Result<Value, String> {
        let value = self.pop()?;
        self.last_popped = slot::to_slot(Value::None);
        Ok(value)
    }

    #[inline(always)]
    fn discard_top(&mut self) -> Result<(), String> {
        if self.sp == 0 {
            return Err("stack underflow".to_string());
        }
        self.sp -= 1;
        // Drop the value in-place without the clear_stack_range loop overhead.
        // SAFETY: sp was > 0, so self.sp is now a valid index holding a live Slot.
        unsafe {
            let slot_ptr = self.stack.as_mut_ptr().add(self.sp);
            let _old = std::ptr::read(slot_ptr);
            std::ptr::write(slot_ptr, slot::uninit());
        }
        self.last_popped = slot::to_slot(Value::None);
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
        // Move both values out in one pass and overwrite dead slots with uninit.
        // SAFETY: old sp >= 2 guarantees both slots are initialized and in-bounds.
        let (left, right) = unsafe {
            let base = self.stack.as_mut_ptr().add(new_sp);
            let left = std::ptr::read(base);
            let right = std::ptr::read(base.add(1));
            std::ptr::write(base, slot::uninit());
            std::ptr::write(base.add(1), slot::uninit());
            (slot::from_slot(left), slot::from_slot(right))
        };
        self.last_popped = slot::to_slot(Value::None);
        #[cfg(debug_assertions)]
        self.debug_assert_stack_invariant();
        Ok((left, right))
    }

    // ── Stack/globals/constants accessor helpers ──────────────────────────────
    //
    // Use these in dispatch.rs and function_call.rs instead of direct indexing
    // to keep NanBox conversions in one place.

    /// Clone the Value at stack index `idx`.
    #[inline(always)]
    fn stack_get(&self, idx: usize) -> Value {
        slot::from_slot_ref(&self.stack[idx])
    }

    /// Store `v` at stack index `idx`.
    #[inline(always)]
    fn stack_set(&mut self, idx: usize, v: Value) {
        self.stack[idx] = slot::to_slot(v);
    }

    /// Take the Value at stack index `idx`, leaving `Uninit` in its place.
    #[inline(always)]
    fn stack_take(&mut self, idx: usize) -> Value {
        slot::from_slot(std::mem::replace(&mut self.stack[idx], slot::uninit()))
    }

    /// Take the raw `Slot` at stack index `idx`, leaving `Uninit` in its place.
    ///
    /// Unlike [`stack_take`], this does NOT decode the slot to a [`Value`].
    /// Use this in the NaN-boxing fast path to avoid an unnecessary decode/re-encode
    /// round-trip when passing slots directly to [`BaseFunction::call_owned_nanboxed`].
    #[inline(always)]
    fn stack_slot_take(&mut self, idx: usize) -> Slot {
        std::mem::replace(&mut self.stack[idx], slot::uninit())
    }

    /// Push a raw `Slot` onto the stack without encoding from [`Value`].
    ///
    /// Counterpart to [`stack_slot_take`]: used after [`BaseFunction::call_owned_nanboxed`]
    /// returns a `Slot` so the result is stored without a decode/re-encode round-trip.
    #[inline(always)]
    fn push_slot(&mut self, s: Slot) -> Result<(), String> {
        if self.sp < self.stack.len() {
            self.stack[self.sp] = s;
            self.sp += 1;
            return Ok(());
        }
        // Slow path: grow stack. Must convert to Value to reuse push_slow's growth logic.
        self.push_slow(slot::from_slot(s))
    }

    /// Clone the Value at constants index `idx`.
    #[inline(always)]
    fn const_get(&self, idx: usize) -> Value {
        slot::from_slot_ref(&self.constants[idx])
    }

    /// Clone the Value at globals index `idx`.
    #[inline(always)]
    fn global_get(&self, idx: usize) -> Value {
        slot::from_slot_ref(&self.globals[idx])
    }

    /// Store `v` at globals index `idx`.
    #[inline(always)]
    fn global_set(&mut self, idx: usize, v: Value) {
        self.globals[idx] = slot::to_slot(v);
    }

    /// Returns the last popped value from the stack.
    ///
    /// After a program completes execution, this returns the final result.
    pub fn last_popped_stack_elem(&self) -> Value {
        slot::from_slot_ref(&self.last_popped)
    }

    /// Swap the VM's globals with an external `Vec<Value>` buffer.
    ///
    /// Used by the REPL to persist globals across iterations without exposing
    /// the internal `Slot` type.
    pub fn swap_globals_values(&mut self, external: &mut [Value]) {
        // Convert VM slots -> Values into external, and external Values -> slots into VM.
        let vm_len = self.globals.len();
        let ext_len = external.len();
        // Ensure both have the same length (they should; both are GLOBALS_SIZE).
        debug_assert_eq!(vm_len, ext_len);

        // Swap element-by-element.
        for (g, e) in self.globals[..vm_len.min(ext_len)]
            .iter_mut()
            .zip(external.iter_mut())
        {
            let vm_val = slot::from_slot(std::mem::replace(g, slot::uninit()));
            let ext_val = std::mem::replace(e, Value::None);
            *g = slot::to_slot(ext_val);
            *e = vm_val;
        }
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
