#[cfg(test)]
mod tests {
    use flux::bytecode::op_code::Instructions;
    use flux::bytecode::op_code::OpCode;
    use flux::bytecode::op_code::disassemble;
    use flux::bytecode::op_code::make;
    use flux::bytecode::op_code::operand_widths;
    use flux::bytecode::op_code::read_u8;
    use flux::bytecode::op_code::read_u16;

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
    }

    #[test]
    fn make_u16_operand() {
        // OpConstant has one 2-byte operand
        let ins = make(OpCode::OpConstant, &[655]); // 0x028F
        assert_eq!(ins, vec![OpCode::OpConstant as u8, 0x02, 0x8F]);

        // OpJump has one 2-byte operand
        let ins = make(OpCode::OpJump, &[1024]); // 0x0400
        assert_eq!(ins, vec![OpCode::OpJump as u8, 0x04, 0x00]);
    }

    #[test]
    fn make_u8_operand() {
        // OpGetLocal has one 1-byte operand
        let ins = make(OpCode::OpGetLocal, &[7]);
        assert_eq!(ins, vec![OpCode::OpGetLocal as u8, 7]);

        // OpCall has one 1-byte operand
        let ins = make(OpCode::OpCall, &[2]);
        assert_eq!(ins, vec![OpCode::OpCall as u8, 2]);
    }

    #[test]
    fn make_multi_operands() {
        // OpClosure has operands [u16, u8]
        // constant index 1, free vars 2
        let ins = make(OpCode::OpClosure, &[1, 2]);
        assert_eq!(ins, vec![OpCode::OpClosure as u8, 0x00, 0x01, 0x02]);
    }

    #[test]
    fn read_helpers() {
        let bytes = vec![0xAA, 0x01, 0x02, 0xFF];

        assert_eq!(read_u8(&bytes, 0), 0xAA);
        assert_eq!(read_u16(&bytes, 1), 0x0102);
        assert_eq!(read_u8(&bytes, 3), 0xFF);
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
    fn operand_widths_contract() {
        assert_eq!(operand_widths(OpCode::OpConstant), vec![2]);
        assert_eq!(operand_widths(OpCode::OpGetLocal), vec![1]);
        assert_eq!(operand_widths(OpCode::OpClosure), vec![2, 1]);
        assert_eq!(operand_widths(OpCode::OpAdd), Vec::<usize>::new());
    }
}
