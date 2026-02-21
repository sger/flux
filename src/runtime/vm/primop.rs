use crate::primop::{PrimOp, execute_primop};
use crate::runtime::value::Value;

use super::VM;

impl VM {
    pub(super) fn execute_primop_opcode(&mut self, primop_id: usize, arity: usize) -> Result<(), String> {
        let op = PrimOp::from_id(primop_id as u8).ok_or_else(|| format!("invalid primop id {}", primop_id))?;

        // Keep VM-side arity checks strict so malformed bytecode fails fast.
        if arity != op.arity() {
            return Err(format!(
                "primop {} expects {} args, got {}", 
                op.display_name(), 
                op.arity(), 
                arity
            ));
        }

        if self.sp < arity {
            return Err("stack underflow".to_string());
        }

        let mut args = Vec::with_capacity(arity);

        for _ in 0..arity {
            // Stack is LIFO; collect in reverse, then flip to call-order.
            args.push(self.pop()?);
        }

        args.reverse();

        let result = execute_primop(self, op, args)?;
        self.push(result)?;
        self.last_popped = Value::None;
        Ok(())
    }
}
