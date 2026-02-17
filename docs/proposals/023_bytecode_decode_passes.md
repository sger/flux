# Proposal 023: Bytecode Decode & Pass-Oriented Rewriting API

**Status:** Proposed
**Priority:** Medium
**Created:** 2026-02-12
**Related:** Proposal 022 (AST Traversal Framework, which identified bytecode inspection as a future proposal)

## Overview

Provide a structured bytecode inspection and rewriting API that operates on Flux's raw `Vec<u8>` instruction streams. Instead of a classic Visitor over raw bytes, this proposal introduces a zero-allocation decoder yielding `InstrView` values per instruction, an iterator for sequential decoding, and pass helpers (`for_each_instr`, `map_instrs`) that support instruction rewriting with automatic jump target fixup.

## Motivation

Flux's compiled bytecode is stored as `Vec<u8>` (aliased as `Instructions`). The existing `disassemble()` function in `op_code.rs` decodes instructions for display but returns a `String` — there is no structured API for programmatic inspection or rewriting. Adding optimization passes, bytecode analysis, or instruction rewriting currently requires hand-rolling byte-level decode logic each time.

As identified in [Proposal 022](022_ast_traversal_framework.md), each compiler layer should have its own inspection mechanism:

| Layer | Mechanism | Status |
|-------|-----------|--------|
| **Syntax (AST)** | `Visitor` + `Folder` traits | Done (Proposal 022) |
| **Bytecode** | Pass-oriented decode + rewrite | **This proposal** |
| **Runtime** | Instrumentation hooks | Future |

## Existing Infrastructure

All in [op_code.rs](../../src/bytecode/op_code.rs):

- `OpCode` enum — `#[repr(u8)]`, 50 variants (`OpConstant = 0` through `OpHashLong = 49`)
- `type Instructions = Vec<u8>` — raw byte vector
- `operand_widths(op) -> Vec<usize>` — returns operand byte widths per opcode
- `make(op, operands) -> Instructions` — encodes opcode + operands to bytes (big-endian)
- `read_u8`, `read_u16`, `read_u32` — big-endian operand readers
- `OpCode::from(u8)` — panics on unknown bytes (decode guards before calling)

### Instruction Encoding Summary

| Category | Opcodes | Total bytes |
|----------|---------|-------------|
| No operands | OpAdd, OpSub, OpPop, OpTrue, OpFalse, etc. (24 opcodes) | 1 |
| 1-byte operand (u8) | OpGetLocal, OpSetLocal, OpCall, OpTailCall, etc. (7 opcodes) | 2 |
| 2-byte operand (u16) | OpConstant, OpJump, OpJumpNotTruthy, OpJumpTruthy, OpGetGlobal, etc. (8 opcodes) | 3 |
| 4-byte operand (u32) | OpConstantLong, OpArrayLong, OpHashLong (3 opcodes) | 5 |
| 2 + 1 byte operands | OpClosure (u16 index + u8 free count) | 4 |
| 4 + 1 byte operands | OpClosureLong (u32 index + u8 free count) | 6 |

### Jump Opcodes

Three jump instructions, all using **u16 absolute** addresses (3 bytes total each):

| Opcode | Semantics |
|--------|-----------|
| `OpJump` | Unconditional jump |
| `OpJumpNotTruthy` | Jump if top-of-stack is falsy (peeks) |
| `OpJumpTruthy` | Jump if top-of-stack is truthy (peeks) |

The compiler uses placeholder-backpatching: emit with `9999`, then `change_operand(pos, target)` once the target is known.

## Value & Use Cases

### What This Enables

A structured decode/rewrite API is foundational infrastructure for bytecode-level compiler work. Without it, every new pass must hand-roll byte-level decoding logic, duplicating what `disassemble()` already does but in a non-reusable way.

### Concrete Use Cases

| Use Case | How it uses the API |
|----------|-------------------|
| **Peephole optimization** | `map_instrs` to pattern-match instruction sequences and replace with optimized equivalents (e.g., `OpConstant 0 + OpAdd` → remove the add, `OpPop; OpPop` → future `OpPopN 2`) |
| **Dead code elimination** | `for_each_instr` to build a control-flow graph from jump targets, identify unreachable instructions, then `map_instrs` to strip them with automatic jump fixup |
| **Bytecode verification** | `for_each_instr` to validate stack depth invariants, verify all jump targets land on instruction boundaries, check operand indices are in bounds |
| **Bytecode-level instrumentation** | `map_instrs` to inject profiling or tracing instructions before/after function calls without breaking jump targets |
| **Constant folding at bytecode level** | `map_instrs` to replace `OpConstant X; OpConstant Y; OpAdd` with `OpConstant Z` where Z = X + Y |
| **NOP sled removal** | If a future NOP opcode is added for alignment, `map_instrs` strips them with one pass and all jumps stay valid |
| **Binary size analysis** | `for_each_instr` to compute instruction frequency histograms, identify hot opcodes, measure function sizes |

### Why Not Just Use `disassemble()`?

`disassemble()` returns a `String` — useful for display, but unusable for programmatic analysis or rewriting. Every analysis pass that needs structured data would have to re-parse that string, which is fragile and wasteful. `InstrView` provides the same decoded data as a typed, zero-allocation struct.

### Why a Two-Pass Rewriter Instead of Manual Patching?

The compiler already has `change_operand()` for backpatching during emission, but that only works during compilation when you control the emission order. Post-compilation rewriting (optimization passes, instrumentation) faces the jump fixup problem: removing or inserting bytes shifts all subsequent addresses. `map_instrs` solves this generically so every future pass gets correct jump fixup for free.

## Design

### Module Structure

```
src/bytecode/
├── decode.rs    # InstrView, DecodeError, decode_at, DecodeIter, helpers
├── passes.rs    # for_each_instr, map_instrs with jump fixup
└── mod.rs       # Add pub mod decode; pub mod passes;
```

### Deliverable 1: `src/bytecode/decode.rs`

#### Types

```rust
/// Must match highest OpCode variant + 1. Enforced by test.
const OPCODE_COUNT: u8 = 50;

/// A decoded view of a single bytecode instruction. Zero-allocation (Copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrView {
    pub pc: usize,    // byte offset where instruction starts
    pub op: OpCode,   // decoded opcode
    pub a: u32,       // first operand (0 if unused)
    pub b: u32,       // second operand (0 if unused)
    pub len: usize,   // total byte length of instruction
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEnd { pc: usize },
    UnknownOpcode { pc: usize, byte: u8 },
}
```

#### Core Function: `decode_at`

```rust
pub fn decode_at(bytes: &[u8], pc: usize) -> Result<InstrView, DecodeError>
```

Algorithm:
1. Check `pc < bytes.len()`, else `UnexpectedEnd`
2. Read `byte = bytes[pc]`; if `byte >= OPCODE_COUNT`, return `UnknownOpcode` (guards before calling `OpCode::from()` which panics on unknown bytes)
3. Call `operand_widths(op)` to get widths
4. Compute `len = 1 + sum(widths)`; check `pc + len <= bytes.len()`, else `UnexpectedEnd`
5. Read operands using `read_u8`/`read_u16`/`read_u32` based on widths, store in `a` and `b`
6. Return `InstrView { pc, op, a, b, len }`

This reuses the existing `operand_widths`, `read_u8`, `read_u16`, `read_u32` from `op_code.rs` — decode logic stays automatically in sync with encoding.

#### Iterator: `DecodeIter`

```rust
pub struct DecodeIter<'a> {
    bytes: &'a [u8],
    pc: usize,
}

impl<'a> Iterator for DecodeIter<'a> {
    type Item = Result<InstrView, DecodeError>;
    // Calls decode_at(self.bytes, self.pc), advances by view.len
    // Returns None when pc >= bytes.len()
    // Stops iteration on error (sets pc = bytes.len())
}
```

#### Helpers

```rust
impl InstrView {
    /// Re-encode this view back to bytes via make().
    pub fn encode(&self) -> Vec<u8>;
    /// Number of operands (0, 1, or 2).
    pub fn operand_count(&self) -> usize;
}

/// True for OpJump, OpJumpNotTruthy, OpJumpTruthy.
pub fn is_jump(op: OpCode) -> bool;

/// If view is a jump, return the absolute target address.
pub fn jump_target(view: &InstrView) -> Option<usize>;
```

### Deliverable 2: `src/bytecode/passes.rs`

#### `for_each_instr`

```rust
pub fn for_each_instr<E>(
    bytes: &[u8],
    f: impl FnMut(InstrView) -> Result<(), E>,
) -> Result<(), ForEachError<E>>
```

Simple decode loop calling `f` for each instruction. Stops on decode or callback error.

#### `map_instrs` with Automatic Jump Fixup

```rust
pub fn map_instrs<E>(
    bytes: &[u8],
    f: impl FnMut(InstrView) -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, MapError<E>>
```

The callback returns replacement bytes per instruction. If replacement sizes differ from originals, jump targets are automatically remapped.

**Two-pass algorithm:**

**Pass 1 — Rewrite & build offset mapping:**
```
input_pc = 0, output_pc = 0
while input_pc < bytes.len():
    record: offset_map[input_pc] = output_pc
    view = decode_at(bytes, input_pc)
    replacement = f(view)
    output.extend(replacement)
    output_pc += replacement.len()
    input_pc += view.len
record: offset_map[input_pc] = output_pc    // one-past-end sentinel
```

**Pass 2 — Patch jump targets:**
```
scan_pc = 0
while scan_pc < output.len():
    view = decode_at(output, scan_pc)
    if is_jump(view.op):
        old_target = view.a
        new_target = offset_map[old_target]
        patch output[scan_pc+1..scan_pc+3] with new_target as u16 big-endian
    scan_pc += view.len
```

**Semantic contract:** Jump targets in replacement bytes are interpreted as old-PC addresses. Pass 2 remaps them via `offset_map`. If a callback re-encodes an existing jump (via `view.encode()`), the old target is preserved and remapped correctly. If a jump target is not found in the mapping, `MapError::InvalidJumpTarget` is returned.

#### Error Types

```rust
pub enum ForEachError<E> {
    Decode(DecodeError),
    Callback(E),
}

pub enum MapError<E> {
    Decode(DecodeError),
    Callback(E),
    InvalidJumpTarget { pc: usize, target: usize },
}
```

### Deliverable 3: Update `src/bytecode/mod.rs`

Add two module declarations in alphabetical order:
```rust
pub mod decode;       // after debug_info
pub mod passes;       // after op_code
```

### Deliverable 4: Tests (`tests/bytecode_decode_tests.rs`)

| Test | What it verifies |
|------|-----------------|
| **Round-trip decode** | Build stream with `make()` containing OpConstant, OpAdd, OpGetLocal, OpClosure, OpConstantLong, OpClosureLong. Iterate with `DecodeIter`, verify pc/op/a/b/len per instruction. Call `encode()` on each view and verify bytes match original. |
| **Unknown opcode** | `decode_at(&[0xFF], 0)` returns `DecodeError::UnknownOpcode` |
| **Truncated stream** | `decode_at(&[OpConstant as u8, 0x00], 0)` returns `DecodeError::UnexpectedEnd` |
| **Same-size rewrite** | Input: `[OpTrue, OpPop, OpFalse, OpPop]`. Replace OpTrue with OpFalse via `map_instrs`. Verify output = `[OpFalse, OpPop, OpFalse, OpPop]`. |
| **Insert rewrite + jump fixup** | Input: `OpJump target=4, OpPop, OpTrue`. Expand OpPop (1 byte) to 2 bytes. Verify OpJump target remapped from 4 to 5. |
| **Shrink rewrite + jump fixup** | Input: `OpJumpNotTruthy target=5, OpTrue, OpPop, OpFalse`. Delete OpTrue (return empty). Verify jump target remapped from 5 to 4. |
| **for_each_instr** | Count instructions and collect opcodes; verify count and sequence. |
| **is_jump / jump_target** | True for 3 jump opcodes, false for others. Correct target extraction. |
| **operand_count** | 0 for OpAdd, 1 for OpConstant, 1 for OpGetLocal, 2 for OpClosure. |
| **OPCODE_COUNT consistency** | Byte 49 decodes successfully; byte 50 returns `UnknownOpcode`. |

## Design Decisions

### Why not modify `OpCode::from(u8)`?

The existing `From<u8>` panics on unknown bytes. The VM dispatch relies on this always succeeding (it reads from validated bytecode). Changing to `TryFrom` would require updating every call site in the VM, compiler, and disassembler. Instead, `decode_at` guards with `byte >= OPCODE_COUNT` before calling `from()` — fully additive, zero risk to existing code.

### Why `Vec<u8>` callback return, not `InstrView`?

Returning `Vec<u8>` gives maximum flexibility: the callback can return zero bytes (delete instruction), one instruction (replace), or multiple instructions (expand). Returning `InstrView` would limit to single-instruction replacement.

### Why `OPCODE_COUNT` constant instead of enum introspection?

Rust doesn't provide `#[repr(u8)]` enum variant count at compile time. A test enforces the constant stays in sync with the enum by checking that byte 49 decodes and byte 50 does not.

### Why HashMap for offset_map?

A `Vec<(usize, usize)>` with binary search would also work since input PCs are monotonically increasing, but `HashMap` provides O(1) lookup and clearer code. The entry count is bounded by the number of instructions, which is small.

## Files Summary

| Action | File |
|--------|------|
| Create | `src/bytecode/decode.rs` |
| Create | `src/bytecode/passes.rs` |
| Create | `tests/bytecode_decode_tests.rs` |
| Modify | `src/bytecode/mod.rs` (add 2 module declarations) |

No modifications to `op_code.rs` or any existing file beyond `mod.rs`.

## Verification

```bash
cargo test --test bytecode_decode_tests               # New tests
cargo test --test op_code_tests                        # Existing opcode tests
cargo test                                             # Full suite (no regressions)
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

## Future Extensions

- **Peephole optimizer pass** — use `map_instrs` to implement constant folding, dead store elimination
- **Bytecode verifier** — use `for_each_instr` to validate stack depth, jump target alignment
- **NOP sled removal** — use `map_instrs` to strip no-ops with automatic jump fixup
- **Relative jumps** — if encoding switches from absolute to relative offsets, only `map_instrs` Pass 2 needs updating
