use std::rc::Rc;

use crate::runtime::{closure::Closure, frame::Frame, value::Value};
use crate::syntax::diagnostics::NOT_A_FUNCTION;

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
                let result = (builtin.func)(args)?;
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
}
