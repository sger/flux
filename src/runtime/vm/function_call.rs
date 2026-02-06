use std::rc::Rc;

use crate::frontend::diagnostics::NOT_A_FUNCTION;
use crate::runtime::{closure::Closure, frame::Frame, object::Object};

use super::VM;

impl VM {
    pub(super) fn execute_call(&mut self, num_args: usize) -> Result<(), String> {
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

    pub(super) fn push_closure(
        &mut self,
        const_index: usize,
        num_free: usize,
    ) -> Result<(), String> {
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
}
