use crate::{
    bytecode::{
        debug_info::{InstructionLocation, Location},
        emitted_instruction::EmittedInstruction,
        op_code::{Instructions, OpCode, make},
        binding::Binding,
        symbol_scope::SymbolScope,
    },
    frontend::position::Span,
    runtime::object::Object,
};

use super::Compiler;

impl Compiler {
    pub(super) fn emit(&mut self, op_code: OpCode, operands: &[usize]) -> usize {
        let instruction = make(op_code, operands);
        let pos = self.add_instruction(&instruction, self.current_span);
        self.set_last_instruction(op_code, pos);
        pos
    }

    fn add_instruction(&mut self, instruction: &[u8], span: Option<Span>) -> usize {
        let pos = self.scopes[self.scope_index].instructions.len();
        self.scopes[self.scope_index]
            .instructions
            .extend_from_slice(instruction);
        self.add_location(pos, span);
        pos
    }

    fn add_location(&mut self, offset: usize, span: Option<Span>) {
        let file_id = self.file_id_for_current();
        let location = span.map(|span| Location { file_id, span });
        self.scopes[self.scope_index]
            .locations
            .push(InstructionLocation { offset, location });
    }

    fn file_id_for_current(&mut self) -> u32 {
        let files = &mut self.scopes[self.scope_index].files;
        if let Some((index, _)) = files
            .iter()
            .enumerate()
            .find(|(_, file)| file.as_str() == self.file_path)
        {
            return index as u32;
        }
        files.push(self.file_path.clone());
        (files.len() - 1) as u32
    }

    fn set_last_instruction(&mut self, op_code: OpCode, pos: usize) {
        let previous = self.scopes[self.scope_index].last_instruction.clone();
        self.scopes[self.scope_index].previous_instruction = previous;
        self.scopes[self.scope_index].last_instruction = EmittedInstruction {
            opcode: Some(op_code),
            position: pos,
        };
    }

    pub(super) fn add_constant(&mut self, obj: Object) -> usize {
        self.constants.push(obj);
        self.constants.len() - 1
    }

    pub(super) fn load_symbol(&mut self, symbol: &Binding) {
        match symbol.symbol_scope {
            SymbolScope::Global => {
                self.emit(OpCode::OpGetGlobal, &[symbol.index]);
            }
            SymbolScope::Local => {
                self.emit(OpCode::OpGetLocal, &[symbol.index]);
            }
            SymbolScope::Builtin => {
                self.emit(OpCode::OpGetBuiltin, &[symbol.index]);
            }
            SymbolScope::Free => {
                self.emit(OpCode::OpGetFree, &[symbol.index]);
            }
            SymbolScope::Function => {
                self.emit(OpCode::OpCurrentClosure, &[]);
            }
        }
    }

    pub(super) fn is_last_instruction(&self, opcode: OpCode) -> bool {
        self.scopes[self.scope_index].last_instruction.opcode == Some(opcode)
    }

    pub(super) fn remove_last_pop(&mut self) {
        let last_pos = self.scopes[self.scope_index].last_instruction.position;
        let previous = self.scopes[self.scope_index].previous_instruction.clone();

        self.scopes[self.scope_index]
            .instructions
            .truncate(last_pos);
        while let Some(last) = self.scopes[self.scope_index].locations.last() {
            if last.offset >= last_pos {
                self.scopes[self.scope_index].locations.pop();
            } else {
                break;
            }
        }
        self.scopes[self.scope_index].last_instruction = previous;
    }

    pub(super) fn change_operand(&mut self, op_pos: usize, operand: usize) {
        let op_code = OpCode::from(self.current_instructions()[op_pos]);
        self.replace_instruction(op_pos, make(op_code, &[operand]));
    }

    pub(super) fn replace_instruction(&mut self, pos: usize, new_instruction: Instructions) {
        for (i, byte) in new_instruction.iter().enumerate() {
            self.scopes[self.scope_index].instructions[pos + i] = *byte;
        }
    }
}
