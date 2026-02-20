use std::rc::Rc;

use crate::diagnostics::NOT_A_FUNCTION;
use crate::runtime::RuntimeContext;
use crate::runtime::builtins::get_builtin_by_index;
use crate::runtime::gc::GcHeap;
use crate::runtime::{closure::Closure, frame::Frame, value::Value};

use super::VM;

impl VM {
    fn unwind_invoke_error(&mut self, start_sp: usize, start_frame_index: usize) {
        while self.frame_index > start_frame_index {
            let bp = self.pop_frame_bp();
            let _ = self.reset_sp(bp.saturating_sub(1));
        }
        let _ = self.reset_sp(start_sp);
    }

    #[inline]
    fn builtin_fixed_arity(name: &str) -> Option<usize> {
        match name {
            "len" | "first" | "last" | "rest" | "to_string" | "reverse" | "trim" | "upper"
            | "lower" | "chars" | "keys" | "values" | "abs" | "type_of" | "is_int" | "is_float"
            | "is_string" | "is_bool" | "is_array" | "is_hash" | "is_none" | "is_some" | "hd"
            | "tl" | "is_list" | "to_list" | "to_array" | "is_map" | "read_file" | "read_lines"
            | "parse_int" | "now_ms" | "time" | "sum" | "product" | "parse_ints" => Some(1),
            "contains" | "slice" | "split" | "join" | "starts_with" | "ends_with" | "has_key"
            | "merge" | "delete" | "min" | "max" | "map" | "filter" | "put" | "get" | "range"
            | "split_ints" | "assert_eq" | "assert_neq" => Some(2),
            "replace" | "substring" | "fold" => Some(3),
            "read_stdin" => Some(0),
            "assert_true" | "assert_false" => Some(1),
            // Variadic / optional arity builtins remain on generic path.
            "print" | "sort" | "concat" | "list" => None,
            _ => None,
        }
    }

    pub(super) fn execute_call(&mut self, num_args: usize) -> Result<(), String> {
        let callee_idx = self.sp - 1 - num_args;
        match &self.stack[callee_idx] {
            Value::Closure(closure) => self.call_closure(closure.clone(), num_args),
            Value::Builtin(builtin_idx) => {
                let builtin = get_builtin_by_index(*builtin_idx as usize)
                    .ok_or_else(|| format!("invalid builtin index {}", builtin_idx))?;
                let callee_idx = self.sp - 1 - num_args;
                self.stack[callee_idx] = Value::Uninit;
                let fixed_arity = Self::builtin_fixed_arity(builtin.name);

                let args = if fixed_arity == Some(num_args) {
                    match num_args {
                        0 => Vec::new(),
                        1 => {
                            let a0 = std::mem::replace(&mut self.stack[self.sp - 1], Value::Uninit);
                            vec![a0]
                        }
                        2 => {
                            let a0 = std::mem::replace(&mut self.stack[self.sp - 2], Value::Uninit);
                            let a1 = std::mem::replace(&mut self.stack[self.sp - 1], Value::Uninit);
                            vec![a0, a1]
                        }
                        3 => {
                            let a0 = std::mem::replace(&mut self.stack[self.sp - 3], Value::Uninit);
                            let a1 = std::mem::replace(&mut self.stack[self.sp - 2], Value::Uninit);
                            let a2 = std::mem::replace(&mut self.stack[self.sp - 1], Value::Uninit);
                            vec![a0, a1, a2]
                        }
                        _ => {
                            let mut args = Vec::with_capacity(num_args);
                            for i in self.sp - num_args..self.sp {
                                args.push(std::mem::replace(&mut self.stack[i], Value::Uninit));
                            }
                            args
                        }
                    }
                } else {
                    // Keep generic path to preserve existing builtin-level arity errors.
                    let mut args = Vec::with_capacity(num_args);
                    for i in self.sp - num_args..self.sp {
                        args.push(std::mem::replace(&mut self.stack[i], Value::Uninit));
                    }
                    args
                };

                self.reset_sp(callee_idx)?;
                let result = (builtin.func)(self, args)?;
                self.push(result)?;
                Ok(())
            }
            other => Err(self.runtime_error_enhanced(&NOT_A_FUNCTION, &[other.type_name()])),
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
        let max_stack = frame.closure.function.max_stack;
        self.push_frame(frame);
        self.ensure_stack_capacity_with_headroom(
            self.sp + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.sp += num_locals;
        Ok(())
    }

    pub(super) fn execute_tail_call(&mut self, num_args: usize) -> Result<(), String> {
        let callee_idx = self.sp - 1 - num_args;
        match &self.stack[callee_idx] {
            Value::Closure(closure) => self.tail_call_closure(closure.clone(), num_args),
            Value::Builtin(_) => {
                // Builtins don't push frames, so treat as normal call
                self.execute_call(num_args)
            }
            other => Err(self.runtime_error_enhanced(&NOT_A_FUNCTION, &[other.type_name()])),
        }
    }

    fn tail_call_closure(&mut self, closure: Rc<Closure>, num_args: usize) -> Result<(), String> {
        if num_args != closure.function.num_parameters {
            return Err(format!(
                "wrong number of arguments: want={}, got={}",
                closure.function.num_parameters, num_args
            ));
        }

        let base_pointer = self.current_frame().base_pointer;

        // CRITICAL: Pre-copy arguments to handle cases like f(x, x) where
        // multiple arguments reference the same local
        self.tail_arg_scratch.clear();
        self.tail_arg_scratch.reserve(num_args);
        for i in 0..num_args {
            self.tail_arg_scratch
                .push(self.stack[self.sp - num_args + i].clone());
        }

        // Overwrite old locals with new arguments
        for (i, arg) in self.tail_arg_scratch.drain(..).enumerate() {
            self.stack[base_pointer + i] = arg;
        }

        // Reset stack pointer and instruction pointer
        let max_stack = closure.function.max_stack;
        self.ensure_stack_capacity_with_headroom(
            base_pointer + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.reset_sp(base_pointer + closure.function.num_locals)?;
        self.current_frame_mut().ip = 0;
        self.current_frame_mut().closure = closure;

        Ok(())
    }

    pub(super) fn push_closure(
        &mut self,
        const_index: usize,
        num_free: usize,
    ) -> Result<(), String> {
        match &self.constants[const_index] {
            Value::Function(func) => {
                let func = func.clone();
                let mut free = Vec::with_capacity(num_free);
                for i in 0..num_free {
                    free.push(self.stack[self.sp - num_free + i].clone());
                }
                self.reset_sp(self.sp - num_free)?;
                let closure = Closure::new(func, free);
                self.push(Value::Closure(Rc::new(closure)))
            }
            _ => Err("not a function".to_string()),
        }
    }

    /// Invokes a callable Value (closure or builtin) with the given arguments
    /// and returns the result synchronously.
    ///
    /// Used by higher-order builtins (map, filter, fold) to call user-provided
    /// functions from within the builtin implementation.
    pub fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        match callee {
            Value::Builtin(builtin_idx) => {
                let builtin = get_builtin_by_index(builtin_idx as usize)
                    .ok_or_else(|| format!("invalid builtin index {}", builtin_idx))?;
                (builtin.func)(self, args)
            }
            Value::Closure(closure) => {
                let start_sp = self.sp;
                let start_frame_index = self.frame_index;
                let num_args = args.len();
                if num_args != closure.function.num_parameters {
                    return Err(format!(
                        "wrong number of arguments: want={}, got={}",
                        closure.function.num_parameters, num_args
                    ));
                }

                // Push the closure onto the stack (callee slot)
                self.push(Value::Closure(closure.clone()))?;

                // Push arguments onto the stack
                for arg in args {
                    self.push(arg)?;
                }

                // Push a new frame
                let frame = Frame::new(closure, self.sp - num_args);
                let num_locals = frame.closure.function.num_locals;
                let max_stack = frame.closure.function.max_stack;
                self.push_frame(frame);
                self.ensure_stack_capacity_with_headroom(
                    self.sp + max_stack,
                    super::STACK_PREGROW_HEADROOM,
                )?;
                self.sp += num_locals;

                // Track frame index so we know when the closure returns
                let target_frame_index = self.frame_index;

                // Run the dispatch loop until this frame returns
                while self.frame_index >= target_frame_index {
                    if self.frame_index == target_frame_index
                        && self.current_frame().ip >= self.current_frame().instructions().len()
                    {
                        self.unwind_invoke_error(start_sp, start_frame_index);
                        return Err("callable exited without return".to_string());
                    }
                    if let Err(err) = self.execute_current_instruction(Some(target_frame_index)) {
                        self.unwind_invoke_error(start_sp, start_frame_index);
                        return Err(err);
                    }
                }

                // The return value is on the stack (pushed by OpReturnValue/OpReturn)
                self.pop()
            }
            _ => Err(format!("not callable: {}", callee.type_name())),
        }
    }

    #[inline]
    fn invoke_closure_arity1(&mut self, closure: Rc<Closure>, arg: Value) -> Result<Value, String> {
        if closure.function.num_parameters != 1 {
            return Err(format!(
                "wrong number of arguments: want={}, got=1",
                closure.function.num_parameters
            ));
        }

        self.push(Value::Closure(closure.clone()))?;
        self.push(arg)?;

        let frame = Frame::new(closure, self.sp - 1);
        let num_locals = frame.closure.function.num_locals;
        let max_stack = frame.closure.function.max_stack;
        self.push_frame(frame);
        self.ensure_stack_capacity_with_headroom(
            self.sp + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.sp += num_locals;

        let target_frame_index = self.frame_index;
        while self.frame_index >= target_frame_index {
            if self.frame_index == target_frame_index
                && self.current_frame().ip >= self.current_frame().instructions().len()
            {
                return Err("callable exited without return".to_string());
            }
            self.execute_current_instruction(Some(target_frame_index))?;
        }

        self.pop()
    }

    #[inline]
    fn invoke_closure_arity2(
        &mut self,
        closure: Rc<Closure>,
        left: Value,
        right: Value,
    ) -> Result<Value, String> {
        if closure.function.num_parameters != 2 {
            return Err(format!(
                "wrong number of arguments: want={}, got=2",
                closure.function.num_parameters
            ));
        }

        self.push(Value::Closure(closure.clone()))?;
        self.push(left)?;
        self.push(right)?;

        let frame = Frame::new(closure, self.sp - 2);
        let num_locals = frame.closure.function.num_locals;
        let max_stack = frame.closure.function.max_stack;
        self.push_frame(frame);
        self.ensure_stack_capacity_with_headroom(
            self.sp + max_stack,
            super::STACK_PREGROW_HEADROOM,
        )?;
        self.sp += num_locals;

        let target_frame_index = self.frame_index;
        while self.frame_index >= target_frame_index {
            if self.frame_index == target_frame_index
                && self.current_frame().ip >= self.current_frame().instructions().len()
            {
                return Err("callable exited without return".to_string());
            }
            self.execute_current_instruction(Some(target_frame_index))?;
        }

        self.pop()
    }
}

impl RuntimeContext for VM {
    fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        VM::invoke_value(self, callee, args)
    }

    #[inline]
    fn invoke_unary_value(&mut self, callee: &Value, arg: Value) -> Result<Value, String> {
        match callee {
            Value::Builtin(builtin_idx) => {
                let builtin = get_builtin_by_index(*builtin_idx as usize)
                    .ok_or_else(|| format!("invalid builtin index {}", builtin_idx))?;
                (builtin.func)(self, vec![arg])
            }
            Value::Closure(closure) => self.invoke_closure_arity1(closure.clone(), arg),
            other => Err(format!("not callable: {}", other.type_name())),
        }
    }

    #[inline]
    fn invoke_binary_value(
        &mut self,
        callee: &Value,
        left: Value,
        right: Value,
    ) -> Result<Value, String> {
        match callee {
            Value::Builtin(builtin_idx) => {
                let builtin = get_builtin_by_index(*builtin_idx as usize)
                    .ok_or_else(|| format!("invalid builtin index {}", builtin_idx))?;
                (builtin.func)(self, vec![left, right])
            }
            Value::Closure(closure) => self.invoke_closure_arity2(closure.clone(), left, right),
            other => Err(format!("not callable: {}", other.type_name())),
        }
    }

    fn gc_heap(&self) -> &GcHeap {
        &self.gc_heap
    }

    fn gc_heap_mut(&mut self) -> &mut GcHeap {
        &mut self.gc_heap
    }
}
