use std::{collections::HashMap, fmt};

use crate::{
    core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram},
    core_to_llvm::{
        CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmFunction, LlvmFunctionSig, LlvmInstr,
        LlvmLocal, LlvmModule, LlvmTerminator, LlvmType, emit_prelude_and_arith,
    },
    syntax::{Identifier, interner::Interner},
};

use super::expr::FunctionLowering;

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

pub fn compile_program(core: &CoreProgram) -> Result<LlvmModule, CoreToLlvmError> {
    compile_program_with_interner(core, None)
}

pub fn compile_program_with_interner(
    core: &CoreProgram,
    interner: Option<&Interner>,
) -> Result<LlvmModule, CoreToLlvmError> {
    let mut module = LlvmModule::new();
    emit_prelude_and_arith(&mut module);

    let mut symbols = HashMap::new();
    for def in &core.defs {
        let CoreExpr::Lam { .. } = &def.expr else {
            return Err(CoreToLlvmError::Unsupported {
                feature: "top-level value definitions",
                context: format!(
                    "definition `{}` is not a lambda",
                    display_ident(def.name, interner)
                ),
            });
        };
        symbols.insert(
            def.binder.id,
            GlobalId(sanitize_symbol_name(def.name, interner)),
        );
    }

    for def in &core.defs {
        let symbol =
            symbols
                .get(&def.binder.id)
                .cloned()
                .ok_or_else(|| CoreToLlvmError::MissingSymbol {
                    message: format!(
                        "missing top-level symbol for `{}`",
                        display_ident(def.name, interner)
                    ),
                })?;
        let function = lower_top_level_function(def, symbol, &symbols, interner)?;
        module.functions.push(function);
    }

    Ok(module)
}

pub(super) fn display_ident(ident: Identifier, interner: Option<&Interner>) -> String {
    interner
        .map(|it| it.resolve(ident).to_string())
        .unwrap_or_else(|| ident.to_string())
}

pub(super) fn sanitize_symbol_name(ident: Identifier, interner: Option<&Interner>) -> String {
    let raw = display_ident(ident, interner);
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
    symbols: &HashMap<CoreBinderId, GlobalId>,
    interner: Option<&Interner>,
) -> Result<LlvmFunction, CoreToLlvmError> {
    let CoreExpr::Lam { params, body, .. } = &def.expr else {
        return Err(CoreToLlvmError::Malformed {
            message: format!(
                "top-level function `{}` was not lowered as Lam",
                display_ident(def.name, interner)
            ),
        });
    };

    let mut lowering = FunctionLowering::new(symbol.clone(), params, symbols, interner);
    let result = lowering.lower_expr(body)?;
    lowering.finish_with_return(result)
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
    pub top_level_symbols: &'a HashMap<CoreBinderId, GlobalId>,
    pub param_bindings: Vec<(CoreBinder, LlvmLocal)>,
    pub blocks: Vec<FunctionBlock>,
    pub current_block: usize,
    pub entry_allocas: Vec<LlvmInstr>,
    pub next_tmp: u32,
    pub next_slot: u32,
    pub next_block_id: u32,
    pub local_slots: HashMap<CoreBinderId, LlvmLocal>,
}

impl<'a> FunctionState<'a> {
    pub fn new(
        symbol: GlobalId,
        params: &[CoreBinder],
        top_level_symbols: &'a HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a Interner>,
    ) -> Self {
        let entry = FunctionBlock {
            label: LabelId("entry".into()),
            instrs: Vec::new(),
            terminator: None,
        };
        let param_bindings = params
            .iter()
            .enumerate()
            .map(|(idx, binder)| {
                (
                    CoreBinder::new(binder.id, binder.name),
                    LlvmLocal(format!("arg{idx}")),
                )
            })
            .collect();
        Self {
            symbol,
            interner,
            top_level_symbols,
            param_bindings,
            blocks: vec![entry],
            current_block: 0,
            entry_allocas: Vec::new(),
            next_tmp: 0,
            next_slot: 0,
            next_block_id: 0,
            local_slots: HashMap::new(),
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

    pub fn bind_slot(&mut self, binder: CoreBinderId, slot: LlvmLocal) {
        self.local_slots.insert(binder, slot);
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
                ret: LlvmType::i64(),
                params: self
                    .param_bindings
                    .iter()
                    .map(|_| LlvmType::i64())
                    .collect(),
                varargs: false,
                call_conv: CallConv::Fastcc,
            },
            params: self
                .param_bindings
                .into_iter()
                .map(|(_, local)| local)
                .collect(),
            attrs: vec![],
            blocks,
        })
    }
}
