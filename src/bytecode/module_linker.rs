use std::{collections::HashMap, rc::Rc};

use crate::{
    bytecode::{
        bytecode::Bytecode,
        bytecode_cache::module_cache::{
            CachedModuleBinding, CachedModuleBindingKind, CachedModuleBytecode,
        },
        debug_info::{EffectSummary, FunctionDebugInfo, InstructionLocation, Location},
        op_code::{Instructions, OpCode, make, operand_widths, read_u16, read_u32},
        symbol_table::SymbolTable,
    },
    runtime::value::Value,
    syntax::interner::Interner,
};

pub struct LinkedVmProgram {
    pub bytecode: Bytecode,
    pub symbol_table: SymbolTable,
}

pub struct VmAssemblyContext {
    interner: Interner,
    symbol_table: SymbolTable,
    global_indices: HashMap<String, usize>,
    constants: Vec<Value>,
    instructions: Instructions,
    debug_info: FunctionDebugInfo,
}

impl VmAssemblyContext {
    pub fn new(interner: Interner) -> Self {
        Self {
            interner,
            symbol_table: SymbolTable::new(),
            global_indices: HashMap::new(),
            constants: Vec::new(),
            instructions: Vec::new(),
            debug_info: FunctionDebugInfo::default(),
        }
    }

    pub fn assemble_module(&mut self, artifact: &CachedModuleBytecode) -> Result<(), String> {
        let global_map = self.global_map_for(artifact)?;
        let constant_base = self.constants.len();

        let mut constants = artifact.constants.clone();
        for value in &mut constants {
            patch_value(value, constant_base, &global_map)?;
        }

        let mut instructions = artifact.instructions.clone();
        patch_instructions(&mut instructions, constant_base, &global_map)?;

        let instruction_base = self.instructions.len();
        self.instructions.extend(instructions);
        self.constants.extend(constants);
        merge_debug_info(&mut self.debug_info, &artifact.debug_info, instruction_base);
        Ok(())
    }

    pub fn finish(self) -> LinkedVmProgram {
        LinkedVmProgram {
            bytecode: Bytecode {
                instructions: self.instructions,
                constants: self.constants,
                debug_info: Some(self.debug_info),
            },
            symbol_table: self.symbol_table,
        }
    }

    fn global_map_for(
        &mut self,
        artifact: &CachedModuleBytecode,
    ) -> Result<HashMap<usize, usize>, String> {
        let mut globals = artifact.globals.clone();
        globals.sort_by_key(|binding| binding.index);

        let mut mapping = HashMap::with_capacity(globals.len());
        for binding in globals {
            let final_index = match binding.kind {
                CachedModuleBindingKind::Defined => {
                    if let Some(existing) = self.global_indices.get(&binding.name) {
                        *existing
                    } else {
                        self.define_global(&binding)
                    }
                }
                CachedModuleBindingKind::Imported => self
                    .global_indices
                    .get(&binding.name)
                    .copied()
                    .ok_or_else(|| format!("missing imported global {}", binding.name))?,
            };
            mapping.insert(binding.index, final_index);
        }
        Ok(mapping)
    }

    fn define_global(&mut self, binding: &CachedModuleBinding) -> usize {
        let symbol = self.interner.intern(&binding.name);
        let defined = self.symbol_table.define_global_with_index(
            symbol,
            self.symbol_table.num_definitions,
            binding.span,
            binding.is_assigned,
        );
        self.global_indices
            .insert(binding.name.clone(), defined.index);
        defined.index
    }
}

fn patch_value(
    value: &mut Value,
    constant_base: usize,
    global_map: &HashMap<usize, usize>,
) -> Result<(), String> {
    if let Value::Function(function) = value {
        let mut compiled = (**function).clone();
        patch_instructions(&mut compiled.instructions, constant_base, global_map)?;
        *value = Value::Function(Rc::new(compiled));
    }
    Ok(())
}

fn patch_instructions(
    instructions: &mut Instructions,
    constant_base: usize,
    global_map: &HashMap<usize, usize>,
) -> Result<(), String> {
    let mut ip = 0usize;
    while ip < instructions.len() {
        let op = OpCode::from(instructions[ip]);
        match op {
            OpCode::OpGetGlobal | OpCode::OpSetGlobal => {
                let local = read_u16(instructions, ip + 1) as usize;
                let global = *global_map
                    .get(&local)
                    .ok_or_else(|| format!("missing global mapping for local index {local}"))?;
                write_operand(instructions, ip, op, &[global]);
            }
            OpCode::OpConstant
            | OpCode::OpConstantLong
            | OpCode::OpClosure
            | OpCode::OpClosureLong
            | OpCode::OpMakeAdt
            | OpCode::OpIsAdt
            | OpCode::OpIsAdtJump
            | OpCode::OpIsAdtJumpLocal
            | OpCode::OpHandle
            | OpCode::OpHandleDirect
            | OpCode::OpPerform
            | OpCode::OpPerformDirect => {
                patch_constant_operand(instructions, ip, op, constant_base)?;
            }
            _ => {}
        }
        ip += 1 + operand_widths(op).iter().sum::<usize>();
    }
    Ok(())
}

fn patch_constant_operand(
    instructions: &mut Instructions,
    ip: usize,
    op: OpCode,
    constant_base: usize,
) -> Result<(), String> {
    match op {
        OpCode::OpConstant => {
            let idx = read_u16(instructions, ip + 1) as usize + constant_base;
            let _ = u16::try_from(idx)
                .map_err(|_| format!("constant index overflow for {op:?}: {idx}"))?;
            replace_instruction(instructions, ip, op, &make(OpCode::OpConstant, &[idx]));
        }
        OpCode::OpConstantLong => {
            let idx = read_u32(instructions, ip + 1) as usize + constant_base;
            let replacement = make(OpCode::OpConstantLong, &[idx]);
            replace_instruction(instructions, ip, op, &replacement);
        }
        OpCode::OpClosure => {
            let idx = read_u16(instructions, ip + 1) as usize + constant_base;
            let num_free = instructions[ip + 3] as usize;
            let _ = u16::try_from(idx)
                .map_err(|_| format!("constant index overflow for {op:?}: {idx}"))?;
            replace_instruction(
                instructions,
                ip,
                op,
                &make(OpCode::OpClosure, &[idx, num_free]),
            );
        }
        OpCode::OpClosureLong => {
            let idx = read_u32(instructions, ip + 1) as usize + constant_base;
            let num_free = instructions[ip + 5] as usize;
            let replacement = make(OpCode::OpClosureLong, &[idx, num_free]);
            replace_instruction(instructions, ip, op, &replacement);
        }
        OpCode::OpMakeAdt => {
            let idx = read_u16(instructions, ip + 1) as usize + constant_base;
            let arity = instructions[ip + 3] as usize;
            replace_instruction(instructions, ip, op, &make(op, &[idx, arity]));
        }
        OpCode::OpIsAdt => {
            let idx = read_u16(instructions, ip + 1) as usize + constant_base;
            replace_instruction(instructions, ip, op, &make(op, &[idx]));
        }
        OpCode::OpIsAdtJump => {
            let idx = read_u16(instructions, ip + 1) as usize + constant_base;
            let target = read_u16(instructions, ip + 3) as usize;
            replace_instruction(instructions, ip, op, &make(op, &[idx, target]));
        }
        OpCode::OpIsAdtJumpLocal => {
            let local = instructions[ip + 1] as usize;
            let idx = read_u16(instructions, ip + 2) as usize + constant_base;
            let target = read_u16(instructions, ip + 4) as usize;
            replace_instruction(instructions, ip, op, &make(op, &[local, idx, target]));
        }
        OpCode::OpHandle | OpCode::OpHandleDirect => {
            let idx = instructions[ip + 1] as usize + constant_base;
            let idx = u8::try_from(idx)
                .map_err(|_| format!("constant index overflow for {op:?}: {idx}"))?;
            replace_instruction(instructions, ip, op, &make(op, &[idx as usize]));
        }
        OpCode::OpPerform | OpCode::OpPerformDirect => {
            let idx = instructions[ip + 1] as usize + constant_base;
            let idx = u8::try_from(idx)
                .map_err(|_| format!("constant index overflow for {op:?}: {idx}"))?;
            let arity = instructions[ip + 2] as usize;
            replace_instruction(instructions, ip, op, &make(op, &[idx as usize, arity]));
        }
        _ => {}
    }
    Ok(())
}

fn replace_instruction(
    instructions: &mut Instructions,
    pos: usize,
    op: OpCode,
    replacement: &Instructions,
) {
    let original_len = 1 + operand_widths(op).iter().sum::<usize>();
    debug_assert_eq!(replacement.len(), original_len);
    instructions[pos..pos + original_len].copy_from_slice(replacement);
}

fn write_operand(instructions: &mut Instructions, pos: usize, op: OpCode, operands: &[usize]) {
    let replacement = make(op, operands);
    replace_instruction(instructions, pos, op, &replacement);
}

fn merge_debug_info(
    target: &mut FunctionDebugInfo,
    debug_info: &FunctionDebugInfo,
    instruction_base: usize,
) {
    let mut file_id_map = HashMap::new();
    for (source_id, file) in debug_info.files.iter().enumerate() {
        let target_id = ensure_file(target, file) as u32;
        file_id_map.insert(source_id as u32, target_id);
    }

    for location in &debug_info.locations {
        let remapped = location.location.as_ref().map(|entry| Location {
            file_id: file_id_map
                .get(&entry.file_id)
                .copied()
                .unwrap_or(entry.file_id),
            span: entry.span,
        });
        target.locations.push(InstructionLocation {
            offset: instruction_base + location.offset,
            location: remapped,
        });
    }

    target.effect_summary = merge_effect_summary(target.effect_summary, debug_info.effect_summary);
    if target.name.is_none() {
        target.name = debug_info.name.clone();
    }
}

fn ensure_file(debug_info: &mut FunctionDebugInfo, file: &str) -> usize {
    if let Some((index, _)) = debug_info
        .files
        .iter()
        .enumerate()
        .find(|(_, existing)| existing.as_str() == file)
    {
        index
    } else {
        debug_info.files.push(file.to_string());
        debug_info.files.len() - 1
    }
}

fn merge_effect_summary(left: EffectSummary, right: EffectSummary) -> EffectSummary {
    match (left, right) {
        (EffectSummary::HasEffects, _) | (_, EffectSummary::HasEffects) => {
            EffectSummary::HasEffects
        }
        (EffectSummary::Unknown, _) | (_, EffectSummary::Unknown) => EffectSummary::Unknown,
        _ => EffectSummary::Pure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bytecode::bytecode_cache::module_cache::CachedModuleBindingKind,
        diagnostics::position::{Position, Span},
    };

    #[test]
    fn assembles_defined_and_imported_globals() {
        let mut interner = Interner::new();
        let dep = interner.intern("Dep.value");
        let mut linker = VmAssemblyContext::new(interner.clone());
        linker
            .symbol_table
            .define_global_with_index(dep, 0, Span::default(), true);
        linker.global_indices.insert("Dep.value".to_string(), 0);

        let artifact = CachedModuleBytecode {
            globals: vec![
                CachedModuleBinding {
                    name: "Dep.value".to_string(),
                    index: 0,
                    span: Span::default(),
                    is_assigned: true,
                    kind: CachedModuleBindingKind::Imported,
                },
                CachedModuleBinding {
                    name: "Main.answer".to_string(),
                    index: 1,
                    span: Span::new(Position::new(1, 0), Position::new(1, 6)),
                    is_assigned: true,
                    kind: CachedModuleBindingKind::Defined,
                },
            ],
            constants: vec![Value::Integer(42)],
            instructions: make(OpCode::OpConstant, &[0])
                .into_iter()
                .chain(make(OpCode::OpSetGlobal, &[1]))
                .collect(),
            debug_info: FunctionDebugInfo::default(),
        };

        linker.assemble_module(&artifact).expect("assemble module");
        let linked = linker.finish();
        assert_eq!(linked.symbol_table.num_definitions, 2);
        assert_eq!(linked.bytecode.constants.len(), 1);
    }
}
