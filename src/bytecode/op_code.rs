use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    OpConstant = 0,
    OpAdd = 1,
    OpSub = 2,
    OpMul = 3,
    OpDiv = 4,
    OpMod = 5,
    OpEqual = 6,
    OpNotEqual = 7,
    OpGreaterThan = 8,
    OpLessThanOrEqual = 9,
    OpGreaterThanOrEqual = 10,
    OpMinus = 11,
    OpBang = 12,
    OpTrue = 13,
    OpFalse = 14,
    OpJump = 15,
    OpJumpNotTruthy = 16,
    OpPop = 17,
    OpGetGlobal = 18,
    OpSetGlobal = 19,
    OpGetLocal = 20,
    OpSetLocal = 21,
    OpCall = 22,
    OpReturnValue = 23,
    OpReturn = 24,
    OpClosure = 25,
    OpGetFree = 26,
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
    OpJumpTruthy = 37,
    OpLeft = 38,
    OpRight = 39,
    OpIsLeft = 40,
    OpIsRight = 41,
    OpUnwrapLeft = 42,
    OpUnwrapRight = 43,
}

impl From<u8> for OpCode {
    fn from(byte: u8) -> Self {
        match byte {
            0 => OpCode::OpConstant,
            1 => OpCode::OpAdd,
            2 => OpCode::OpSub,
            3 => OpCode::OpMul,
            4 => OpCode::OpDiv,
            5 => OpCode::OpMod,
            6 => OpCode::OpEqual,
            7 => OpCode::OpNotEqual,
            8 => OpCode::OpGreaterThan,
            9 => OpCode::OpLessThanOrEqual,
            10 => OpCode::OpGreaterThanOrEqual,
            11 => OpCode::OpMinus,
            12 => OpCode::OpBang,
            13 => OpCode::OpTrue,
            14 => OpCode::OpFalse,
            15 => OpCode::OpJump,
            16 => OpCode::OpJumpNotTruthy,
            17 => OpCode::OpPop,
            18 => OpCode::OpGetGlobal,
            19 => OpCode::OpSetGlobal,
            20 => OpCode::OpGetLocal,
            21 => OpCode::OpSetLocal,
            22 => OpCode::OpCall,
            23 => OpCode::OpReturnValue,
            24 => OpCode::OpReturn,
            25 => OpCode::OpClosure,
            26 => OpCode::OpGetFree,
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
            37 => OpCode::OpJumpTruthy,
            38 => OpCode::OpLeft,
            39 => OpCode::OpRight,
            40 => OpCode::OpIsLeft,
            41 => OpCode::OpIsRight,
            42 => OpCode::OpUnwrapLeft,
            43 => OpCode::OpUnwrapRight,
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
        | OpCode::OpJumpTruthy
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
