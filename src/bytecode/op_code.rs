use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    OpConstant = 0,
    OpAdd = 1,
    OpSub = 2,
    OpMul = 3,
    OpDiv = 4,
    OpEqual = 5,
    OpNotEqual = 6,
    OpGreaterThan = 7,
    OpMinus = 8,
    OpBang = 9,
    OpTrue = 10,
    OpFalse = 11,
    OpJump = 12,
    OpJumpNotTruthy = 13,
    OpPop = 14,
    OpGetGlobal = 15,
    OpSetGlobal = 16,
    OpGetLocal = 17,
    OpSetLocal = 18,
    OpCall = 19,
    OpReturnValue = 20,
    OpReturn = 21,
    OpClosure = 22,
    OpGetFree = 23,
    OpNull = 24,
    OpArray = 25,
    OpHash = 26,
    OpIndex = 27,
    OpGetBuiltin = 28,
    OpCurrentClosure = 29,
    OpNone = 30,
    OpSome = 31,
}

impl From<u8> for OpCode {
    fn from(byte: u8) -> Self {
        match byte {
            0 => OpCode::OpConstant,
            1 => OpCode::OpAdd,
            2 => OpCode::OpSub,
            3 => OpCode::OpMul,
            4 => OpCode::OpDiv,
            5 => OpCode::OpEqual,
            6 => OpCode::OpNotEqual,
            7 => OpCode::OpGreaterThan,
            8 => OpCode::OpMinus,
            9 => OpCode::OpBang,
            10 => OpCode::OpTrue,
            11 => OpCode::OpFalse,
            12 => OpCode::OpJump,
            13 => OpCode::OpJumpNotTruthy,
            14 => OpCode::OpPop,
            15 => OpCode::OpGetGlobal,
            16 => OpCode::OpSetGlobal,
            17 => OpCode::OpGetLocal,
            18 => OpCode::OpSetLocal,
            19 => OpCode::OpCall,
            20 => OpCode::OpReturnValue,
            21 => OpCode::OpReturn,
            22 => OpCode::OpClosure,
            23 => OpCode::OpGetFree,
            24 => OpCode::OpNull,
            25 => OpCode::OpArray,
            26 => OpCode::OpHash,
            27 => OpCode::OpIndex,
            28 => OpCode::OpGetBuiltin,
            29 => OpCode::OpCurrentClosure,
            30 => OpCode::OpNone,
            31 => OpCode::OpSome,
            _ => panic!("Unknown opcode {}", byte),
        }
    }
}

impl fmt::Display for OpCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub fn operand_widths(op: OpCode) -> Vec<usize> {
    match op {
        OpCode::OpConstant
        | OpCode::OpJump
        | OpCode::OpJumpNotTruthy
        | OpCode::OpGetGlobal
        | OpCode::OpSetGlobal
        | OpCode::OpArray
        | OpCode::OpHash => vec![2],
        OpCode::OpGetLocal
        | OpCode::OpSetLocal
        | OpCode::OpCall
        | OpCode::OpGetFree
        | OpCode::OpGetBuiltin => vec![1],
        OpCode::OpClosure => vec![2, 1],
        _ => vec![],
    }
}

pub type Instructions = Vec<u8>;

pub fn make(op: OpCode, operands: &[usize]) -> Instructions {
    let widths = operand_widths(op);
    let mut instruction = vec![op as u8];

    for (i, operand) in operands.iter().enumerate() {
        let width = widths.get(i).copied().unwrap_or(0);
        match width {
            1 => instruction.push(*operand as u8),
            2 => {
                instruction.push((*operand >> 8) as u8);
                instruction.push(*operand as u8);
            }
            _ => {}
        }
    }

    instruction
}

pub fn read_u16(instructions: &[u8], offset: usize) -> u16 {
    ((instructions[offset] as u16) << 8) | (instructions[offset + 1] as u16)
}

pub fn read_u8(instructions: &[u8], offset: usize) -> u8 {
    instructions[offset]
}

pub fn disassemble(instructions: &Instructions) -> String {
    let mut result = String::new();
    let mut i = 0;

    while i < instructions.len() {
        let op = OpCode::from(instructions[i]);
        let widths = operand_widths(op);

        let mut operands = Vec::new();
        let mut offset = i + 1;

        for width in widths {
            match width {
                1 => {
                    operands.push(read_u8(instructions, offset) as usize);
                    offset += 1;
                }
                2 => {
                    operands.push(read_u16(instructions, offset) as usize);
                    offset += 2;
                }
                _ => {}
            }
        }

        let operand_str = operands
            .iter()
            .map(|o| o.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        result.push_str(&format!("{:04} {} {}\n", i, op, operand_str));
        i = offset;
    }

    result
}
