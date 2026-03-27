use crate::core::CorePrimOp;
use crate::runtime::value::Value;

use super::VM;
use super::core_dispatch::execute_core_primop;
use super::slot;

impl VM {
    /// Executes the `OpPrimOp` VM instruction.
    ///
    /// Decodes the provided `primop_id` as a `CorePrimOp` discriminant,
    /// pops `arity` arguments from the stack (preserving call order),
    /// dispatches to `execute_core_primop`, and pushes the result.
    pub(super) fn execute_primop_opcode(
        &mut self,
        primop_id: usize,
        arity: usize,
    ) -> Result<(), String> {
        let op = CorePrimOp::from_id(primop_id as u8)
            .ok_or_else(|| format!("invalid CorePrimOp id {}", primop_id))?;

        // Keep VM-side arity checks strict so malformed bytecode fails fast.
        // AssertThrows accepts 1 or 2 arguments (optional expected message).
        if op != CorePrimOp::AssertThrows && arity != op.arity() {
            return Err(format!(
                "primop {:?} expects {} args, got {}",
                op,
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

        let result = execute_core_primop(self, op, args)?;
        self.push(result)?;
        self.last_popped = slot::to_slot(Value::None);
        Ok(())
    }
}
