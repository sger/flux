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
    OpLessThanOrEqual = 8,
    OpGreaterThanOrEqual = 9,
    OpMinus = 10,
    OpBang = 11,
    OpTrue = 12,
    OpFalse = 13,
    OpJump = 14,
    OpJumpNotTruthy = 15,
    OpPop = 16,
    OpGetGlobal = 17,
    OpSetGlobal = 18,
    OpGetLocal = 19,
    OpSetLocal = 20,
    OpCall = 21,
    OpReturnValue = 22,
    OpReturn = 23,
    OpClosure = 24,
    OpGetFree = 25,
    OpNull = 26,
    OpArray = 27,
    OpHash = 28,
    OpIndex = 29,
    OpGetBuiltin = 30,
    OpCurrentClosure = 31,
    OpNone = 32,
    OpSome = 33,
    OpIsSome = 34,
    OpUnwrapSome = 35,
    OpToString = 36,
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
            8 => OpCode::OpLessThanOrEqual,
            9 => OpCode::OpGreaterThanOrEqual,
            10 => OpCode::OpMinus,
            11 => OpCode::OpBang,
            12 => OpCode::OpTrue,
            13 => OpCode::OpFalse,
            14 => OpCode::OpJump,
            15 => OpCode::OpJumpNotTruthy,
            16 => OpCode::OpPop,
            17 => OpCode::OpGetGlobal,
            18 => OpCode::OpSetGlobal,
            19 => OpCode::OpGetLocal,
            20 => OpCode::OpSetLocal,
            21 => OpCode::OpCall,
            22 => OpCode::OpReturnValue,
            23 => OpCode::OpReturn,
            24 => OpCode::OpClosure,
            25 => OpCode::OpGetFree,
            26 => OpCode::OpNull,
            27 => OpCode::OpArray,
            28 => OpCode::OpHash,
            29 => OpCode::OpIndex,
            30 => OpCode::OpGetBuiltin,
            31 => OpCode::OpCurrentClosure,
            32 => OpCode::OpNone,
            33 => OpCode::OpSome,
            34 => OpCode::OpIsSome,
            35 => OpCode::OpUnwrapSome,
            36 => OpCode::OpToString,
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
