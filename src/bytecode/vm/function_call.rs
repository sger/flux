use std::rc::Rc;

use crate::diagnostics::NOT_A_FUNCTION;
use crate::runtime::RuntimeContext;
use crate::runtime::value::format_value;
use crate::runtime::{closure::Closure, frame::Frame, value::Value};

use super::VM;

// OpPerform instruction size: opcode (1) + const_idx (1) + arity (1) = 3 bytes.
// This constant is used during continuation resume to advance the captured frame's IP past OpPerform.
// We don't need it here since the IP is already advanced during capture, but kept for documentation.
const _OP_PERFORM_SIZE: usize = 3;

impl VM {
    #[inline]
    fn check_closure_contract_stack_args(
        &self,
        closure: &Closure,
        num_args: usize,
    ) -> Result<(), String> {
        let Some(contract) = closure.function.contract.as_ref() else {
            return Ok(());
        };
        let args_start = self.sp - num_args;
        for (index, maybe_expected) in contract.params.iter().enumerate() {
            let Some(expected) = maybe_expected.as_ref() else {
                continue;
            };
            if index >= num_args {
                break;
            }
            let actual = self.stack_get(args_start + index);
            if !expected.matches_value(&actual, self) {
                let expected_name = expected.type_name();
                let actual_type = actual.type_name();
                let actual_value = format_value(&actual);
                return Err(self.runtime_type_error_enhanced(
                    &expected_name,
                    actual_type,
                    Some(&actual_value),
                ));
            }
        }
        Ok(())
    }

    #[inline]
    fn check_closure_contract_value_args(
        &self,
        closure: &Closure,
        args: &[Value],
    ) -> Result<(), String> {
        let Some(contract) = closure.function.contract.as_ref() else {
            return Ok(());
        };
        for (index, maybe_expected) in contract.params.iter().enumerate() {
            let Some(expected) = maybe_expected.as_ref() else {
                continue;
            };
            let Some(actual) = args.get(index) else {
                break;
            };
            if !expected.matches_value(actual, self) {
                let expected_name = expected.type_name();
                let actual_type = actual.type_name();
                let actual_value = format_value(actual);
                return Err(self.runtime_type_error_enhanced(
                    &expected_name,
                    actual_type,
                    Some(&actual_value),
                ));
            }
        }
        Ok(())
    }

    fn unwind_invoke_error(&mut self, start_sp: usize, start_frame_index: usize) {
        while self.frame_index > start_frame_index {
            let return_slot = self.pop_frame_return_slot();
            let _ = self.reset_sp(return_slot);
        }
        let _ = self.reset_sp(start_sp);
    }

    pub(super) fn execute_call(&mut self, num_args: usize) -> Result<(), String> {
        let callee_idx = self.sp - 1 - num_args;

        match self.stack_get(callee_idx) {
            Value::Closure(closure) => self.call_closure(closure, num_args),
            Value::BaseFunction(_) => {
                Err("BaseFunction values are deprecated; base functions are now compiled from lib/Flow/".to_string())
            }
            other => Err(self.runtime_error_enhanced(&NOT_A_FUNCTION, &[other.type_name()])),
        }
    }

    pub(super) fn execute_call_self(&mut self, num_args: usize) -> Result<(), String> {
        let closure = self.current_frame().closure.clone();
        let args_start = self.sp - num_args;
        self.call_closure_with_return_slot(closure, num_args, args_start, args_start)
    }

    fn call_closure(&mut self, closure: Rc<Closure>, num_args: usize) -> Result<(), String> {
        let args_start = self.sp - num_args;
        self.call_closure_at_args_start(closure, num_args, args_start)
    }

    fn call_closure_at_args_start(
        &mut self,
        closure: Rc<Closure>,
        num_args: usize,
        args_start: usize,
    ) -> Result<(), String> {
        self.call_closure_with_return_slot(closure, num_args, args_start, args_start - 1)
    }

    fn call_closure_with_return_slot(
        &mut self,
        closure: Rc<Closure>,
        num_args: usize,
        args_start: usize,
        return_slot: usize,
    ) -> Result<(), String> {
        if num_args != closure.function.num_parameters {
            return Err(format!(
                "wrong number of arguments: want={}, got={}",
                closure.function.num_parameters, num_args
            ));
        }
        self.check_closure_contract_stack_args(&closure, num_args)?;
        let frame = Frame::new_with_return_slot(closure, args_start, return_slot);
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
        let callee_val = self.stack_get(callee_idx);
        match &callee_val {
            Value::Closure(closure) => self.tail_call_closure(closure.clone(), num_args),
            Value::BaseFunction(_) => {
                // BaseFunctions don't push frames, so treat as normal call
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
        self.check_closure_contract_stack_args(&closure, num_args)?;

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
        let const_val = self.const_get(const_index);
        match &const_val {
            Value::Function(func) => {
                let func = func.clone();
                let mut free = Vec::with_capacity(num_free);
                for i in 0..num_free {
                    free.push(self.stack_get(self.sp - num_free + i));
                }
                self.reset_sp(self.sp - num_free)?;
                let closure = Closure::new(func, free);
                self.push(Value::Closure(Rc::new(closure)))
            }
            _ => Err("not a function".to_string()),
        }
    }

    /// Resume a captured continuation.
    ///
    /// Called from the OpCall dispatch when the callee is `Value::Continuation`.
    /// `num_args` must be 1 (the resume value). Returns `Ok(())` with the VM
    /// state restored to the captured continuation; ip_delta of 0 is returned by
    /// the OpCall arm so the restored frame's IP is left unchanged.
    pub(super) fn execute_resume(&mut self, num_args: usize) -> Result<(), String> {
        if num_args != 1 {
            return Err(format!("resume expects 1 argument, got {}", num_args));
        }
        let resume_val = self.pop_untracked()?;
        let cont_val = self.pop_untracked()?; // the callee (Continuation)

        let cont_rc = match cont_val {
            Value::Continuation(rc) => rc,
            _ => unreachable!("execute_resume called with non-Continuation callee"),
        };

        let (entry_frame_index, entry_sp, frames, stack, captured_sp, inner_handlers) = {
            let mut cont = cont_rc.borrow_mut();
            if cont.used {
                return Err("continuation already resumed (one-shot)".to_string());
            }
            cont.used = true;
            (
                cont.entry_frame_index,
                cont.entry_sp,
                cont.frames.clone(),
                cont.stack.clone(),
                cont.sp,
                cont.inner_handlers.clone(),
            )
        };

        // Unwind all frames above the handler boundary.
        self.frame_index = entry_frame_index;

        // Reset stack to handler boundary.
        self.reset_sp(entry_sp)?;

        // Restore inner handlers that were nested inside the captured region.
        for h in inner_handlers {
            self.handler_stack.push(h);
        }

        // Restore the captured stack slice.
        let stack_len = stack.len();
        self.ensure_stack_capacity(entry_sp + stack_len + 1)?;
        for (i, v) in stack.into_iter().enumerate() {
            self.stack_set(entry_sp + i, v);
        }

        // Place the resume value at the position corresponding to the result
        // of the perform expression (= captured_sp, right after the saved stack).
        self.stack_set(captured_sp, resume_val);
        self.sp = captured_sp + 1;

        // Restore captured frames above the handler boundary.
        for frame in frames {
            self.push_frame(frame);
        }

        Ok(())
    }

    /// Invokes a callable Value (closure or Flow function) with the given arguments
    /// and returns the result synchronously.
    ///
    /// Used by higher-order Flow functions (map, filter, fold) to call user-provided
    /// functions from within the Flow function implementation.
    pub fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        match callee {
            Value::BaseFunction(_) => {
                Err("BaseFunction values are deprecated; base functions are now compiled from lib/Flow/".to_string())
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
                self.check_closure_contract_value_args(&closure, &args)?;

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
        self.check_closure_contract_value_args(&closure, std::slice::from_ref(&arg))?;

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
        let args = [left.clone(), right.clone()];
        self.check_closure_contract_value_args(&closure, &args)?;

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

    fn invoke_base_function_borrowed(
        &mut self,
        _base_fn_index: usize,
        _args: &[&Value],
    ) -> Result<Value, String> {
        Err("invoke_base_function_borrowed is deprecated; base functions are now compiled from lib/Flow/".to_string())
    }

    #[inline]
    fn invoke_unary_value(&mut self, callee: &Value, arg: Value) -> Result<Value, String> {
        match callee {
            Value::BaseFunction(_) => Err("BaseFunction values are deprecated".to_string()),
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
            Value::BaseFunction(_) => Err("BaseFunction values are deprecated".to_string()),
            Value::Closure(closure) => self.invoke_closure_arity2(closure.clone(), left, right),
            other => Err(format!("not callable: {}", other.type_name())),
        }
    }

    fn callable_contract<'a>(
        &'a self,
        callee: &'a Value,
    ) -> Option<&'a crate::runtime::function_contract::FunctionContract> {
        match callee {
            Value::Closure(closure) => closure.function.contract.as_ref(),
            _ => None,
        }
    }
}
