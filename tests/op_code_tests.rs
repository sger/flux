#[cfg(test)]
mod tests {
    use flux::bytecode::op_code::Instructions;
    use flux::bytecode::op_code::OpCode;
    use flux::bytecode::op_code::disassemble;
    use flux::bytecode::op_code::make;
    use flux::bytecode::op_code::operand_widths;
    use flux::bytecode::op_code::read_u8;
    use flux::bytecode::op_code::read_u16;
    use flux::bytecode::op_code::read_u32;

    #[test]
    fn test_make() {
        let instruction = make(OpCode::OpConstant, &[65534]);
        assert_eq!(instruction, vec![0u8, 255, 254]);
    }

    #[test]
    fn make_no_operands() {
        let ins = make(OpCode::OpAdd, &[]);
        assert_eq!(ins, vec![OpCode::OpAdd as u8]);

        let ins = make(OpCode::OpPop, &[]);
        assert_eq!(ins, vec![OpCode::OpPop as u8]);

        let ins = make(OpCode::OpConsumeLocal0, &[]);
        assert_eq!(ins, vec![OpCode::OpConsumeLocal0 as u8]);
    }

    #[test]
    fn make_u16_operand() {
        // OpConstant has one 2-byte operand
        let ins = make(OpCode::OpConstant, &[655]); // 0x028F
        assert_eq!(ins, vec![OpCode::OpConstant as u8, 0x02, 0x8F]);

        // OpJump has one 2-byte operand
        let ins = make(OpCode::OpJump, &[1024]); // 0x0400
        assert_eq!(ins, vec![OpCode::OpJump as u8, 0x04, 0x00]);

        let ins = make(OpCode::OpCmpEqJumpNotTruthy, &[1024]);
        assert_eq!(ins, vec![OpCode::OpCmpEqJumpNotTruthy as u8, 0x04, 0x00]);

        let ins = make(OpCode::OpConstantAdd, &[1024]);
        assert_eq!(ins, vec![OpCode::OpConstantAdd as u8, 0x04, 0x00]);
    }

    #[test]
    fn make_u8_operand() {
        // OpGetLocal has one 1-byte operand
        let ins = make(OpCode::OpGetLocal, &[7]);
        assert_eq!(ins, vec![OpCode::OpGetLocal as u8, 7]);

        // OpCall has one 1-byte operand
        let ins = make(OpCode::OpCall, &[2]);
        assert_eq!(ins, vec![OpCode::OpCall as u8, 2]);

        let ins = make(OpCode::OpCallSelf, &[2]);
        assert_eq!(ins, vec![OpCode::OpCallSelf as u8, 2]);

        let ins = make(OpCode::OpAetherDropLocal, &[7]);
        assert_eq!(ins, vec![OpCode::OpAetherDropLocal as u8, 7]);

        let ins = make(OpCode::OpGetLocalCall1, &[4]);
        assert_eq!(ins, vec![OpCode::OpGetLocalCall1 as u8, 4]);

        let ins = make(OpCode::OpGetLocalIndex, &[2]);
        assert_eq!(ins, vec![OpCode::OpGetLocalIndex as u8, 2]);

        let ins = make(OpCode::OpSetLocalPop, &[9]);
        assert_eq!(ins, vec![OpCode::OpSetLocalPop as u8, 9]);
    }

    #[test]
    fn make_multi_operands() {
        // OpClosure has operands [u16, u8]
        // constant index 1, free vars 2
        let ins = make(OpCode::OpClosure, &[1, 2]);
        assert_eq!(ins, vec![OpCode::OpClosure as u8, 0x00, 0x01, 0x02]);

        let ins = make(OpCode::OpAddLocals, &[1, 3]);
        assert_eq!(ins, vec![OpCode::OpAddLocals as u8, 1, 3]);

        let ins = make(OpCode::OpSubLocals, &[2, 4]);
        assert_eq!(ins, vec![OpCode::OpSubLocals as u8, 2, 4]);

        let ins = make(OpCode::OpGetLocalGetLocal, &[5, 6]);
        assert_eq!(ins, vec![OpCode::OpGetLocalGetLocal as u8, 5, 6]);

        let ins = make(OpCode::OpGetLocalIsAdt, &[7, 258]);
        assert_eq!(ins, vec![OpCode::OpGetLocalIsAdt as u8, 7, 0x01, 0x02]);
    }

    #[test]
    fn make_op_call_base_operands() {
        let ins = make(OpCode::OpCallBase, &[9, 2]);
        assert_eq!(ins, vec![OpCode::OpCallBase as u8, 9, 2]);
    }

    #[test]
    fn read_helpers() {
        let bytes = vec![0xAA, 0x01, 0x02, 0x03, 0x04, 0xFF];

        assert_eq!(read_u8(&bytes, 0), 0xAA);
        assert_eq!(read_u16(&bytes, 1), 0x0102);
        assert_eq!(read_u32(&bytes, 1), 0x01020304);
        assert_eq!(read_u8(&bytes, 5), 0xFF);
    }

    #[test]
    fn make_u32_operand() {
        let ins = make(OpCode::OpConstantLong, &[70_000]);
        assert_eq!(
            ins,
            vec![OpCode::OpConstantLong as u8, 0x00, 0x01, 0x11, 0x70]
        );
    }

    #[test]
    fn make_multi_operands_with_u32_prefix() {
        let ins = make(OpCode::OpClosureLong, &[70_000, 2]);
        assert_eq!(
            ins,
            vec![OpCode::OpClosureLong as u8, 0x00, 0x01, 0x11, 0x70, 0x02]
        );
    }

    #[test]
    fn test_disassemble() {
        let mut instructions = Instructions::new();
        instructions.extend(make(OpCode::OpConstant, &[1]));
        instructions.extend(make(OpCode::OpAdd, &[]));

        let output = disassemble(&instructions);
        assert!(output.contains("OpConstant"));
        assert!(output.contains("OpAdd"));
    }

    #[test]
    fn disassemble_mixed_instructions() {
        let mut program: Instructions = Vec::new();

        // 0: OpConstant 655  => 3 bytes total
        program.extend(make(OpCode::OpConstant, &[655]));

        // 3: OpAdd          => 1 byte total
        program.extend(make(OpCode::OpAdd, &[]));

        // 4: OpClosure 1 2  => 4 bytes total
        program.extend(make(OpCode::OpClosure, &[1, 2]));

        let out = disassemble(&program);

        // Note: current disassemble() prints a trailing space for no-operand ops.
        let expected = concat!(
            "0000 OpConstant 655\n",
            "0003 OpAdd \n",
            "0004 OpClosure 1 2\n",
        );

        assert_eq!(out, expected);
    }

    #[test]
    fn disassemble_superinstructions() {
        let mut program: Instructions = Vec::new();
        program.extend(make(OpCode::OpAddLocals, &[0, 1]));
        program.extend(make(OpCode::OpCall1, &[]));
        program.extend(make(OpCode::OpTailCall1, &[]));

        let out = disassemble(&program);

        assert!(out.contains("OpAddLocals 0 1"));
        assert!(out.contains("OpCall1"));
        assert!(out.contains("OpTailCall1"));
    }

    #[test]
    fn operand_widths_contract() {
        assert_eq!(operand_widths(OpCode::OpConstant), vec![2]);
        assert_eq!(operand_widths(OpCode::OpConstantLong), vec![4]);
        assert_eq!(operand_widths(OpCode::OpGetLocal), vec![1]);
        assert_eq!(operand_widths(OpCode::OpCallSelf), vec![1]);
        assert_eq!(operand_widths(OpCode::OpCmpEqJumpNotTruthy), vec![2]);
        assert_eq!(operand_widths(OpCode::OpCmpNeJumpNotTruthy), vec![2]);
        assert_eq!(operand_widths(OpCode::OpCmpGtJumpNotTruthy), vec![2]);
        assert_eq!(operand_widths(OpCode::OpCmpLeJumpNotTruthy), vec![2]);
        assert_eq!(operand_widths(OpCode::OpCmpGeJumpNotTruthy), vec![2]);
        assert_eq!(operand_widths(OpCode::OpClosure), vec![2, 1]);
        assert_eq!(operand_widths(OpCode::OpClosureLong), vec![4, 1]);
        assert_eq!(operand_widths(OpCode::OpArrayLong), vec![4]);
        assert_eq!(operand_widths(OpCode::OpHashLong), vec![4]);
        assert_eq!(operand_widths(OpCode::OpTuple), vec![2]);
        assert_eq!(operand_widths(OpCode::OpTupleLong), vec![4]);
        assert_eq!(operand_widths(OpCode::OpTupleIndex), vec![1]);
        assert_eq!(operand_widths(OpCode::OpAetherDropLocal), vec![1]);
        assert_eq!(operand_widths(OpCode::OpAddLocals), vec![1, 1]);
        assert_eq!(operand_widths(OpCode::OpSubLocals), vec![1, 1]);
        assert_eq!(operand_widths(OpCode::OpGetLocalCall1), vec![1]);
        assert_eq!(operand_widths(OpCode::OpConstantAdd), vec![2]);
        assert_eq!(operand_widths(OpCode::OpGetLocalIndex), vec![1]);
        assert_eq!(operand_widths(OpCode::OpGetLocalIsAdt), vec![1, 2]);
        assert_eq!(operand_widths(OpCode::OpSetLocalPop), vec![1]);
        assert_eq!(operand_widths(OpCode::OpGetLocalGetLocal), vec![1, 1]);
        assert_eq!(operand_widths(OpCode::OpCall0), Vec::<usize>::new());
        assert_eq!(operand_widths(OpCode::OpCall1), Vec::<usize>::new());
        assert_eq!(operand_widths(OpCode::OpCall2), Vec::<usize>::new());
        assert_eq!(operand_widths(OpCode::OpTailCall1), Vec::<usize>::new());
        assert_eq!(operand_widths(OpCode::OpConsumeLocal0), Vec::<usize>::new());
        assert_eq!(operand_widths(OpCode::OpConsumeLocal1), Vec::<usize>::new());
        assert_eq!(operand_widths(OpCode::OpIsAdtJumpLocal), vec![1, 2, 2]);
        assert_eq!(operand_widths(OpCode::OpAdd), Vec::<usize>::new());
    }

    #[test]
    fn max_opcode_matches_last_variant() {
        // Ensure MAX_OPCODE stays in sync when new opcodes are added.
        assert_eq!(
            flux::bytecode::op_code::MAX_OPCODE,
            OpCode::OpTailCall1 as u8,
            "MAX_OPCODE must equal the last OpCode variant"
        );
    }

    #[test]
    fn from_u8_roundtrips_all_opcodes() {
        // Verify transmute-based From<u8> produces correct values for all opcodes.
        for byte in 0..=flux::bytecode::op_code::MAX_OPCODE {
            let op = OpCode::from(byte);
            assert_eq!(op as u8, byte, "OpCode::from({byte}) roundtrip failed");
        }
    }
}
