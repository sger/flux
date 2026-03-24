use std::{collections::HashMap, fmt};

use crate::{
    core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram},
    core_to_llvm::{
        CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmFunction, LlvmFunctionSig, LlvmInstr,
        LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator, LlvmType, LlvmValueKind,
        emit_adt_support, emit_closure_support, emit_prelude_and_arith,
    },
    syntax::{Identifier, interner::Interner},
};

use super::{adt::AdtMetadata, closure::common_closure_load_instrs, expr::FunctionLowering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreToLlvmError {
    Unsupported {
        feature: &'static str,
        context: String,
    },
    Malformed {
        message: String,
    },
    MissingSymbol {
        message: String,
    },
}

impl fmt::Display for CoreToLlvmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreToLlvmError::Unsupported { feature, context } => {
                write!(f, "unsupported CoreToLlvm feature `{feature}`: {context}")
            }
            CoreToLlvmError::Malformed { message } => {
                write!(f, "malformed Core lowering: {message}")
            }
            CoreToLlvmError::MissingSymbol { message } => {
                write!(f, "missing CoreToLlvm symbol: {message}")
            }
        }
    }
}

impl std::error::Error for CoreToLlvmError {}

#[derive(Debug, Clone)]
pub(super) struct TopLevelFunctionInfo {
    pub symbol: GlobalId,
    pub arity: usize,
    pub name: Identifier,
}

pub(super) struct ProgramState<'a> {
    pub interner: Option<&'a Interner>,
    pub top_level: HashMap<CoreBinderId, TopLevelFunctionInfo>,
    pub adt_metadata: AdtMetadata,
    pub generated_functions: Vec<LlvmFunction>,
    pub top_level_wrappers: HashMap<CoreBinderId, GlobalId>,
    pub next_lambda_id: u32,
    /// Builtin C runtime functions referenced during codegen.
    pub needed_builtins: Vec<&'static super::builtins::BuiltinMapping>,
    /// String literal globals to emit (name, content).
    pub generated_string_globals: Vec<(GlobalId, String)>,
    /// C runtime declarations needed (name, param types, return type).
    pub needed_c_decls: Vec<(String, Vec<LlvmType>, LlvmType)>,
}

impl<'a> ProgramState<'a> {
    fn new(
        top_level: HashMap<CoreBinderId, TopLevelFunctionInfo>,
        adt_metadata: AdtMetadata,
        interner: Option<&'a Interner>,
    ) -> Self {
        Self {
            interner,
            top_level,
            adt_metadata,
            generated_functions: Vec::new(),
            top_level_wrappers: HashMap::new(),
            next_lambda_id: 0,
            needed_builtins: Vec::new(),
            generated_string_globals: Vec::new(),
            needed_c_decls: Vec::new(),
        }
    }

    /// Track a C runtime function declaration needed by the codegen.
    pub fn ensure_c_decl(&mut self, name: &str, params: &[LlvmType], ret: LlvmType) {
        if !self.needed_c_decls.iter().any(|(n, _, _)| n == name) {
            self.needed_c_decls
                .push((name.to_string(), params.to_vec(), ret));
        }
    }

    pub fn register_builtin(&mut self, mapping: &'static super::builtins::BuiltinMapping) {
        if !self
            .needed_builtins
            .iter()
            .any(|m| m.c_name == mapping.c_name)
        {
            self.needed_builtins.push(mapping);
        }
    }

    pub fn fresh_lambda_symbol(&mut self, hint: &str) -> GlobalId {
        let id = self.next_lambda_id;
        self.next_lambda_id += 1;
        GlobalId(format!("{}.lambda.{id}", sanitize_symbol_fragment(hint)))
    }

    pub fn push_generated_function(&mut self, function: LlvmFunction) {
        self.generated_functions.push(function);
    }

    pub fn top_level_info(&self, binder: CoreBinderId) -> Option<&TopLevelFunctionInfo> {
        self.top_level.get(&binder)
    }

    /// Look up a top-level function by its Identifier (name), for MemberAccess resolution.
    /// Returns (CoreBinderId, &TopLevelFunctionInfo) so the caller can use ensure_top_level_wrapper.
    pub fn top_level_by_name_with_binder(
        &self,
        name: Identifier,
    ) -> Option<(CoreBinderId, TopLevelFunctionInfo)> {
        self.top_level
            .iter()
            .find(|(_, info)| info.name == name)
            .map(|(k, v)| (*k, v.clone()))
    }

    pub fn ensure_top_level_wrapper(
        &mut self,
        binder: CoreBinderId,
    ) -> Result<GlobalId, CoreToLlvmError> {
        if let Some(symbol) = self.top_level_wrappers.get(&binder) {
            return Ok(symbol.clone());
        }
        let info =
            self.top_level
                .get(&binder)
                .cloned()
                .ok_or_else(|| CoreToLlvmError::MissingSymbol {
                    message: format!("missing wrapper target for binder {:?}", binder),
                })?;
        let wrapper = GlobalId(format!(
            "{}.closure_wrapper",
            sanitize_symbol_name(info.name, self.interner)
        ));
        let function = build_top_level_wrapper(&wrapper, &info.symbol, info.arity);
        self.generated_functions.push(function);
        self.top_level_wrappers.insert(binder, wrapper.clone());
        Ok(wrapper)
    }
}

pub fn compile_program(core: &CoreProgram) -> Result<LlvmModule, CoreToLlvmError> {
    compile_program_with_interner(core, None)
}

pub fn compile_program_with_interner(
    core: &CoreProgram,
    interner: Option<&Interner>,
) -> Result<LlvmModule, CoreToLlvmError> {
    let mut module = LlvmModule::new();
    emit_prelude_and_arith(&mut module);
    emit_closure_support(&mut module);
    let adt_metadata = AdtMetadata::collect(core, interner)?;
    emit_adt_support(&mut module, &adt_metadata);

    let mut top_level = HashMap::new();
    for def in &core.defs {
        let CoreExpr::Lam { params, .. } = &def.expr else {
            return Err(CoreToLlvmError::Unsupported {
                feature: "top-level value definitions",
                context: format!(
                    "definition `{}` is not a lambda",
                    display_ident(def.name, interner)
                ),
            });
        };
        let raw_name = sanitize_symbol_name(def.name, interner);
        // Rename user's `main` to `flux_main` so the C runtime can call it.
        let symbol_name = if raw_name == "main" {
            "flux_main".to_string()
        } else {
            raw_name
        };
        top_level.insert(
            def.binder.id,
            TopLevelFunctionInfo {
                symbol: GlobalId(symbol_name),
                arity: params.len(),
                name: def.name,
            },
        );
    }

    let mut program = ProgramState::new(top_level, adt_metadata, interner);
    for def in &core.defs {
        let info = program
            .top_level
            .get(&def.binder.id)
            .cloned()
            .ok_or_else(|| CoreToLlvmError::MissingSymbol {
                message: format!(
                    "missing top-level symbol for `{}`",
                    display_ident(def.name, interner)
                ),
            })?;
        let function =
            lower_top_level_function(def, info.symbol.clone(), def.is_recursive, &mut program)?;
        module.functions.push(function);
    }
    module.functions.extend(program.generated_functions);

    // Make flux_main externally visible so the C runtime's main() can call it.
    // Also use ccc (C calling convention) for the entry point.
    for func in &mut module.functions {
        if func.name.0 == "flux_main" {
            func.linkage = crate::core_to_llvm::Linkage::External;
            func.sig.call_conv = crate::core_to_llvm::CallConv::Ccc;
        }
    }

    // Add C runtime declarations for any builtin functions referenced.
    for mapping in &program.needed_builtins {
        super::builtins::ensure_builtin_declared(&mut module, mapping);
    }

    // Add C runtime declarations requested by codegen.
    for (name, params, ret) in &program.needed_c_decls {
        if !module.declarations.iter().any(|d| d.name.0 == *name)
            && !module.functions.iter().any(|f| f.name.0 == *name)
        {
            module.declarations.push(crate::core_to_llvm::LlvmDecl {
                linkage: crate::core_to_llvm::Linkage::External,
                name: GlobalId(name.clone()),
                sig: crate::core_to_llvm::LlvmFunctionSig {
                    ret: ret.clone(),
                    params: params.clone(),
                    varargs: false,
                    call_conv: crate::core_to_llvm::CallConv::Ccc,
                },
                attrs: vec!["nounwind".into()],
            });
        }
    }

    // Emit string literal globals.
    for (name, content) in &program.generated_string_globals {
        module.globals.push(crate::core_to_llvm::LlvmGlobal {
            linkage: crate::core_to_llvm::Linkage::Private,
            name: name.clone(),
            ty: LlvmType::Array {
                len: content.len() as u64,
                element: Box::new(LlvmType::i8()),
            },
            is_constant: true,
            value: crate::core_to_llvm::LlvmConst::Array {
                element_ty: LlvmType::i8(),
                elements: content
                    .bytes()
                    .map(|b| crate::core_to_llvm::LlvmConst::Int {
                        bits: 8,
                        value: b as i128,
                    })
                    .collect(),
            },
            attrs: vec![],
        });
    }

    // Ensure flux_string_new is declared with its actual signature (ptr, i32) → i64.
    if !program.generated_string_globals.is_empty() {
        let name = "flux_string_new";
        if !module.declarations.iter().any(|d| d.name.0 == name)
            && !module.functions.iter().any(|f| f.name.0 == name)
        {
            module.declarations.push(crate::core_to_llvm::LlvmDecl {
                linkage: crate::core_to_llvm::Linkage::External,
                name: GlobalId(name.into()),
                sig: crate::core_to_llvm::LlvmFunctionSig {
                    ret: LlvmType::i64(),
                    params: vec![LlvmType::ptr(), LlvmType::i32()],
                    varargs: false,
                    call_conv: crate::core_to_llvm::CallConv::Ccc,
                },
                attrs: vec!["nounwind".into()],
            });
        }
    }

    Ok(module)
}

pub(super) fn display_ident(ident: Identifier, interner: Option<&Interner>) -> String {
    interner
        .map(|it| it.resolve(ident).to_string())
        .unwrap_or_else(|| ident.to_string())
}

pub(super) fn sanitize_symbol_name(ident: Identifier, interner: Option<&Interner>) -> String {
    sanitize_symbol_fragment(&display_ident(ident, interner))
}

pub(super) fn sanitize_symbol_fragment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "_anon".into() } else { out }
}

fn lower_top_level_function(
    def: &CoreDef,
    symbol: GlobalId,
    is_recursive: bool,
    program: &mut ProgramState<'_>,
) -> Result<LlvmFunction, CoreToLlvmError> {
    let CoreExpr::Lam { params, body, .. } = &def.expr else {
        return Err(CoreToLlvmError::Malformed {
            message: format!(
                "top-level function `{}` was not lowered as Lam",
                display_ident(def.name, program.interner)
            ),
        });
    };

    let mut lowering = FunctionLowering::new_top_level(symbol.clone(), params, program);
    if is_recursive {
        lowering.setup_tco_loop();
    }
    let result = lowering.lower_expr(body)?;
    lowering.finish_with_return(result)
}

fn build_top_level_wrapper(wrapper: &GlobalId, target: &GlobalId, arity: usize) -> LlvmFunction {
    let mut state = FunctionState::new_closure_entry(wrapper.clone(), HashMap::new(), None);
    state.blocks[0]
        .instrs
        .extend(common_closure_load_instrs(LlvmOperand::Local(LlvmLocal(
            "closure_raw".into(),
        ))));
    let mut instrs = emit_closure_param_unpack(&mut state, arity, 0);
    let mut args = Vec::with_capacity(arity);
    for index in 0..arity {
        args.push((
            LlvmType::i64(),
            LlvmOperand::Local(LlvmLocal(format!("param.{index}"))),
        ));
    }
    instrs.push(LlvmInstr::Call {
        dst: Some(LlvmLocal("result".into())),
        tail: false,
        call_conv: Some(CallConv::Fastcc),
        ret_ty: LlvmType::i64(),
        callee: LlvmOperand::Global(target.clone()),
        args,
        attrs: vec![],
    });
    state.blocks[0].instrs.extend(instrs);
    state.blocks[0].terminator = Some(LlvmTerminator::Ret {
        ty: LlvmType::i64(),
        value: LlvmOperand::Local(LlvmLocal("result".into())),
    });
    state.finish().expect("top-level wrapper should be valid")
}

pub(super) fn emit_closure_param_unpack(
    state: &mut FunctionState<'_>,
    arity: usize,
    capture_count: usize,
) -> Vec<LlvmInstr> {
    let mut instrs = Vec::new();
    let payload = LlvmOperand::Local(LlvmLocal("payload".into()));
    for index in 0..arity {
        let applied_gep = LlvmLocal(format!("param.src.applied.{index}"));
        let applied_load = LlvmLocal(format!("param.applied.{index}"));
        let applied_idx = capture_count as i32 + index as i32;
        instrs.push(LlvmInstr::GetElementPtr {
            dst: applied_gep.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: payload.clone(),
            indices: vec![(LlvmType::i32(), const_i32_operand(applied_idx))],
        });
        instrs.push(LlvmInstr::Load {
            dst: applied_load.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(applied_gep),
            align: Some(8),
        });
        let new_arg_idx = LlvmLocal(format!("param.new.idx.{index}"));
        instrs.push(LlvmInstr::Binary {
            dst: new_arg_idx.clone(),
            op: LlvmValueKind::Sub,
            ty: LlvmType::i32(),
            lhs: const_i32_operand(index as i32),
            rhs: LlvmOperand::Local(LlvmLocal("applied_count".into())),
        });
        let new_gep = LlvmLocal(format!("param.src.new.{index}"));
        instrs.push(LlvmInstr::GetElementPtr {
            dst: new_gep.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(LlvmLocal("args".into())),
            indices: vec![(LlvmType::i32(), LlvmOperand::Local(new_arg_idx))],
        });
        let new_load = LlvmLocal(format!("param.new.{index}"));
        instrs.push(LlvmInstr::Load {
            dst: new_load.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(new_gep),
            align: Some(8),
        });
        let cond = LlvmLocal(format!("param.is_applied.{index}"));
        instrs.push(LlvmInstr::Icmp {
            dst: cond.clone(),
            op: crate::core_to_llvm::LlvmCmpOp::Slt,
            ty: LlvmType::i32(),
            lhs: const_i32_operand(index as i32),
            rhs: LlvmOperand::Local(LlvmLocal("applied_count".into())),
        });
        instrs.push(LlvmInstr::Select {
            dst: LlvmLocal(format!("param.{index}")),
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(cond),
            value_ty: LlvmType::i64(),
            then_value: LlvmOperand::Local(applied_load),
            else_value: LlvmOperand::Local(new_load),
        });
    }
    let _ = state;
    instrs
}

/// State for self-tail-call optimization.  When present, the function body
/// is lowered inside a loop block; tail self-calls store updated argument
/// values into the parameter alloca slots and branch back to `loop_header`.
pub(super) struct TcoLoopState {
    /// Label of the loop header block (body starts here).
    pub loop_header: LabelId,
    /// Alloca slots for each parameter, in order.  Tail self-calls store new
    /// argument values into these slots before branching to `loop_header`.
    pub param_slots: Vec<LlvmLocal>,
}

pub(super) struct FunctionBlock {
    pub label: LabelId,
    pub instrs: Vec<LlvmInstr>,
    pub terminator: Option<LlvmTerminator>,
}

impl FunctionBlock {
    fn into_llvm(self) -> Result<LlvmBlock, CoreToLlvmError> {
        Ok(LlvmBlock {
            label: self.label,
            instrs: self.instrs,
            term: self.terminator.ok_or_else(|| CoreToLlvmError::Malformed {
                message: "LLVM block finished without terminator".into(),
            })?,
        })
    }
}

pub(super) struct FunctionState<'a> {
    pub symbol: GlobalId,
    pub interner: Option<&'a Interner>,
    #[allow(dead_code)]
    pub top_level_symbols: HashMap<CoreBinderId, GlobalId>,
    pub param_bindings: Vec<(CoreBinder, LlvmLocal)>,
    pub llvm_params: Vec<LlvmLocal>,
    pub llvm_param_types: Vec<LlvmType>,
    pub ret_ty: LlvmType,
    pub call_conv: CallConv,
    pub blocks: Vec<FunctionBlock>,
    pub current_block: usize,
    pub entry_allocas: Vec<LlvmInstr>,
    pub next_tmp: u32,
    pub next_slot: u32,
    pub next_block_id: u32,
    pub local_slots: HashMap<CoreBinderId, LlvmLocal>,
    pub binder_names: HashMap<CoreBinderId, Identifier>,
    /// TCO loop state — present when the function is self-recursive.
    pub tco_loop: Option<TcoLoopState>,
}

impl<'a> FunctionState<'a> {
    pub fn new_top_level(
        symbol: GlobalId,
        params: &[CoreBinder],
        top_level_symbols: HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a Interner>,
    ) -> Self {
        let param_bindings = params
            .iter()
            .enumerate()
            .map(|(idx, binder)| {
                (
                    CoreBinder::new(binder.id, binder.name),
                    LlvmLocal(format!("arg{idx}")),
                )
            })
            .collect::<Vec<_>>();
        let llvm_params = param_bindings
            .iter()
            .map(|(_, local)| local.clone())
            .collect::<Vec<_>>();
        let llvm_param_types = llvm_params.iter().map(|_| LlvmType::i64()).collect();
        Self::base(
            symbol,
            top_level_symbols,
            interner,
            param_bindings,
            llvm_params,
            llvm_param_types,
            LlvmType::i64(),
            CallConv::Fastcc,
        )
    }

    pub fn new_closure_entry(
        symbol: GlobalId,
        top_level_symbols: HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a Interner>,
    ) -> Self {
        Self::base(
            symbol,
            top_level_symbols,
            interner,
            Vec::new(),
            vec![
                LlvmLocal("closure_raw".into()),
                LlvmLocal("args".into()),
                LlvmLocal("nargs".into()),
            ],
            vec![LlvmType::i64(), LlvmType::ptr(), LlvmType::i32()],
            LlvmType::i64(),
            CallConv::Fastcc,
        )
    }

    fn base(
        symbol: GlobalId,
        top_level_symbols: HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a Interner>,
        param_bindings: Vec<(CoreBinder, LlvmLocal)>,
        llvm_params: Vec<LlvmLocal>,
        llvm_param_types: Vec<LlvmType>,
        ret_ty: LlvmType,
        call_conv: CallConv,
    ) -> Self {
        let binder_names = param_bindings
            .iter()
            .map(|(binder, _)| (binder.id, binder.name))
            .collect();
        Self {
            symbol,
            interner,
            top_level_symbols,
            param_bindings,
            llvm_params,
            llvm_param_types,
            ret_ty,
            call_conv,
            blocks: vec![FunctionBlock {
                label: LabelId("entry".into()),
                instrs: Vec::new(),
                terminator: None,
            }],
            current_block: 0,
            entry_allocas: Vec::new(),
            next_tmp: 0,
            next_slot: 0,
            next_block_id: 0,
            local_slots: HashMap::new(),
            binder_names,
            tco_loop: None,
        }
    }

    pub fn temp_local(&mut self, prefix: &str) -> LlvmLocal {
        let id = self.next_tmp;
        self.next_tmp += 1;
        LlvmLocal(format!("{prefix}.{id}"))
    }

    pub fn new_slot(&mut self) -> LlvmLocal {
        let id = self.next_slot;
        self.next_slot += 1;
        LlvmLocal(format!("slot.{id}"))
    }

    pub fn new_block_label(&mut self, prefix: &str) -> LabelId {
        let id = self.next_block_id;
        self.next_block_id += 1;
        LabelId(format!("{prefix}.{id}"))
    }

    pub fn emit(&mut self, instr: LlvmInstr) {
        self.blocks[self.current_block].instrs.push(instr);
    }

    pub fn emit_entry_alloca(&mut self, instr: LlvmInstr) {
        self.entry_allocas.push(instr);
    }

    pub fn set_terminator(&mut self, term: LlvmTerminator) {
        self.blocks[self.current_block].terminator = Some(term);
    }

    pub fn current_block_label(&self) -> LabelId {
        self.blocks[self.current_block].label.clone()
    }

    pub fn current_block_open(&self) -> bool {
        self.blocks[self.current_block].terminator.is_none()
    }

    pub fn push_block(&mut self, label: LabelId) -> usize {
        self.blocks.push(FunctionBlock {
            label,
            instrs: Vec::new(),
            terminator: None,
        });
        self.blocks.len() - 1
    }

    pub fn switch_to_block(&mut self, idx: usize) {
        self.current_block = idx;
    }

    pub fn bind_local(&mut self, binder: CoreBinder, slot: LlvmLocal) {
        self.local_slots.insert(binder.id, slot);
        self.binder_names.insert(binder.id, binder.name);
    }

    pub fn finish(mut self) -> Result<LlvmFunction, CoreToLlvmError> {
        self.blocks[0].instrs.splice(0..0, self.entry_allocas);
        let blocks = self
            .blocks
            .into_iter()
            .map(FunctionBlock::into_llvm)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LlvmFunction {
            linkage: Linkage::Internal,
            name: self.symbol,
            sig: LlvmFunctionSig {
                ret: self.ret_ty,
                params: self.llvm_param_types,
                varargs: false,
                call_conv: self.call_conv,
            },
            params: self.llvm_params,
            attrs: vec![],
            blocks,
        })
    }
}

pub(super) fn const_i32_operand(value: i32) -> LlvmOperand {
    LlvmOperand::Const(crate::core_to_llvm::LlvmConst::Int {
        bits: 32,
        value: value.into(),
    })
}

pub(super) fn closure_entry_function(
    symbol: GlobalId,
    top_level_symbols: HashMap<CoreBinderId, GlobalId>,
    interner: Option<&Interner>,
) -> FunctionState<'_> {
    FunctionState::new_closure_entry(symbol, top_level_symbols, interner)
}
