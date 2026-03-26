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
    OpGetBase = 30,
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
    OpTailCall = 44,
    OpConsumeLocal = 45,
    OpConstantLong = 46,
    OpClosureLong = 47,
    OpArrayLong = 48,
    OpHashLong = 49,
    OpCons = 50,
    OpIsCons = 51,
    OpIsEmptyList = 52,
    OpConsHead = 53,
    OpConsTail = 54,
    OpGetLocal0 = 55,
    OpGetLocal1 = 56,
    /// Superinstruction: fuses OpGetLocal(n) + OpReturnValue.
    /// Operand: 1-byte local index.
    OpReturnLocal = 57,
    /// Build a tuple from N stack values (u16 count).
    OpTuple = 58,
    /// Build a tuple from N stack values (u32 count).
    OpTupleLong = 59,
    /// Direct tuple field access by constant index (u8).
    OpTupleIndex = 60,
    /// Pushes whether top-of-stack value is a tuple.
    OpIsTuple = 61,
    /// Generic primop dispatch: operands are `[primop_id: u8, arity: u8]`.
    /// Consumes `arity` arguments from the stack and pushes one result.
    OpPrimOp = 62,
    /// Direct built-in function call: operands are `[base_fn_index: u8, arity: u8]`.
    /// Unlike `OpCall`, no callee value is read from the stack.
    /// Consumes `arity` arguments from the stack and pushes one result.
    OpCallBase = 63,
    /// Construct a user-defined ADT value.
    /// Operands: `[const_idx: u16, arity: u8]`.
    /// Pops `arity` values (the constructor fields), then pushes `Value::Adt { constructor, fields }`.
    /// `constants[const_idx]` must be a `Value::String` containing the constructor name.
    OpMakeAdt = 64,
    /// Test whether top-of-stack is a `Value::Adt` with a specific constructor name.
    /// Operand: `[const_idx: u16]`. Replaces top-of-stack with a boolean.
    OpIsAdt = 65,
    /// Extract a field from a `Value::Adt` by index.
    /// Operand: `[field_idx: u8]`. Replaces top-of-stack with `fields[field_idx]`.
    OpAdtField = 66,
    /// Install a handler for an effect.
    /// Operand: `[const_idx: u8]` — index of a `Value::HandlerDescriptor` in constants.
    /// Pushes a `HandlerFrame` onto the handler stack and falls through to the handled expression.
    OpHandle = 67,
    /// Remove the innermost handler frame.
    /// No operands. Pops one `HandlerFrame` from the handler stack.
    OpEndHandle = 68,
    /// Perform an effect operation (suspends the current computation).
    /// Operands: `[const_idx: u8, arity: u8]`.
    /// `constants[const_idx]` is a `Value::PerformDescriptor`.
    /// Pops `arity` arguments from the stack, searches handler_stack for a matching handler,
    /// captures a continuation, and calls the matching handler arm.
    OpPerform = 69,
    /// Call the current frame's closure directly with `arity` arguments.
    /// Unlike `OpCall`, no callee value is read from the stack.
    OpCallSelf = 70,
    /// Consume local 0 by moving it out of the current frame slot.
    OpConsumeLocal0 = 71,
    /// Consume local 1 by moving it out of the current frame slot.
    OpConsumeLocal1 = 72,
    /// Compare top two stack values with `==` and jump if the result is falsy.
    OpCmpEqJumpNotTruthy = 73,
    /// Compare top two stack values with `!=` and jump if the result is falsy.
    OpCmpNeJumpNotTruthy = 74,
    /// Compare top two stack values with `>` and jump if the result is falsy.
    OpCmpGtJumpNotTruthy = 75,
    /// Compare top two stack values with `<=` and jump if the result is falsy.
    OpCmpLeJumpNotTruthy = 76,
    /// Compare top two stack values with `>=` and jump if the result is falsy.
    OpCmpGeJumpNotTruthy = 77,
    /// Peek at TOS; if it is a `Value::Adt` or `Value::AdtUnit` with a matching constructor,
    /// fall through (ADT stays on stack). Otherwise jump to `jump_offset` (ADT stays on stack).
    /// Operands: `[const_idx: u16, jump_offset: u16]`.
    /// `constants[const_idx]` must be a `Value::String` containing the constructor name.
    OpIsAdtJump = 78,
    /// Pop TOS (must be `Value::Adt` with exactly 2 fields), push `fields[0]` then `fields[1]`.
    /// No operands.
    OpAdtFields2 = 79,
    /// Peek at a local slot (no stack push); if it is a matching ADT constructor, fall through.
    /// Otherwise jump to `jump_offset`. The local slot is always left unchanged — the matching
    /// arm must follow with `OpConsumeLocal` to move the value onto the stack before field
    /// extraction, keeping `Rc` strong_count == 1 so that `Rc::try_unwrap` succeeds in
    /// `OpAdtFields2` / `OpAdtField`.
    /// Operands: `[local_idx: u8, const_idx: u16, jump_offset: u16]`.
    OpIsAdtJumpLocal = 80,
    /// Install a tail-resumptive handler for an effect.
    /// Operand: `[const_idx: u8]` — index of a `Value::HandlerDescriptor`.
    /// Identical to `OpHandle` but marks the handler frame as `is_direct = true`
    /// so that `OpPerformDirect` skips continuation capture.
    OpHandleDirect = 81,
    /// Perform an effect operation on a tail-resumptive handler (no continuation).
    /// Operands: `[const_idx: u8, arity: u8]`.
    /// Like `OpPerform` but the matching handler arm is called directly — no
    /// continuation is captured and `resume(v)` simply returns `v`.
    OpPerformDirect = 82,
    /// Perform an effect operation with compile-time resolved handler.
    /// Operands: `[handler_depth: u8, arm_index: u8, arity: u8]`.
    /// Like `OpPerformDirect` but skips the handler stack search entirely.
    /// `handler_depth` is the distance from the top of the handler stack
    /// (0 = innermost handler). `arm_index` is the index into the handler's arms.
    OpPerformDirectIndexed = 83,

    // ── Aether reuse opcodes ────────────────────────────────────────────
    /// Aether: test if a value is uniquely owned (Rc::strong_count == 1).
    /// If unique, pushes the value as a reuse token. Otherwise pushes None.
    /// Operand: none (value on TOS).
    OpDropReuse = 84,
    /// Aether: construct a Cons cell, reusing the token's allocation if non-null.
    /// Pops token, head, tail from the stack. Operand: `[field_mask: u8]`.
    /// field_mask 0xFF = write all fields.
    OpReuseCons = 85,
    /// Aether: construct an ADT, reusing the token's allocation if non-null.
    /// Operands: `[const_idx: u16, arity: u8, field_mask: u8]`.
    /// Pops token then `arity` fields from the stack.
    OpReuseAdt = 86,
    /// Aether: construct Some, reusing token if non-null.
    /// Pops token and inner from the stack. No operands.
    OpReuseSome = 87,
    /// Aether: construct Left, reusing token if non-null.
    /// Pops token and inner from the stack. No operands.
    OpReuseLeft = 88,
    /// Aether: construct Right, reusing token if non-null.
    /// Pops token and inner from the stack. No operands.
    OpReuseRight = 89,
    /// Aether: test if TOS value's Rc is uniquely owned (strong_count == 1).
    /// Pushes boolean result. No operands.
    OpIsUnique = 90,
    /// Aether: drop a local slot early if it currently holds a boxed value.
    /// Operand: `[local_idx: u8]`.
    /// Immediate `Int`/`Float`/`Bool` locals are left unchanged.
    OpAetherDropLocal = 91,
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
            30 => OpCode::OpGetBase,
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
            44 => OpCode::OpTailCall,
            45 => OpCode::OpConsumeLocal,
            46 => OpCode::OpConstantLong,
            47 => OpCode::OpClosureLong,
            48 => OpCode::OpArrayLong,
            49 => OpCode::OpHashLong,
            50 => OpCode::OpCons,
            51 => OpCode::OpIsCons,
            52 => OpCode::OpIsEmptyList,
            53 => OpCode::OpConsHead,
            54 => OpCode::OpConsTail,
            55 => OpCode::OpGetLocal0,
            56 => OpCode::OpGetLocal1,
            57 => OpCode::OpReturnLocal,
            58 => OpCode::OpTuple,
            59 => OpCode::OpTupleLong,
            60 => OpCode::OpTupleIndex,
            61 => OpCode::OpIsTuple,
            62 => OpCode::OpPrimOp,
            63 => OpCode::OpCallBase,
            64 => OpCode::OpMakeAdt,
            65 => OpCode::OpIsAdt,
            66 => OpCode::OpAdtField,
            67 => OpCode::OpHandle,
            68 => OpCode::OpEndHandle,
            69 => OpCode::OpPerform,
            70 => OpCode::OpCallSelf,
            71 => OpCode::OpConsumeLocal0,
            72 => OpCode::OpConsumeLocal1,
            73 => OpCode::OpCmpEqJumpNotTruthy,
            74 => OpCode::OpCmpNeJumpNotTruthy,
            75 => OpCode::OpCmpGtJumpNotTruthy,
            76 => OpCode::OpCmpLeJumpNotTruthy,
            77 => OpCode::OpCmpGeJumpNotTruthy,
            78 => OpCode::OpIsAdtJump,
            79 => OpCode::OpAdtFields2,
            80 => OpCode::OpIsAdtJumpLocal,
            81 => OpCode::OpHandleDirect,
            82 => OpCode::OpPerformDirect,
            83 => OpCode::OpPerformDirectIndexed,
            84 => OpCode::OpDropReuse,
            85 => OpCode::OpReuseCons,
            86 => OpCode::OpReuseAdt,
            87 => OpCode::OpReuseSome,
            88 => OpCode::OpReuseLeft,
            89 => OpCode::OpReuseRight,
            90 => OpCode::OpIsUnique,
            91 => OpCode::OpAetherDropLocal,
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
        | OpCode::OpCmpEqJumpNotTruthy
        | OpCode::OpCmpNeJumpNotTruthy
        | OpCode::OpCmpGtJumpNotTruthy
        | OpCode::OpCmpLeJumpNotTruthy
        | OpCode::OpCmpGeJumpNotTruthy
        | OpCode::OpGetGlobal
        | OpCode::OpSetGlobal
        | OpCode::OpArray
        | OpCode::OpHash
        | OpCode::OpTuple => vec![2],
        OpCode::OpConstantLong | OpCode::OpArrayLong | OpCode::OpHashLong | OpCode::OpTupleLong => {
            vec![4]
        }
        OpCode::OpGetLocal
        | OpCode::OpConsumeLocal
        | OpCode::OpSetLocal
        | OpCode::OpCall
        | OpCode::OpCallSelf
        | OpCode::OpTailCall
        | OpCode::OpGetFree
        | OpCode::OpGetBase
        | OpCode::OpReturnLocal
        | OpCode::OpTupleIndex
        | OpCode::OpAetherDropLocal => vec![1],
        OpCode::OpPrimOp | OpCode::OpCallBase => vec![1, 1],
        OpCode::OpClosure => vec![2, 1],
        OpCode::OpClosureLong => vec![4, 1],
        // ADT opcodes
        OpCode::OpMakeAdt => vec![2, 1], // const_idx: u16, arity: u8
        OpCode::OpIsAdt => vec![2],      // const_idx: u16
        OpCode::OpAdtField => vec![1],   // field_idx: u8
        OpCode::OpIsAdtJump => vec![2, 2], // const_idx: u16, jump_offset: u16
        OpCode::OpIsAdtJumpLocal => vec![1, 2, 2], // local_idx: u8, const_idx: u16, jump_offset: u16
        // OpAdtFields2: no operands, covered by _ => vec![]
        // Effect handler opcodes
        OpCode::OpHandle | OpCode::OpHandleDirect => vec![1], // const_idx: u8
        OpCode::OpEndHandle => vec![],                        // no operands
        OpCode::OpPerform | OpCode::OpPerformDirect => vec![1, 1], // const_idx: u8, arity: u8
        OpCode::OpPerformDirectIndexed => vec![1, 1, 1], // handler_depth: u8, arm_index: u8, arity: u8
        OpCode::OpConsumeLocal0 | OpCode::OpConsumeLocal1 => vec![],
        // Aether reuse opcodes
        OpCode::OpDropReuse => vec![],       // TOS consumed
        OpCode::OpReuseCons => vec![1],      // field_mask: u8
        OpCode::OpReuseAdt => vec![2, 1, 1], // const_idx: u16, arity: u8, field_mask: u8
        OpCode::OpReuseSome | OpCode::OpReuseLeft | OpCode::OpReuseRight => vec![],
        OpCode::OpIsUnique => vec![],
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
            4 => {
                instruction.push((*operand >> 24) as u8);
                instruction.push((*operand >> 16) as u8);
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

pub fn read_u32(instructions: &[u8], offset: usize) -> u32 {
    ((instructions[offset] as u32) << 24)
        | ((instructions[offset + 1] as u32) << 16)
        | ((instructions[offset + 2] as u32) << 8)
        | (instructions[offset + 3] as u32)
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
                4 => {
                    operands.push(read_u32(instructions, offset) as usize);
                    offset += 4;
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
