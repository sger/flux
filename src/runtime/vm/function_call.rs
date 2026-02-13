use std::rc::Rc;

use crate::diagnostics::NOT_A_FUNCTION;
use crate::runtime::RuntimeContext;
use crate::runtime::gc::GcHeap;
use crate::runtime::{closure::Closure, frame::Frame, value::Value};

use super::VM;

impl VM {
    pub(super) fn execute_call(&mut self, num_args: usize) -> Result<(), String> {
        let callee = self.stack[self.sp - 1 - num_args].clone();
        match callee {
            Value::Closure(closure) => self.call_closure(closure, num_args),
            Value::Builtin(builtin) => {
                let mut args = Vec::with_capacity(num_args);
                for i in self.sp - num_args..self.sp {
                    args.push(std::mem::replace(&mut self.stack[i], Value::None));
                }

                self.sp -= num_args + 1;
                let result = (builtin.func)(self, args)?;
                self.push(result)?;
                // Advance past the OpCall operand since builtins don't push a new frame.
                self.current_frame_mut().ip += 1;
                Ok(())
            }
            _ => Err(self.runtime_error_enhanced(&NOT_A_FUNCTION, &[callee.type_name()])),
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
        self.ensure_stack_capacity(self.sp + num_locals)?;
        self.sp += num_locals;
        Ok(())
    }

    pub(super) fn execute_tail_call(&mut self, num_args: usize) -> Result<(), String> {
        let callee = self.stack[self.sp - 1 - num_args].clone();

        match callee {
            Value::Closure(closure) => self.tail_call_closure(closure, num_args),
            Value::Builtin(_) => {
                // Builtins don't push frames, so treat as normal call
                self.execute_call(num_args)
            }
            _ => Err(self.runtime_error_enhanced(&NOT_A_FUNCTION, &[callee.type_name()])),
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
        let mut new_args = Vec::with_capacity(num_args);
        for i in 0..num_args {
            new_args.push(self.stack[self.sp - num_args + i].clone());
        }

        // Overwrite old locals with new arguments
        for (i, arg) in new_args.into_iter().enumerate() {
            self.stack[base_pointer + i] = arg;
        }

        // Reset stack pointer and instruction pointer
        self.ensure_stack_capacity(base_pointer + closure.function.num_locals)?;
        self.sp = base_pointer + closure.function.num_locals;
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
                let mut free = Vec::with_capacity(num_free);
                for i in 0..num_free {
                    free.push(self.stack[self.sp - num_free + i].clone());
                }
                self.sp -= num_free;
                let closure = Closure::new(func.clone(), free);
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
            Value::Builtin(builtin) => (builtin.func)(self, args),
            Value::Closure(closure) => {
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
                self.push_frame(frame);
                self.ensure_stack_capacity(self.sp + num_locals)?;
                self.sp += num_locals;

                // Track frame index so we know when the closure returns
                let target_frame_index = self.frame_index;

                // Run the dispatch loop until this frame returns
                while self.frame_index >= target_frame_index {
                    if self.frame_index == target_frame_index
                        && self.current_frame().ip >= self.current_frame().instructions().len()
                    {
                        return Err("callable exited without return".to_string());
                    }
                    self.execute_current_instruction(Some(target_frame_index))?;
                }

                // The return value is on the stack (pushed by OpReturnValue/OpReturn)
                self.pop()
            }
            _ => Err(format!("not callable: {}", callee.type_name())),
        }
    }
}

impl RuntimeContext for VM {
    fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        VM::invoke_value(self, callee, args)
    }

    fn gc_heap(&self) -> &GcHeap {
        &self.gc_heap
    }

    fn gc_heap_mut(&mut self) -> &mut GcHeap {
        &mut self.gc_heap
    }
}
