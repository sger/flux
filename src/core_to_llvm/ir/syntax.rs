use super::LlvmType;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LlvmLocal(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlobalId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LabelId(pub String);

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmModule {
    pub source_filename: Option<String>,
    pub target_triple: Option<String>,
    pub data_layout: Option<String>,
    pub type_defs: Vec<LlvmTypeDef>,
    pub globals: Vec<LlvmGlobal>,
    pub declarations: Vec<LlvmDecl>,
    pub functions: Vec<LlvmFunction>,
}

impl LlvmModule {
    pub fn new() -> Self {
        Self {
            source_filename: None,
            target_triple: None,
            data_layout: None,
            type_defs: Vec::new(),
            globals: Vec::new(),
            declarations: Vec::new(),
            functions: Vec::new(),
        }
    }
}

impl Default for LlvmModule {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmTypeDef {
    pub name: String,
    pub ty: LlvmType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    External,
    Private,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallConv {
    Ccc,
    Fastcc,
}

pub type LlvmCallingConv = CallConv;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmFunctionSig {
    pub ret: LlvmType,
    pub params: Vec<LlvmType>,
    pub varargs: bool,
    pub call_conv: CallConv,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmGlobal {
    pub linkage: Linkage,
    pub name: GlobalId,
    pub ty: LlvmType,
    pub is_constant: bool,
    pub value: LlvmConst,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmDecl {
    pub linkage: Linkage,
    pub name: GlobalId,
    pub sig: LlvmFunctionSig,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmFunction {
    pub linkage: Linkage,
    pub name: GlobalId,
    pub sig: LlvmFunctionSig,
    pub params: Vec<LlvmLocal>,
    pub attrs: Vec<String>,
    pub blocks: Vec<LlvmBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlvmBlock {
    pub label: LabelId,
    pub instrs: Vec<LlvmInstr>,
    pub term: LlvmTerminator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlvmConst {
    Int {
        bits: u32,
        value: i128,
    },
    Float(f64),
    Null,
    Undef,
    Array {
        element_ty: LlvmType,
        elements: Vec<LlvmConst>,
    },
    Struct {
        packed: bool,
        fields: Vec<(LlvmType, LlvmConst)>,
    },
    GlobalRef(GlobalId),
    ZeroInit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlvmOperand {
    Local(LlvmLocal),
    Global(GlobalId),
    Const(LlvmConst),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlvmCmpOp {
    Eq,
    Ne,
    Sgt,
    Sge,
    Slt,
    Sle,
    Oeq,
    One,
    Ogt,
    Oge,
    Olt,
    Ole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlvmValueKind {
    Add,
    Sub,
    Mul,
    SDiv,
    UDiv,
    SRem,
    FAdd,
    FSub,
    FMul,
    FDiv,
    And,
    Or,
    Xor,
    Shl,
    LShr,
    AShr,
    Alloca,
    Load,
    GetElementPtr,
    ExtractValue,
    InsertValue,
    IntToPtr,
    PtrToInt,
    Bitcast,
    ZExt,
    SExt,
    Trunc,
    FpToSi,
    SiToFp,
    Icmp(LlvmCmpOp),
    Fcmp(LlvmCmpOp),
    Phi,
    Select,
    Call,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlvmInstr {
    Alloca {
        dst: LlvmLocal,
        ty: LlvmType,
        count: Option<(LlvmType, LlvmOperand)>,
        align: Option<u32>,
    },
    Load {
        dst: LlvmLocal,
        ty: LlvmType,
        ptr: LlvmOperand,
        align: Option<u32>,
    },
    Store {
        ty: LlvmType,
        value: LlvmOperand,
        ptr: LlvmOperand,
        align: Option<u32>,
    },
    Binary {
        dst: LlvmLocal,
        op: LlvmValueKind,
        ty: LlvmType,
        lhs: LlvmOperand,
        rhs: LlvmOperand,
    },
    Cast {
        dst: LlvmLocal,
        op: LlvmValueKind,
        from_ty: LlvmType,
        operand: LlvmOperand,
        to_ty: LlvmType,
    },
    Icmp {
        dst: LlvmLocal,
        op: LlvmCmpOp,
        ty: LlvmType,
        lhs: LlvmOperand,
        rhs: LlvmOperand,
    },
    Fcmp {
        dst: LlvmLocal,
        op: LlvmCmpOp,
        ty: LlvmType,
        lhs: LlvmOperand,
        rhs: LlvmOperand,
    },
    Phi {
        dst: LlvmLocal,
        ty: LlvmType,
        incoming: Vec<(LlvmOperand, LabelId)>,
    },
    Select {
        dst: LlvmLocal,
        cond_ty: LlvmType,
        cond: LlvmOperand,
        value_ty: LlvmType,
        then_value: LlvmOperand,
        else_value: LlvmOperand,
    },
    Call {
        dst: Option<LlvmLocal>,
        tail: bool,
        call_conv: Option<CallConv>,
        ret_ty: LlvmType,
        callee: LlvmOperand,
        args: Vec<(LlvmType, LlvmOperand)>,
        attrs: Vec<String>,
    },
    GetElementPtr {
        dst: LlvmLocal,
        inbounds: bool,
        element_ty: LlvmType,
        base: LlvmOperand,
        indices: Vec<(LlvmType, LlvmOperand)>,
    },
    ExtractValue {
        dst: LlvmLocal,
        aggregate_ty: LlvmType,
        aggregate: LlvmOperand,
        indices: Vec<u32>,
    },
    InsertValue {
        dst: LlvmLocal,
        aggregate_ty: LlvmType,
        aggregate: LlvmOperand,
        element_ty: LlvmType,
        element: LlvmOperand,
        indices: Vec<u32>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlvmTerminator {
    RetVoid,
    Ret {
        ty: LlvmType,
        value: LlvmOperand,
    },
    Br {
        target: LabelId,
    },
    CondBr {
        cond_ty: LlvmType,
        cond: LlvmOperand,
        then_label: LabelId,
        else_label: LabelId,
    },
    Switch {
        ty: LlvmType,
        scrutinee: LlvmOperand,
        default: LabelId,
        cases: Vec<(LlvmConst, LabelId)>,
    },
    Unreachable,
}
