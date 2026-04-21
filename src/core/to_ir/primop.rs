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
        CorePrimOp::ReadFile => "read_file",
        CorePrimOp::WriteFile => "write_file",
        CorePrimOp::ReadStdin => "read_stdin",
        CorePrimOp::ReadLines => "read_lines",
        CorePrimOp::StringLength => "string_length",
        CorePrimOp::StringConcat => "string_concat",
        CorePrimOp::StringSlice => "string_slice",
        CorePrimOp::ToString => "to_string",
        CorePrimOp::Split => "split",
        CorePrimOp::Join => "join",
        CorePrimOp::Trim => "trim",
        CorePrimOp::Upper => "upper",
        CorePrimOp::Lower => "lower",
        CorePrimOp::StartsWith => "starts_with",
        CorePrimOp::EndsWith => "ends_with",
        CorePrimOp::Replace => "replace",
        CorePrimOp::Substring => "substring",
        CorePrimOp::Chars => "chars",
        CorePrimOp::StrContains => "str_contains",
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
        CorePrimOp::ParseInts => "parse_ints",
        CorePrimOp::SplitInts => "split_ints",
        CorePrimOp::ToList => "to_list",
        CorePrimOp::ToArray => "to_array",
        CorePrimOp::Abs => "abs",
        CorePrimOp::FSqrt => "sqrt",
        CorePrimOp::FSin => "sin",
        CorePrimOp::FCos => "cos",
        CorePrimOp::FExp => "exp",
        CorePrimOp::FLog => "log",
        CorePrimOp::FFloor => "floor",
        CorePrimOp::FCeil => "ceil",
        CorePrimOp::FRound => "round",
        CorePrimOp::BitAnd => "bit_and",
        CorePrimOp::BitOr => "bit_or",
        CorePrimOp::BitXor => "bit_xor",
        CorePrimOp::BitShl => "bit_shl",
        CorePrimOp::BitShr => "bit_shr",
        CorePrimOp::Min => "min",
        CorePrimOp::Max => "max",
        CorePrimOp::Len => "len",
        CorePrimOp::ArrayReverse => "array_reverse",
        CorePrimOp::ArrayContains => "array_contains",
        CorePrimOp::Sort => "sort",
        CorePrimOp::SortBy => "sort_by",
        CorePrimOp::HoMap => "map",
        CorePrimOp::HoFilter => "filter",
        CorePrimOp::HoFold => "fold",
        CorePrimOp::HoAny => "any",
        CorePrimOp::HoAll => "all",
        CorePrimOp::HoEach => "each",
        CorePrimOp::HoFind => "find",
        CorePrimOp::HoCount => "count",
        CorePrimOp::Zip => "zip",
        CorePrimOp::Flatten => "flatten",
        CorePrimOp::HoFlatMap => "flat_map",
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
            | CorePrimOp::ReadFile
            | CorePrimOp::WriteFile
            | CorePrimOp::ReadStdin
            | CorePrimOp::ReadLines
            | CorePrimOp::StringLength
            | CorePrimOp::StringConcat
            | CorePrimOp::StringSlice
            | CorePrimOp::ToString
            | CorePrimOp::Split
            | CorePrimOp::Join
            | CorePrimOp::Trim
            | CorePrimOp::Upper
            | CorePrimOp::Lower
            | CorePrimOp::StartsWith
            | CorePrimOp::EndsWith
            | CorePrimOp::Replace
            | CorePrimOp::Substring
            | CorePrimOp::Chars
            | CorePrimOp::StrContains
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
            | CorePrimOp::ParseInts
            | CorePrimOp::SplitInts
            | CorePrimOp::ToList
            | CorePrimOp::ToArray
            | CorePrimOp::Abs
            | CorePrimOp::FSqrt
            | CorePrimOp::FSin
            | CorePrimOp::FCos
            | CorePrimOp::FExp
            | CorePrimOp::FLog
            | CorePrimOp::FFloor
            | CorePrimOp::FCeil
            | CorePrimOp::FRound
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
            | CorePrimOp::ArrayReverse
            | CorePrimOp::ArrayContains
            | CorePrimOp::Sort
            | CorePrimOp::SortBy
            | CorePrimOp::HoMap
            | CorePrimOp::HoFilter
            | CorePrimOp::HoFold
            | CorePrimOp::HoAny
            | CorePrimOp::HoAll
            | CorePrimOp::HoEach
            | CorePrimOp::HoFind
            | CorePrimOp::HoCount
            | CorePrimOp::Zip
            | CorePrimOp::Flatten
            | CorePrimOp::HoFlatMap => {
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
