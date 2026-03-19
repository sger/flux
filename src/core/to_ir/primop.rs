use crate::{
    cfg::{IrBinaryOp, IrConst, IrExpr, IrInstr, IrMetadata, IrStringPart, IrVar},
    core::{CoreExpr, CoreLit, CorePrimOp},
    diagnostics::position::Span,
};

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
            CorePrimOp::MemberAccess(member) => {
                let module_name = match &args[0] {
                    CoreExpr::Var { var, .. } => Some(var.name),
                    _ => None,
                };
                let object = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::MemberAccess {
                        object,
                        member: *member,
                        module_name,
                    },
                    metadata: meta,
                });
            }
            CorePrimOp::TupleField(idx) => {
                let object = self.lower_expr(&args[0]);
                self.emit(IrInstr::Assign {
                    dest,
                    expr: IrExpr::TupleFieldAccess {
                        object,
                        index: *idx,
                    },
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
