use crate::{
    cfg::{IrBinaryOp, IrCallTarget, IrConst, IrExpr, IrInstr, IrMetadata, IrStringPart, IrVar},
    core::{CoreExpr, CoreLit, CorePrimOp},
    diagnostics::position::Span,
};

/// Map a promoted primop back to its original function name for CFG lowering.
///
/// The CFG/bytecode path doesn't benefit from the promotion — the bytecode
/// compiler already dispatches these names via `resolve_primop_call` and
/// `OpCallBase`.  This function reverses the promotion so the CFG IR emits a
/// normal named call.
fn promoted_primop_name(op: &CorePrimOp) -> &'static str {
    match op {
        CorePrimOp::Print => "print",
        CorePrimOp::Println => "println",
        CorePrimOp::DebugTrace => "__primop_debug_trace",
        CorePrimOp::ReadFile => "read_file",
        CorePrimOp::WriteFile => "write_file",
        CorePrimOp::ReadStdin => "read_stdin",
        CorePrimOp::ReadLines => "read_lines",
        CorePrimOp::StringLength => "string_length",
        CorePrimOp::StringConcat => "string_concat",
        CorePrimOp::StringSlice => "string_slice",
        CorePrimOp::StringToBytes => "string_to_bytes",
        CorePrimOp::BytesLength => "bytes_length",
        CorePrimOp::BytesSlice => "bytes_slice",
        CorePrimOp::BytesToString => "bytes_to_string",
        CorePrimOp::ToString => "to_string",
        CorePrimOp::Split => "split",
        CorePrimOp::Trim => "trim",
        CorePrimOp::Upper => "upper",
        CorePrimOp::Lower => "lower",
        CorePrimOp::Replace => "replace",
        CorePrimOp::Substring => "substring",
        CorePrimOp::ArrayLen => "array_len",
        CorePrimOp::ArrayGet => "array_get",
        CorePrimOp::ArraySet => "array_set",
        CorePrimOp::ArrayPush => "array_push",
        CorePrimOp::ArrayConcat => "array_concat",
        CorePrimOp::ArraySlice => "array_slice",
        CorePrimOp::HamtGet => "get",
        CorePrimOp::HamtSet => "put",
        CorePrimOp::HamtDelete => "delete",
        CorePrimOp::HamtKeys => "keys",
        CorePrimOp::HamtValues => "values",
        CorePrimOp::HamtMerge => "merge",
        CorePrimOp::HamtSize => "size",
        CorePrimOp::HamtContains => "has_key",
        CorePrimOp::TypeOf => "type_of",
        CorePrimOp::IsInt => "is_int",
        CorePrimOp::IsFloat => "is_float",
        CorePrimOp::IsString => "is_string",
        CorePrimOp::IsBool => "is_bool",
        CorePrimOp::IsArray => "is_array",
        CorePrimOp::IsNone => "is_none",
        CorePrimOp::IsSome => "is_some",
        CorePrimOp::IsList => "is_list",
        CorePrimOp::IsMap => "is_map",
        CorePrimOp::CmpEq => "cmp_eq",
        CorePrimOp::CmpNe => "cmp_ne",
        CorePrimOp::Panic => "panic",
        CorePrimOp::Unwrap => "unwrap",
        CorePrimOp::SafeDiv => "safe_div",
        CorePrimOp::SafeMod => "safe_mod",
        CorePrimOp::ClockNow => "now_ms",
        CorePrimOp::Time => "time",
        CorePrimOp::Try => "try",
        CorePrimOp::AssertThrows => "assert_throws",
        CorePrimOp::ParseInt => "parse_int",
        CorePrimOp::Abs => "abs",
        CorePrimOp::FSqrt => "sqrt",
        CorePrimOp::FSin => "sin",
        CorePrimOp::FCos => "cos",
        CorePrimOp::FExp => "exp",
        CorePrimOp::FLog => "log",
        CorePrimOp::FFloor => "floor",
        CorePrimOp::FCeil => "ceil",
        CorePrimOp::FRound => "round",
        CorePrimOp::FTan => "tan",
        CorePrimOp::FAsin => "asin",
        CorePrimOp::FAcos => "acos",
        CorePrimOp::FAtan => "atan",
        CorePrimOp::FSinh => "sinh",
        CorePrimOp::FCosh => "cosh",
        CorePrimOp::FTanh => "tanh",
        CorePrimOp::FTruncate => "truncate",
        CorePrimOp::BitAnd => "bit_and",
        CorePrimOp::BitOr => "bit_or",
        CorePrimOp::BitXor => "bit_xor",
        CorePrimOp::BitShl => "bit_shl",
        CorePrimOp::BitShr => "bit_shr",
        CorePrimOp::Min => "min",
        CorePrimOp::Max => "max",
        CorePrimOp::Len => "len",
        CorePrimOp::TaskSpawn => "task_spawn",
        CorePrimOp::TaskBlockingJoin => "task_blocking_join",
        CorePrimOp::TaskCancel => "task_cancel",
        CorePrimOp::AsyncSleep => "async_sleep",
        CorePrimOp::AsyncYieldNow => "async_yield_now",
        CorePrimOp::AsyncBoth => "async_both",
        CorePrimOp::AsyncRace => "async_race",
        CorePrimOp::AsyncTimeout => "async_timeout",
        CorePrimOp::AsyncTimeoutResult => "async_timeout_result",
        CorePrimOp::AsyncScope => "async_scope",
        CorePrimOp::AsyncFork => "async_fork",
        CorePrimOp::AsyncTry => "async_try",
        CorePrimOp::AsyncFinally => "async_finally",
        CorePrimOp::AsyncBracket => "async_bracket",
        _ => unreachable!("not a promoted primop"),
    }
}

impl<'a> super::fn_ctx::FnCtx<'a> {
    /// Lower a `PrimOp` node.
    pub(super) fn lower_primop(&mut self, op: &CorePrimOp, args: &[CoreExpr], span: Span) -> IrVar {
        let dest = self.ctx.alloc_var();
        let meta = IrMetadata::from_span(span);
        match op {
            CorePrimOp::Add
            | CorePrimOp::Sub
            | CorePrimOp::Mul
            | CorePrimOp::Div
            | CorePrimOp::Mod
            | CorePrimOp::IAdd
            | CorePrimOp::ISub
            | CorePrimOp::IMul
            | CorePrimOp::IDiv
            | CorePrimOp::IMod
            | CorePrimOp::FAdd
            | CorePrimOp::FSub
            | CorePrimOp::FMul
            | CorePrimOp::FDiv
            | CorePrimOp::Eq
            | CorePrimOp::NEq
            | CorePrimOp::Lt
            | CorePrimOp::Le
            | CorePrimOp::Gt
            | CorePrimOp::Ge
            | CorePrimOp::ICmpEq
            | CorePrimOp::ICmpNe
            | CorePrimOp::ICmpLt
            | CorePrimOp::ICmpLe
            | CorePrimOp::ICmpGt
            | CorePrimOp::ICmpGe
            | CorePrimOp::FCmpEq
            | CorePrimOp::FCmpNe
            | CorePrimOp::FCmpLt
            | CorePrimOp::FCmpLe
            | CorePrimOp::FCmpGt
            | CorePrimOp::FCmpGe
            | CorePrimOp::And
            | CorePrimOp::Or
            | CorePrimOp::Concat => {
                let lv = self.lower_expr(&args[0]);
                let rv = self.lower_expr(&args[1]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Binary(primop_to_binop(op), lv, rv),
                    metadata: meta,
                });
            }
            CorePrimOp::Neg => {
                let v = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Prefix {
                        operator: "-".to_string(),
                        right: v,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::Not => {
                let v = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Prefix {
                        operator: "!".to_string(),
                        right: v,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::Interpolate => {
                let parts: Vec<IrStringPart> = args
                    .iter()
                    .map(|a| IrStringPart::Interpolation(self.lower_expr(a)))
                    .collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::InterpolatedString(parts),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeList => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeList(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeArray => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeArray(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeTuple => {
                let vs: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeTuple(vs),
                    metadata: meta,
                });
            }
            CorePrimOp::MakeHash => {
                let pairs: Vec<(IrVar, IrVar)> = args
                    .chunks(2)
                    .map(|chunk| (self.lower_expr(&chunk[0]), self.lower_expr(&chunk[1])))
                    .collect();
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MakeHash(pairs),
                    metadata: meta,
                });
            }
            CorePrimOp::Index => {
                let left = self.lower_expr(&args[0]);
                let index = self.lower_expr(&args[1]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::Index { left, index },
                    metadata: meta,
                });
            }
            // Promoted primops — lower back to named function calls.
            // The bytecode compiler already handles these via
            // resolve_primop_call / OpCallBase dispatch.
            CorePrimOp::Print
            | CorePrimOp::Println
            | CorePrimOp::DebugTrace
            | CorePrimOp::ReadFile
            | CorePrimOp::WriteFile
            | CorePrimOp::ReadStdin
            | CorePrimOp::ReadLines
            | CorePrimOp::StringLength
            | CorePrimOp::StringConcat
            | CorePrimOp::StringSlice
            | CorePrimOp::StringToBytes
            | CorePrimOp::BytesLength
            | CorePrimOp::BytesSlice
            | CorePrimOp::BytesToString
            | CorePrimOp::ToString
            | CorePrimOp::Split
            | CorePrimOp::Trim
            | CorePrimOp::Upper
            | CorePrimOp::Lower
            | CorePrimOp::Replace
            | CorePrimOp::Substring
            | CorePrimOp::ArrayLen
            | CorePrimOp::ArrayGet
            | CorePrimOp::ArraySet
            | CorePrimOp::ArrayPush
            | CorePrimOp::ArrayConcat
            | CorePrimOp::ArraySlice
            | CorePrimOp::HamtGet
            | CorePrimOp::HamtSet
            | CorePrimOp::HamtDelete
            | CorePrimOp::HamtKeys
            | CorePrimOp::HamtValues
            | CorePrimOp::HamtMerge
            | CorePrimOp::HamtSize
            | CorePrimOp::HamtContains
            | CorePrimOp::TypeOf
            | CorePrimOp::IsInt
            | CorePrimOp::IsFloat
            | CorePrimOp::IsString
            | CorePrimOp::IsBool
            | CorePrimOp::IsArray
            | CorePrimOp::IsNone
            | CorePrimOp::IsSome
            | CorePrimOp::IsList
            | CorePrimOp::IsMap
            | CorePrimOp::Panic
            | CorePrimOp::Unwrap
            | CorePrimOp::SafeDiv
            | CorePrimOp::SafeMod
            | CorePrimOp::ClockNow
            | CorePrimOp::Time
            | CorePrimOp::ParseInt
            | CorePrimOp::Abs
            | CorePrimOp::FSqrt
            | CorePrimOp::FSin
            | CorePrimOp::FCos
            | CorePrimOp::FExp
            | CorePrimOp::FLog
            | CorePrimOp::FFloor
            | CorePrimOp::FCeil
            | CorePrimOp::FRound
            | CorePrimOp::FTan
            | CorePrimOp::FAsin
            | CorePrimOp::FAcos
            | CorePrimOp::FAtan
            | CorePrimOp::FSinh
            | CorePrimOp::FCosh
            | CorePrimOp::FTanh
            | CorePrimOp::FTruncate
            | CorePrimOp::BitAnd
            | CorePrimOp::BitOr
            | CorePrimOp::BitXor
            | CorePrimOp::BitShl
            | CorePrimOp::BitShr
            | CorePrimOp::Min
            | CorePrimOp::Max
            | CorePrimOp::Len
            | CorePrimOp::CmpEq
            | CorePrimOp::CmpNe
            | CorePrimOp::Try
            | CorePrimOp::AssertThrows
            | CorePrimOp::TaskSpawn
            | CorePrimOp::TaskBlockingJoin
            | CorePrimOp::TaskCancel
            | CorePrimOp::AsyncSleep
            | CorePrimOp::AsyncYieldNow
            | CorePrimOp::AsyncBoth
            | CorePrimOp::AsyncRace
            | CorePrimOp::AsyncTimeout
            | CorePrimOp::AsyncTimeoutResult
            | CorePrimOp::AsyncScope
            | CorePrimOp::AsyncFork
            | CorePrimOp::AsyncTry
            | CorePrimOp::AsyncFinally
            | CorePrimOp::AsyncBracket => {
                let name_str = promoted_primop_name(op);
                let arg_vars: Vec<IrVar> = args.iter().map(|a| self.lower_expr(a)).collect();
                // Emit as a named builtin call using the BuiltinCall target
                // which carries the string name directly without interning.
                self.emit(IrInstr::Call {
                    dest,
                    target: IrCallTarget::Builtin(name_str),
                    args: arg_vars,
                    metadata: meta,
                });
            }
            // Effect handler ops — native-only, should never appear in CFG pipeline
            CorePrimOp::EvvGet
            | CorePrimOp::EvvSet
            | CorePrimOp::FreshMarker
            | CorePrimOp::EvvInsert
            | CorePrimOp::YieldTo
            | CorePrimOp::YieldExtend
            | CorePrimOp::YieldPrompt
            | CorePrimOp::IsYielding
            | CorePrimOp::PerformDirect => {
                // These are emitted only by the LIR lowerer for the native backend.
                // Emit a no-op constant for the VM path.
                self.emit(IrInstr::Assign {
                    dest,
                    expr: crate::cfg::IrExpr::None,
                    metadata: meta,
                });
            }
        }
        dest
    }
}

pub(super) fn primop_to_binop(op: &CorePrimOp) -> IrBinaryOp {
    match op {
        // Generic arithmetic
        CorePrimOp::Add | CorePrimOp::Concat => IrBinaryOp::Add,
        CorePrimOp::Sub => IrBinaryOp::Sub,
        CorePrimOp::Mul => IrBinaryOp::Mul,
        CorePrimOp::Div => IrBinaryOp::Div,
        CorePrimOp::Mod => IrBinaryOp::Mod,
        // Typed integer arithmetic — skip the runtime type-dispatch path
        CorePrimOp::IAdd => IrBinaryOp::IAdd,
        CorePrimOp::ISub => IrBinaryOp::ISub,
        CorePrimOp::IMul => IrBinaryOp::IMul,
        CorePrimOp::IDiv => IrBinaryOp::IDiv,
        CorePrimOp::IMod => IrBinaryOp::IMod,
        // Typed float arithmetic
        CorePrimOp::FAdd => IrBinaryOp::FAdd,
        CorePrimOp::FSub => IrBinaryOp::FSub,
        CorePrimOp::FMul => IrBinaryOp::FMul,
        CorePrimOp::FDiv => IrBinaryOp::FDiv,
        // Comparisons and logical
        CorePrimOp::Eq => IrBinaryOp::Eq,
        CorePrimOp::NEq => IrBinaryOp::NotEq,
        CorePrimOp::Lt => IrBinaryOp::Lt,
        CorePrimOp::Le => IrBinaryOp::Le,
        CorePrimOp::Gt => IrBinaryOp::Gt,
        CorePrimOp::Ge => IrBinaryOp::Ge,
        // Typed integer comparisons — map to generic IR comparison ops
        CorePrimOp::ICmpEq => IrBinaryOp::Eq,
        CorePrimOp::ICmpNe => IrBinaryOp::NotEq,
        CorePrimOp::ICmpLt => IrBinaryOp::Lt,
        CorePrimOp::ICmpLe => IrBinaryOp::Le,
        CorePrimOp::ICmpGt => IrBinaryOp::Gt,
        CorePrimOp::ICmpGe => IrBinaryOp::Ge,
        // Typed float comparisons
        CorePrimOp::FCmpEq => IrBinaryOp::Eq,
        CorePrimOp::FCmpNe => IrBinaryOp::NotEq,
        CorePrimOp::FCmpLt => IrBinaryOp::Lt,
        CorePrimOp::FCmpLe => IrBinaryOp::Le,
        CorePrimOp::FCmpGt => IrBinaryOp::Gt,
        CorePrimOp::FCmpGe => IrBinaryOp::Ge,
        CorePrimOp::And => IrBinaryOp::And,
        CorePrimOp::Or => IrBinaryOp::Or,
        _ => unreachable!("not a binary op: {:?}", op),
    }
}

pub(super) fn lower_lit(lit: &CoreLit) -> IrConst {
    match lit {
        CoreLit::Int(n) => IrConst::Int(*n),
        CoreLit::Float(f) => IrConst::Float(*f),
        CoreLit::Bool(b) => IrConst::Bool(*b),
        CoreLit::String(s) => IrConst::String(s.clone()),
        CoreLit::Unit => IrConst::Unit,
    }
}
