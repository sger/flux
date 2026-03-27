use crate::core::CorePrimOp;
use crate::primop::{PrimOp, execute_primop};
use crate::runtime::value::Value;

use super::VM;
use super::slot;

impl VM {
    /// Executes the `OpPrimOp` VM instruction.
    ///
    /// Decodes the provided `primop_id` as a `CorePrimOp` discriminant,
    /// translates to the legacy `PrimOp` for execution, pops `arity`
    /// arguments from the stack (preserving call order), invokes the shared
    /// primop executor, and pushes the result.
    pub(super) fn execute_primop_opcode(
        &mut self,
        primop_id: usize,
        arity: usize,
    ) -> Result<(), String> {
        let core_op = CorePrimOp::from_id(primop_id as u8)
            .ok_or_else(|| format!("invalid CorePrimOp id {}", primop_id))?;

        let op = core_to_primop(core_op)
            .ok_or_else(|| format!("CorePrimOp {:?} has no PrimOp equivalent", core_op))?;

        // Keep VM-side arity checks strict so malformed bytecode fails fast.
        // AssertThrows accepts 1 or 2 arguments (optional expected message).
        if op != PrimOp::AssertThrows && arity != op.arity() {
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
        self.last_popped = slot::to_slot(Value::None);
        Ok(())
    }
}

/// Translate a `CorePrimOp` to the legacy `PrimOp` for VM execution.
///
/// Returns `None` for CorePrimOp variants that have no PrimOp equivalent
/// (generic arithmetic, constructors, etc. — these are handled by other
/// opcodes and should never appear in `OpPrimOp`).
fn core_to_primop(op: CorePrimOp) -> Option<PrimOp> {
    Some(match op {
        CorePrimOp::IAdd => PrimOp::IAdd,
        CorePrimOp::ISub => PrimOp::ISub,
        CorePrimOp::IMul => PrimOp::IMul,
        CorePrimOp::IDiv => PrimOp::IDiv,
        CorePrimOp::IMod => PrimOp::IMod,
        CorePrimOp::FAdd => PrimOp::FAdd,
        CorePrimOp::FSub => PrimOp::FSub,
        CorePrimOp::FMul => PrimOp::FMul,
        CorePrimOp::FDiv => PrimOp::FDiv,
        CorePrimOp::Abs => PrimOp::Abs,
        CorePrimOp::Min => PrimOp::Min,
        CorePrimOp::Max => PrimOp::Max,
        CorePrimOp::ICmpEq => PrimOp::ICmpEq,
        CorePrimOp::ICmpNe => PrimOp::ICmpNe,
        CorePrimOp::ICmpLt => PrimOp::ICmpLt,
        CorePrimOp::ICmpLe => PrimOp::ICmpLe,
        CorePrimOp::ICmpGt => PrimOp::ICmpGt,
        CorePrimOp::ICmpGe => PrimOp::ICmpGe,
        CorePrimOp::FCmpEq => PrimOp::FCmpEq,
        CorePrimOp::FCmpNe => PrimOp::FCmpNe,
        CorePrimOp::FCmpLt => PrimOp::FCmpLt,
        CorePrimOp::FCmpLe => PrimOp::FCmpLe,
        CorePrimOp::FCmpGt => PrimOp::FCmpGt,
        CorePrimOp::FCmpGe => PrimOp::FCmpGe,
        CorePrimOp::CmpEq => PrimOp::CmpEq,
        CorePrimOp::CmpNe => PrimOp::CmpNe,
        CorePrimOp::ArrayLen => PrimOp::ArrayLen,
        CorePrimOp::ArrayGet => PrimOp::ArrayGet,
        CorePrimOp::ArraySet => PrimOp::ArraySet,
        CorePrimOp::ArrayPush => PrimOp::Push,
        CorePrimOp::ArrayConcat => PrimOp::ConcatArray,
        CorePrimOp::ArraySlice => PrimOp::Slice,
        CorePrimOp::HamtGet => PrimOp::MapGet,
        CorePrimOp::HamtSet => PrimOp::MapSet,
        CorePrimOp::HamtContains => PrimOp::MapHas,
        CorePrimOp::HamtDelete => PrimOp::MapDelete,
        CorePrimOp::HamtKeys => PrimOp::MapKeys,
        CorePrimOp::HamtValues => PrimOp::MapValues,
        CorePrimOp::HamtMerge => PrimOp::MapMerge,
        CorePrimOp::HamtSize => PrimOp::MapSize,
        CorePrimOp::StringLength => PrimOp::StringLen,
        CorePrimOp::StringConcat => PrimOp::StringConcat,
        CorePrimOp::StringSlice => PrimOp::StringSlice,
        CorePrimOp::Println => PrimOp::Println,
        CorePrimOp::Print => PrimOp::Print,
        CorePrimOp::ReadFile => PrimOp::ReadFile,
        CorePrimOp::WriteFile => PrimOp::WriteFile,
        CorePrimOp::ReadStdin => PrimOp::ReadStdin,
        CorePrimOp::ReadLines => PrimOp::ReadLines,
        CorePrimOp::ClockNow => PrimOp::ClockNow,
        CorePrimOp::Time => PrimOp::Time,
        CorePrimOp::Panic => PrimOp::Panic,
        CorePrimOp::Len => PrimOp::Len,
        CorePrimOp::Split => PrimOp::Split,
        CorePrimOp::IsInt => PrimOp::IsInt,
        CorePrimOp::IsFloat => PrimOp::IsFloat,
        CorePrimOp::IsString => PrimOp::IsString,
        CorePrimOp::IsBool => PrimOp::IsBool,
        CorePrimOp::IsArray => PrimOp::IsArray,
        CorePrimOp::IsNone => PrimOp::IsNone,
        CorePrimOp::IsSome => PrimOp::IsSomeV,
        CorePrimOp::IsList => PrimOp::IsList,
        CorePrimOp::IsMap => PrimOp::IsHash,
        CorePrimOp::ToString => PrimOp::ToString,
        CorePrimOp::Join => PrimOp::Join,
        CorePrimOp::Trim => PrimOp::Trim,
        CorePrimOp::StartsWith => PrimOp::StartsWith,
        CorePrimOp::EndsWith => PrimOp::EndsWith,
        CorePrimOp::Chars => PrimOp::Chars,
        CorePrimOp::Replace => PrimOp::Replace,
        CorePrimOp::StrContains => PrimOp::StrContains,
        CorePrimOp::Upper => PrimOp::Upper,
        CorePrimOp::Lower => PrimOp::Lower,
        CorePrimOp::TypeOf => PrimOp::TypeOf,
        CorePrimOp::ToList => PrimOp::ToList,
        CorePrimOp::ToArray => PrimOp::ToArray,
        CorePrimOp::ParseInt => PrimOp::ParseInt,
        CorePrimOp::ParseInts => PrimOp::ParseInts,
        CorePrimOp::SplitInts => PrimOp::SplitInts,
        CorePrimOp::Try => PrimOp::Try,
        CorePrimOp::AssertThrows => PrimOp::AssertThrows,
        CorePrimOp::Substring => PrimOp::StringSlice,
        // No PrimOp equivalent — handled by dedicated opcodes:
        CorePrimOp::Add
        | CorePrimOp::Sub
        | CorePrimOp::Mul
        | CorePrimOp::Div
        | CorePrimOp::Mod
        | CorePrimOp::Neg
        | CorePrimOp::Not
        | CorePrimOp::Eq
        | CorePrimOp::NEq
        | CorePrimOp::Lt
        | CorePrimOp::Le
        | CorePrimOp::Gt
        | CorePrimOp::Ge
        | CorePrimOp::And
        | CorePrimOp::Or
        | CorePrimOp::Concat
        | CorePrimOp::Interpolate
        | CorePrimOp::MakeList
        | CorePrimOp::MakeArray
        | CorePrimOp::MakeTuple
        | CorePrimOp::MakeHash
        | CorePrimOp::Index => return None,
    })
}
