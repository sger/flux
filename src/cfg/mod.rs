use std::{collections::HashMap, fmt};

use crate::{
    diagnostics::position::Span,
    syntax::{
        Identifier,
        block::Block,
        data_variant::DataVariant,
        effect_expr::EffectExpr,
        effect_ops::EffectOp,
        expression::{ExprId, Expression, Pattern},
        type_expr::TypeExpr,
    },
    types::infer_type::InferType,
};

pub mod lower;
pub mod passes;
pub mod validate;

pub use lower::lower_program_to_ir;
pub use passes::{IrPassContext, run_ir_pass_pipeline};
pub use validate::validate_ir;

/// Update `function_id` in each `IrTopLevelItem::Function` of `items` to
/// reference the Core IR-derived CFG function instead of the old structured-IR
/// function.  Matching is done by function name.  Items whose name is not
/// present in `core_items` are left unchanged (e.g. module-nested functions
/// that the Core IR pipeline does not yet lower).
///
/// Returns the set of old `FunctionId`s that were NOT replaced (i.e. functions
/// the Core IR doesn't cover).  The caller must preserve the corresponding
/// `IrFunction` entries from the old function list for these IDs.
pub fn patch_function_ids_from_core(
    items: &mut [IrTopLevelItem],
    core_items: &[IrTopLevelItem],
) -> Vec<FunctionId> {
    let name_to_id: HashMap<Identifier, FunctionId> = core_items
        .iter()
        .filter_map(|item| {
            if let IrTopLevelItem::Function { name, function_id: Some(id), .. } = item {
                Some((*name, *id))
            } else {
                None
            }
        })
        .collect();
    let mut kept_old_ids = Vec::new();
    patch_function_ids_recursive(items, &name_to_id, &mut kept_old_ids);
    kept_old_ids
}

fn patch_function_ids_recursive(
    items: &mut [IrTopLevelItem],
    name_to_id: &HashMap<Identifier, FunctionId>,
    kept_old_ids: &mut Vec<FunctionId>,
) {
    for item in items {
        match item {
            IrTopLevelItem::Function { name, function_id, .. } => {
                if let Some(&core_id) = name_to_id.get(name) {
                    // Replace with the Core IR function_id.
                    *function_id = Some(core_id);
                } else {
                    // No Core IR match — keep the original function_id so the
                    // old IrFunction can still be looked up.  The caller must
                    // merge the old function into the new function list.
                    if let Some(old_id) = *function_id {
                        kept_old_ids.push(old_id);
                    }
                }
            }
            IrTopLevelItem::Module { body, .. } => {
                patch_function_ids_recursive(body, name_to_id, kept_old_ids);
            }
            _ => {}
        }
    }
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
    };
}

id_type!(IrVar);
id_type!(BlockId);
id_type!(FunctionId);
id_type!(GlobalId);
id_type!(LiteralId);
id_type!(AdtId);
id_type!(EffectId);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IrType {
    Any,
    Int,
    Float,
    Bool,
    String,
    List,
    Array,
    Tuple(usize),
    Hash,
    Adt(AdtId),
    Function(usize),
    Unit,
    Never,
}

#[derive(Debug, Clone)]
pub struct IrMetadata {
    pub span: Option<Span>,
    pub inferred_type: Option<InferType>,
    pub expr_id: Option<ExprId>,
}

impl IrMetadata {
    pub fn empty() -> Self {
        Self {
            span: None,
            inferred_type: None,
            expr_id: None,
        }
    }

    /// Create metadata carrying only a source span.
    pub fn from_span(span: Span) -> Self {
        Self {
            span: Some(span),
            inferred_type: None,
            expr_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrConst {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    IAdd,
    ISub,
    IMul,
    IDiv,
    IMod,
    FAdd,
    FSub,
    FMul,
    FDiv,
}

#[derive(Debug, Clone)]
pub enum IrStringPart {
    Literal(String),
    Interpolation(IrVar),
}


#[derive(Debug, Clone)]
pub struct IrHandleArm {
    pub operation_name: Identifier,
    pub resume_param: Identifier,
    pub params: Vec<Identifier>,
    pub body: Box<Expression>,
    pub metadata: IrMetadata,
}

/// A handler arm in a `HandleScope` instruction.
/// Each arm is compiled as a separate function (closure) referenced by `FunctionId`.
#[derive(Debug, Clone)]
pub struct HandleScopeArm {
    pub operation_name: Identifier,
    pub function_id: FunctionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrTagTest {
    None,
    Some,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrListTest {
    Empty,
    Cons,
}

#[derive(Debug, Clone)]
pub enum IrExpr {
    Const(IrConst),
    Var(IrVar),
    LoadName(Identifier),
    InterpolatedString(Vec<IrStringPart>),
    Prefix {
        operator: String,
        right: IrVar,
    },
    Binary(IrBinaryOp, IrVar, IrVar),
    MakeTuple(Vec<IrVar>),
    MakeArray(Vec<IrVar>),
    MakeHash(Vec<(IrVar, IrVar)>),
    MakeList(Vec<IrVar>),
    MakeAdt(Identifier, Vec<IrVar>),
    MakeClosure(FunctionId, Vec<IrVar>),
    EmptyList,
    Index {
        left: IrVar,
        index: IrVar,
    },
    MemberAccess {
        object: IrVar,
        member: Identifier,
        /// Original module name (e.g. `Base`, `MyModule`) when known from
        /// the source.  Carried through so the bytecode CFG path can resolve
        /// the qualified binding without tracing back through IrVars.
        module_name: Option<Identifier>,
    },
    TupleFieldAccess {
        object: IrVar,
        index: usize,
    },
    TupleArityTest {
        value: IrVar,
        arity: usize,
    },
    TagTest {
        value: IrVar,
        tag: IrTagTest,
    },
    TagPayload {
        value: IrVar,
        tag: IrTagTest,
    },
    ListTest {
        value: IrVar,
        tag: IrListTest,
    },
    ListHead {
        value: IrVar,
    },
    ListTail {
        value: IrVar,
    },
    AdtTagTest {
        value: IrVar,
        constructor: Identifier,
    },
    AdtField {
        value: IrVar,
        index: usize,
    },
    None,
    Some(IrVar),
    Left(IrVar),
    Right(IrVar),
    Cons {
        head: IrVar,
        tail: IrVar,
    },
    Perform {
        effect: Identifier,
        operation: Identifier,
        args: Vec<IrVar>,
    },
    Handle {
        expr: IrVar,
        effect: Identifier,
        arms: Vec<IrHandleArm>,
    },
}

#[derive(Debug, Clone)]
pub enum IrCallTarget {
    Direct(FunctionId),
    Named(Identifier),
    Var(IrVar),
}

#[derive(Debug, Clone)]
pub enum IrInstr {
    Assign {
        dest: IrVar,
        expr: IrExpr,
        metadata: IrMetadata,
    },
    Call {
        dest: IrVar,
        target: IrCallTarget,
        args: Vec<IrVar>,
        metadata: IrMetadata,
    },
    /// Scoped effect handler: installs a handler before executing the body
    /// blocks, then removes it after. Solves the ordering issue where
    /// `IrExpr::Handle` emits body instructions before the handler install.
    ///
    /// Bytecode emission order:
    ///   arm closures → OpHandle → body blocks → OpEndHandle
    HandleScope {
        effect: Identifier,
        arms: Vec<HandleScopeArm>,
        /// Entry block for the handled body.
        body_entry: BlockId,
        /// Var holding the body's result (set in the body blocks).
        body_result: IrVar,
        dest: IrVar,
        metadata: IrMetadata,
    },
}

#[derive(Debug, Clone)]
pub enum IrTerminator {
    Jump(BlockId, Vec<IrVar>, IrMetadata),
    Branch {
        cond: IrVar,
        then_block: BlockId,
        else_block: BlockId,
        metadata: IrMetadata,
    },
    Return(IrVar, IrMetadata),
    TailCall {
        callee: IrCallTarget,
        args: Vec<IrVar>,
        metadata: IrMetadata,
    },
    Unreachable(IrMetadata),
}

#[derive(Debug, Clone)]
pub struct IrBlockParam {
    pub var: IrVar,
    pub ty: IrType,
}

#[derive(Debug, Clone)]
pub struct IrBlock {
    pub id: BlockId,
    pub params: Vec<IrBlockParam>,
    pub instrs: Vec<IrInstr>,
    pub terminator: IrTerminator,
}

#[derive(Debug, Clone)]
pub struct IrParam {
    pub name: Identifier,
    pub var: IrVar,
    pub ty: IrType,
}

#[derive(Debug, Clone)]
pub enum IrFunctionOrigin {
    ModuleTopLevel,
    NamedFunction,
    FunctionLiteral,
}

#[derive(Debug, Clone)]
pub struct IrFunction {
    pub id: FunctionId,
    pub name: Option<Identifier>,
    pub params: Vec<IrParam>,
    pub parameter_types: Vec<Option<TypeExpr>>,
    pub return_type_annotation: Option<TypeExpr>,
    pub effects: Vec<EffectExpr>,
    pub captures: Vec<Identifier>,
    pub body_span: Span,
    pub ret_type: IrType,
    pub blocks: Vec<IrBlock>,
    pub entry: BlockId,
    pub origin: IrFunctionOrigin,
    pub metadata: IrMetadata,
}

#[derive(Debug, Clone)]
pub enum IrTopLevelItem {
    Let {
        name: Identifier,
        type_annotation: Option<TypeExpr>,
        value: Expression,
        span: Span,
    },
    LetDestructure {
        pattern: Pattern,
        value: Expression,
        span: Span,
    },
    Return {
        value: Option<Expression>,
        span: Span,
    },
    Expression {
        expression: Expression,
        has_semicolon: bool,
        span: Span,
    },
    Function {
        is_public: bool,
        name: Identifier,
        type_params: Vec<Identifier>,
        function_id: Option<FunctionId>,
        parameters: Vec<Identifier>,
        parameter_types: Vec<Option<TypeExpr>>,
        return_type: Option<TypeExpr>,
        effects: Vec<EffectExpr>,
        body: Block,
        span: Span,
    },
    Assign {
        name: Identifier,
        value: Expression,
        span: Span,
    },
    Module {
        name: Identifier,
        body: Vec<IrTopLevelItem>,
        span: Span,
    },
    Import {
        name: Identifier,
        alias: Option<Identifier>,
        except: Vec<Identifier>,
        span: Span,
    },
    Data {
        name: Identifier,
        type_params: Vec<Identifier>,
        variants: Vec<DataVariant>,
        span: Span,
    },
    EffectDecl {
        name: Identifier,
        ops: Vec<EffectOp>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct IrProgram {
    pub top_level_items: Vec<IrTopLevelItem>,
    pub functions: Vec<IrFunction>,
    pub entry: FunctionId,
    pub globals: Vec<Identifier>,
    pub hm_expr_types: HashMap<ExprId, InferType>,
    /// Core IR representation — populated by `lower_program_to_ir`.
    /// `None` when the Core IR lowering pass has not been run.
    pub core: Option<crate::nary::CoreProgram>,
}

fn ir_fmt_var(v: IrVar) -> String {
    format!("v{}", v.0)
}

fn ir_fmt_block(b: BlockId) -> String {
    format!("b{}", b.0)
}

fn ir_fmt_call_target(target: &IrCallTarget) -> String {
    match target {
        IrCallTarget::Direct(fid) => format!("fn{}", fid.0),
        IrCallTarget::Named(name) => format!("#{}", name.as_u32()),
        IrCallTarget::Var(v) => ir_fmt_var(*v),
    }
}

fn ir_fmt_terminator(t: &IrTerminator) -> String {
    match t {
        IrTerminator::Return(v, _) => format!("Return {}", ir_fmt_var(*v)),
        IrTerminator::Jump(b, args, _) => {
            let args_s: Vec<_> = args.iter().map(|v| ir_fmt_var(*v)).collect();
            format!("Jump {}({})", ir_fmt_block(*b), args_s.join(", "))
        }
        IrTerminator::Branch { cond, then_block, else_block, .. } => format!(
            "Branch {} ? {} : {}",
            ir_fmt_var(*cond),
            ir_fmt_block(*then_block),
            ir_fmt_block(*else_block)
        ),
        IrTerminator::TailCall { callee, args, .. } => {
            let args_s: Vec<_> = args.iter().map(|v| ir_fmt_var(*v)).collect();
            format!("TailCall {}({})", ir_fmt_call_target(callee), args_s.join(", "))
        }
        IrTerminator::Unreachable(_) => "Unreachable".to_string(),
    }
}

fn ir_fmt_expr(expr: &IrExpr) -> String {
    match expr {
        IrExpr::Const(c) => match c {
            IrConst::Int(n) => format!("Const({})", n),
            IrConst::Float(f) => format!("Const({}f)", f),
            IrConst::Bool(b) => format!("Const({})", b),
            IrConst::String(s) => format!("Const({:?})", s),
            IrConst::Unit => "Const(())".to_string(),
        },
        IrExpr::Var(v) => ir_fmt_var(*v),
        IrExpr::LoadName(n) => format!("LoadName(#{})", n.as_u32()),
        IrExpr::InterpolatedString(parts) => {
            let inner: Vec<_> = parts
                .iter()
                .map(|p| match p {
                    IrStringPart::Literal(s) => format!("{:?}", s),
                    IrStringPart::Interpolation(v) => ir_fmt_var(*v),
                })
                .collect();
            format!("InterpolatedString[{}]", inner.join(", "))
        }
        IrExpr::Prefix { operator, right } => {
            format!("Prefix({}, {})", operator, ir_fmt_var(*right))
        }
        IrExpr::Binary(op, lhs, rhs) => {
            format!("Binary({:?}, {}, {})", op, ir_fmt_var(*lhs), ir_fmt_var(*rhs))
        }
        IrExpr::MakeTuple(vars) => {
            let s: Vec<_> = vars.iter().map(|v| ir_fmt_var(*v)).collect();
            format!("MakeTuple({})", s.join(", "))
        }
        IrExpr::MakeArray(vars) => {
            let s: Vec<_> = vars.iter().map(|v| ir_fmt_var(*v)).collect();
            format!("MakeArray({})", s.join(", "))
        }
        IrExpr::MakeList(vars) => {
            let s: Vec<_> = vars.iter().map(|v| ir_fmt_var(*v)).collect();
            format!("MakeList({})", s.join(", "))
        }
        IrExpr::MakeHash(pairs) => {
            let s: Vec<_> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", ir_fmt_var(*k), ir_fmt_var(*v)))
                .collect();
            format!("MakeHash({})", s.join(", "))
        }
        IrExpr::MakeAdt(name, fields) => {
            let s: Vec<_> = fields.iter().map(|v| ir_fmt_var(*v)).collect();
            format!("MakeAdt(#{}, [{}])", name.as_u32(), s.join(", "))
        }
        IrExpr::MakeClosure(fid, captures) => {
            let s: Vec<_> = captures.iter().map(|v| ir_fmt_var(*v)).collect();
            format!("MakeClosure(fn{}, [{}])", fid.0, s.join(", "))
        }
        IrExpr::EmptyList => "EmptyList".to_string(),
        IrExpr::Index { left, index } => {
            format!("Index({}, {})", ir_fmt_var(*left), ir_fmt_var(*index))
        }
        IrExpr::MemberAccess { object, member, .. } => {
            format!("MemberAccess({}, #{})", ir_fmt_var(*object), member.as_u32())
        }
        IrExpr::TupleFieldAccess { object, index } => {
            format!("TupleFieldAccess({}, {})", ir_fmt_var(*object), index)
        }
        IrExpr::TupleArityTest { value, arity } => {
            format!("TupleArityTest({}, {})", ir_fmt_var(*value), arity)
        }
        IrExpr::TagTest { value, tag } => {
            format!("TagTest({}, {:?})", ir_fmt_var(*value), tag)
        }
        IrExpr::TagPayload { value, tag } => {
            format!("TagPayload({}, {:?})", ir_fmt_var(*value), tag)
        }
        IrExpr::ListTest { value, tag } => {
            format!("ListTest({}, {:?})", ir_fmt_var(*value), tag)
        }
        IrExpr::ListHead { value } => format!("ListHead({})", ir_fmt_var(*value)),
        IrExpr::ListTail { value } => format!("ListTail({})", ir_fmt_var(*value)),
        IrExpr::AdtTagTest { value, constructor } => {
            format!("AdtTagTest({}, #{})", ir_fmt_var(*value), constructor.as_u32())
        }
        IrExpr::AdtField { value, index } => {
            format!("AdtField({}, {})", ir_fmt_var(*value), index)
        }
        IrExpr::None => "None".to_string(),
        IrExpr::Some(v) => format!("Some({})", ir_fmt_var(*v)),
        IrExpr::Left(v) => format!("Left({})", ir_fmt_var(*v)),
        IrExpr::Right(v) => format!("Right({})", ir_fmt_var(*v)),
        IrExpr::Cons { head, tail } => {
            format!("Cons({}, {})", ir_fmt_var(*head), ir_fmt_var(*tail))
        }
        IrExpr::Perform { effect, operation, args } => {
            let s: Vec<_> = args.iter().map(|v| ir_fmt_var(*v)).collect();
            format!(
                "Perform(#{}.#{}, [{}])",
                effect.as_u32(),
                operation.as_u32(),
                s.join(", ")
            )
        }
        IrExpr::Handle { expr, effect, arms } => {
            format!(
                "Handle(v{}, #{}, {} arms)",
                expr.0,
                effect.as_u32(),
                arms.len()
            )
        }
    }
}

impl IrProgram {
    pub fn function(&self, id: FunctionId) -> Option<&IrFunction> {
        self.functions.iter().find(|function| function.id == id)
    }

    pub fn dump_text(&self) -> String {
        let mut out = String::new();
        for function in &self.functions {
            let origin = match function.origin {
                IrFunctionOrigin::ModuleTopLevel => "ModuleTopLevel",
                IrFunctionOrigin::NamedFunction => "NamedFunction",
                IrFunctionOrigin::FunctionLiteral => "FunctionLiteral",
            };
            let name = function
                .name
                .map(|n| format!("#{}", n.as_u32()))
                .unwrap_or_else(|| "<anon>".to_string());
            out.push_str(&format!("fn {} [{}]\n", name, origin));
            for block in &function.blocks {
                out.push_str(&format!("  b{}(", block.id.0));
                for (i, param) in block.params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!("v{}: {:?}", param.var.0, param.ty));
                }
                out.push_str("):\n");
                for instr in &block.instrs {
                    match instr {
                        IrInstr::Assign { dest, expr, .. } => {
                            out.push_str(&format!("    v{} = {}\n", dest.0, ir_fmt_expr(expr)));
                        }
                        IrInstr::Call { dest, target, args, .. } => {
                            let args_s: Vec<_> = args.iter().map(|v| format!("v{}", v.0)).collect();
                            out.push_str(&format!(
                                "    v{} = call {}({})\n",
                                dest.0,
                                ir_fmt_call_target(target),
                                args_s.join(", ")
                            ));
                        }
                        IrInstr::HandleScope { effect, arms, body_entry, dest, .. } => {
                            let arm_s: Vec<_> = arms.iter().map(|a| format!("#{} -> fn{}", a.operation_name.as_u32(), a.function_id.0)).collect();
                            out.push_str(&format!(
                                "    v{} = HandleScope(#{}, body=B{}, arms=[{}])\n",
                                dest.0, effect.as_u32(), body_entry.0, arm_s.join(", ")
                            ));
                        }
                    }
                }
                out.push_str(&format!("    {}\n", ir_fmt_terminator(&block.terminator)));
            }
            out.push('\n');
        }
        out
    }

    /// Like `dump_text` but resolves symbol IDs to human-readable names via `interner`.
    pub fn dump_text_with_interner(
        &self,
        interner: &crate::syntax::interner::Interner,
    ) -> String {
        let sym = |s: Identifier| interner.resolve(s).to_string();
        let fmt_name = |n: Option<Identifier>| {
            n.map(|s| sym(s)).unwrap_or_else(|| "<anon>".to_string())
        };
        let fmt_target = |t: &IrCallTarget| match t {
            IrCallTarget::Direct(fid) => format!("fn{}", fid.0),
            IrCallTarget::Named(name) => sym(*name),
            IrCallTarget::Var(v) => format!("v{}", v.0),
        };
        let fmt_term = |t: &IrTerminator| {
            let fv = |v: IrVar| format!("v{}", v.0);
            let fb = |b: BlockId| format!("b{}", b.0);
            match t {
                IrTerminator::Return(v, _) => format!("Return {}", fv(*v)),
                IrTerminator::Jump(b, args, _) => {
                    let a: Vec<_> = args.iter().map(|v| fv(*v)).collect();
                    format!("Jump {}({})", fb(*b), a.join(", "))
                }
                IrTerminator::Branch { cond, then_block, else_block, .. } => format!(
                    "Branch {} ? {} : {}",
                    fv(*cond),
                    fb(*then_block),
                    fb(*else_block)
                ),
                IrTerminator::TailCall { callee, args, .. } => {
                    let a: Vec<_> = args.iter().map(|v| fv(*v)).collect();
                    format!("TailCall {}({})", fmt_target(callee), a.join(", "))
                }
                IrTerminator::Unreachable(_) => "Unreachable".to_string(),
            }
        };

        let mut out = String::new();
        for function in &self.functions {
            let origin = match function.origin {
                IrFunctionOrigin::ModuleTopLevel => "ModuleTopLevel",
                IrFunctionOrigin::NamedFunction => "NamedFunction",
                IrFunctionOrigin::FunctionLiteral => "FunctionLiteral",
            };
            out.push_str(&format!("fn {} [{}]\n", fmt_name(function.name), origin));
            for block in &function.blocks {
                out.push_str(&format!("  b{}(", block.id.0));
                for (i, param) in block.params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!("v{}: {:?}", param.var.0, param.ty));
                }
                out.push_str("):\n");
                for instr in &block.instrs {
                    match instr {
                        IrInstr::Assign { dest, expr, .. } => {
                            out.push_str(&format!("    v{} = {}\n", dest.0, ir_fmt_expr(expr)));
                        }
                        IrInstr::Call { dest, target, args, .. } => {
                            let a: Vec<_> = args.iter().map(|v| format!("v{}", v.0)).collect();
                            out.push_str(&format!(
                                "    v{} = call {}({})\n",
                                dest.0,
                                fmt_target(target),
                                a.join(", ")
                            ));
                        }
                        IrInstr::HandleScope { effect, arms, body_entry, dest, .. } => {
                            let arm_s: Vec<_> = arms.iter().map(|a| {
                                let op_name = interner.resolve(a.operation_name);
                                format!("{} -> fn{}", op_name, a.function_id.0)
                            }).collect();
                            let eff_name = interner.resolve(*effect);
                            out.push_str(&format!(
                                "    v{} = HandleScope({}, body=B{}, arms=[{}])\n",
                                dest.0, eff_name, body_entry.0, arm_s.join(", ")
                            ));
                        }
                    }
                }
                out.push_str(&format!("    {}\n", fmt_term(&block.terminator)));
            }
            out.push('\n');
        }
        out
    }
}

impl fmt::Display for IrProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dump_text())
    }
}
