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
    /// Debug metadata, for a build with `-g` (proposal 0188 S5).
    pub debug: Option<ModuleDebug>,
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
            debug: None,
        }
    }
}

/// A module's debug metadata, kept beside the functions it describes so the
/// code that builds functions needs no change.
///
/// Node `i` prints as `!i = <text>`; the text is whatever LLVM metadata the
/// emitter built (`!DISubprogram(...)` and the like).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleDebug {
    pub nodes: Vec<String>,
    /// Named metadata such as `!llvm.dbg.cu = !{!0}`.
    pub named: Vec<(String, Vec<u32>)>,
    /// Per function, by its name.
    pub functions: std::collections::HashMap<String, FunctionDebug>,
}

impl ModuleDebug {
    /// Adds a node and returns its number.
    pub fn node(&mut self, text: String) -> u32 {
        self.nodes.push(text);
        (self.nodes.len() - 1) as u32
    }
}

/// A function's subprogram and the location of each instruction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctionDebug {
    pub subprogram: u32,
    /// The location of any instruction no mark covers: line 0 in this
    /// function. LLVM requires a location on every call in a function with
    /// a subprogram.
    pub line_zero: u32,
    /// Per block, by its position in the function.
    pub blocks: Vec<BlockDebug>,
}

/// Where a block's instructions came from: from instruction `index` on, the
/// location is node `loc`, until the next mark.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockDebug {
    pub marks: Vec<(usize, u32)>,
    pub term: Option<u32>,
}

impl BlockDebug {
    /// The location node of instruction `index`, if a mark covers it.
    pub fn loc_of(&self, index: usize) -> Option<u32> {
        self.marks
            .iter()
            .take_while(|(from, _)| *from <= index)
            .last()
            .map(|(_, loc)| *loc)
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
    /// `None` for external global declarations (no initializer).
    pub value: Option<LlvmConst>,
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
    /// A debug intrinsic's metadata argument, as written: `ptr %slot`,
    /// `!12`, `!DIExpression()`.
    Metadata(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlvmCmpOp {
    Eq,
    Ne,
    Sgt,
    Sge,
    Slt,
    Sle,
    Ule,
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
    FRem,
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
