//! Backward liveness analysis over LIR (Proposal 0162 Phase 3).
//!
//! Computes, for every program point, the set of `LirVar`s whose values may
//! still be read after that point. The native backend uses this to decide
//! which variables must be spilled into a yield continuation closure at each
//! `Call` that could observe a yield.
//!
//! Implementation notes
//! --------------------
//! LIR is SSA with block parameters playing the role of phi arguments. We use
//! a classic per-block `live_in` / `live_out` summary with backward dataflow
//! iteration to fixed point:
//!
//! ```text
//! live_in(b)  = (live_out(b) \ defs(b)) ∪ uses(b)
//! live_out(b) = ⋃ live_in(succ)        (∀ succ ∈ successors(b))
//! ```
//!
//! Block parameters are treated as definitions at block entry — they are
//! "defined" in the block whose header introduces them, and branching
//! terminators effectively "use" the values they pass as phi inputs. The
//! current LIR shape does not yet carry phi arguments on branch terminators,
//! so block parameters are live from the point of view of this analysis only
//! if they are actually read inside the block — that matches the shape we
//! need for yield-continuation spilling (we need to know what the *cont*
//! block reads, not how its parameters arrived).
//!
//! The analysis is intentionally conservative: it treats every var touched by
//! an instruction as a use (even vars that the LLVM backend knows are inline
//! constants). False positives only cost a bit of closure-capture space; they
//! don't affect correctness.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::lir::{
    BlockId, CallKind, CtorArm, LirBlock, LirFunction, LirInstr, LirTerminator, LirVar,
};

/// Per-block liveness summary. `live_in` is the set of vars live entering the
/// block; `live_out` is the set live after the terminator executes. The
/// analysis provides these as `HashSet<LirVar>`s.
#[derive(Debug, Clone, Default)]
pub struct BlockLiveness {
    pub live_in: HashSet<LirVar>,
    pub live_out: HashSet<LirVar>,
}

/// Full liveness result for a function: a map from block id to its summary.
#[derive(Debug, Clone, Default)]
pub struct FunctionLiveness {
    pub blocks: HashMap<BlockId, BlockLiveness>,
}

impl FunctionLiveness {
    /// Return the set of vars live entering `block`. Empty set if the block is
    /// missing (shouldn't happen on a well-formed function).
    pub fn live_in(&self, block: BlockId) -> &HashSet<LirVar> {
        self.blocks
            .get(&block)
            .map(|b| &b.live_in)
            .unwrap_or(&EMPTY_SET)
    }

    /// Return the set of vars live leaving `block`. Empty for returning / tail
    /// blocks.
    pub fn live_out(&self, block: BlockId) -> &HashSet<LirVar> {
        self.blocks
            .get(&block)
            .map(|b| &b.live_out)
            .unwrap_or(&EMPTY_SET)
    }
}

/// Shared empty set so `live_in` / `live_out` can return references without
/// allocating on the missing-block path.
static EMPTY_SET: LazyLock<HashSet<LirVar>> = LazyLock::new(HashSet::new);

/// Compute liveness for every block in `function`.
///
/// The algorithm is a fixed-point iteration over block summaries:
/// for each block, recompute `live_out` from its successors' `live_in`s, then
/// recompute `live_in` by walking the block's instructions backward. Repeat
/// until no summary changes. LIR CFGs are small enough that a worklist isn't
/// necessary.
pub fn compute(function: &LirFunction) -> FunctionLiveness {
    let mut blocks: HashMap<BlockId, BlockLiveness> = function
        .blocks
        .iter()
        .map(|b| (b.id, BlockLiveness::default()))
        .collect();

    let successors: HashMap<BlockId, Vec<BlockId>> = function
        .blocks
        .iter()
        .map(|b| (b.id, terminator_successors(&b.terminator)))
        .collect();

    let match_binders = match_ctor_field_binders_by_target(function);

    loop {
        let mut changed = false;
        for block in &function.blocks {
            // live_out(b) = ⋃ live_in(succ)
            let mut new_live_out: HashSet<LirVar> = HashSet::new();
            if let Some(succs) = successors.get(&block.id) {
                for succ in succs {
                    if let Some(summary) = blocks.get(succ) {
                        new_live_out.extend(summary.live_in.iter().copied());
                    }
                }
            }
            // live_in(b) = (live_out(b) ∪ uses by terminator then body, minus defs)
            let mut new_live_in = compute_block_live_in(block, &new_live_out);
            // MatchCtor field_binders become defs at the arm's target block
            // entry — kill them from live_in of that target. Without this,
            // binders appear live above a block that actually defines them,
            // causing spurious inclusion in yield-continuation capture sets.
            if let Some(binders) = match_binders.get(&block.id) {
                for b in binders {
                    new_live_in.remove(b);
                }
            }

            let summary = blocks.get_mut(&block.id).unwrap();
            if summary.live_in != new_live_in || summary.live_out != new_live_out {
                summary.live_in = new_live_in;
                summary.live_out = new_live_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    FunctionLiveness { blocks }
}

/// Walk a block backward to compute its `live_in` given a `live_out`. The
/// result excludes vars defined in the block (including its parameters) and
/// includes every var read before being redefined.
fn compute_block_live_in(block: &LirBlock, live_out: &HashSet<LirVar>) -> HashSet<LirVar> {
    let mut live: HashSet<LirVar> = live_out.clone();

    // Terminator first (executed after all instrs).
    visit_terminator_defs_and_uses(&block.terminator, &mut live);

    // Instrs in reverse order.
    for instr in block.instrs.iter().rev() {
        visit_instr_defs_and_uses(instr, &mut live);
    }

    // Block params are defs at block entry.
    for p in &block.params {
        live.remove(p);
    }

    live
}

/// Vars defined by a `MatchCtor`'s field binders at the target block's
/// entry. These behave like block params but are attached to the arm, not
/// the destination block itself.
fn match_ctor_field_binders_by_target(
    func: &LirFunction,
) -> HashMap<BlockId, HashSet<LirVar>> {
    let mut map: HashMap<BlockId, HashSet<LirVar>> = HashMap::new();
    for block in &func.blocks {
        if let LirTerminator::MatchCtor { arms, .. } = &block.terminator {
            for arm in arms {
                if !arm.field_binders.is_empty() {
                    map.entry(arm.target)
                        .or_default()
                        .extend(arm.field_binders.iter().copied());
                }
            }
        }
    }
    map
}

/// Update `live` to reflect executing `instr`'s defs (removed from live) and
/// uses (added to live) in backward order.
fn visit_instr_defs_and_uses(instr: &LirInstr, live: &mut HashSet<LirVar>) {
    // Remove defs.
    if let Some(def) = instr_def(instr) {
        live.remove(&def);
    }
    // Add uses.
    for u in instr_uses(instr) {
        live.insert(u);
    }
}

/// Update `live` for a terminator's defs/uses in backward order.
fn visit_terminator_defs_and_uses(term: &LirTerminator, live: &mut HashSet<LirVar>) {
    // `Call` defines `dst` in `cont` — not here. Match-bound field binders are
    // defined at the arm's target block entry (handled via block params in the
    // current lowering, or explicitly via `field_binders`). We only need to
    // record *uses* contributed by the terminator itself.
    for u in terminator_uses(term) {
        live.insert(u);
    }
}

/// Return the single SSA def produced by an instruction, if any.
fn instr_def(instr: &LirInstr) -> Option<LirVar> {
    match instr {
        LirInstr::Load { dst, .. }
        | LirInstr::Alloc { dst, .. }
        | LirInstr::TagInt { dst, .. }
        | LirInstr::UntagInt { dst, .. }
        | LirInstr::TagFloat { dst, .. }
        | LirInstr::UntagFloat { dst, .. }
        | LirInstr::GetTag { dst, .. }
        | LirInstr::TagPtr { dst, .. }
        | LirInstr::UntagPtr { dst, .. }
        | LirInstr::TagBool { dst, .. }
        | LirInstr::UntagBool { dst, .. }
        | LirInstr::IAdd { dst, .. }
        | LirInstr::ISub { dst, .. }
        | LirInstr::IMul { dst, .. }
        | LirInstr::IDiv { dst, .. }
        | LirInstr::IRem { dst, .. }
        | LirInstr::IAnd { dst, .. }
        | LirInstr::IOr { dst, .. }
        | LirInstr::IXor { dst, .. }
        | LirInstr::IShl { dst, .. }
        | LirInstr::IShr { dst, .. }
        | LirInstr::ICmp { dst, .. }
        | LirInstr::IsUnique { dst, .. }
        | LirInstr::DropReuse { dst, .. }
        | LirInstr::MakeClosure { dst, .. }
        | LirInstr::MakeExternClosure { dst, .. }
        | LirInstr::MakeArray { dst, .. }
        | LirInstr::MakeTuple { dst, .. }
        | LirInstr::MakeHash { dst, .. }
        | LirInstr::MakeList { dst, .. }
        | LirInstr::Interpolate { dst, .. }
        | LirInstr::TupleGet { dst, .. }
        | LirInstr::MakeCtor { dst, .. }
        | LirInstr::Copy { dst, .. }
        | LirInstr::Const { dst, .. }
        | LirInstr::GetGlobal { dst, .. } => Some(*dst),
        LirInstr::PrimCall { dst, .. } => *dst,
        LirInstr::Store { .. }
        | LirInstr::StoreI32 { .. }
        | LirInstr::Dup { .. }
        | LirInstr::Drop { .. } => None,
    }
}

/// Return the vars read by an instruction.
fn instr_uses(instr: &LirInstr) -> Vec<LirVar> {
    match instr {
        LirInstr::Load { ptr, .. } => vec![*ptr],
        LirInstr::Store { ptr, val, .. } => vec![*ptr, *val],
        LirInstr::StoreI32 { ptr, .. } => vec![*ptr],
        LirInstr::Alloc { .. } => Vec::new(),
        LirInstr::TagInt { raw, .. }
        | LirInstr::TagFloat { raw, .. }
        | LirInstr::TagPtr { ptr: raw, .. }
        | LirInstr::TagBool { raw, .. } => vec![*raw],
        LirInstr::UntagInt { val, .. }
        | LirInstr::UntagFloat { val, .. }
        | LirInstr::GetTag { val, .. }
        | LirInstr::UntagPtr { val, .. }
        | LirInstr::UntagBool { val, .. } => vec![*val],
        LirInstr::IAdd { a, b, .. }
        | LirInstr::ISub { a, b, .. }
        | LirInstr::IMul { a, b, .. }
        | LirInstr::IDiv { a, b, .. }
        | LirInstr::IRem { a, b, .. }
        | LirInstr::IAnd { a, b, .. }
        | LirInstr::IOr { a, b, .. }
        | LirInstr::IXor { a, b, .. }
        | LirInstr::IShl { a, b, .. }
        | LirInstr::IShr { a, b, .. }
        | LirInstr::ICmp { a, b, .. } => vec![*a, *b],
        LirInstr::PrimCall { args, .. } => args.clone(),
        LirInstr::Dup { val } | LirInstr::Drop { val } | LirInstr::IsUnique { val, .. } => {
            vec![*val]
        }
        LirInstr::DropReuse { val, .. } => vec![*val],
        LirInstr::MakeClosure { captures, .. } => captures.clone(),
        LirInstr::MakeExternClosure { .. } => Vec::new(),
        LirInstr::MakeArray { elements, .. }
        | LirInstr::MakeTuple { elements, .. }
        | LirInstr::MakeList { elements, .. } => elements.clone(),
        LirInstr::MakeHash { pairs, .. } => pairs.clone(),
        LirInstr::Interpolate { parts, .. } => parts.clone(),
        LirInstr::TupleGet { tuple, .. } => vec![*tuple],
        LirInstr::MakeCtor { fields, .. } => fields.clone(),
        LirInstr::Copy { src, .. } => vec![*src],
        LirInstr::Const { .. } | LirInstr::GetGlobal { .. } => Vec::new(),
    }
}

/// Return the vars read by a terminator.
///
/// Important: `CallKind::DirectClosure` carries its captures separately from
/// the `args` slice — those captures are threaded into the emitted LLVM call
/// at code-gen time. They must be counted as uses here or they'll go missing
/// from the live-out set at the call site, which the yield-continuation
/// splitter uses to decide what to spill.
fn terminator_uses(term: &LirTerminator) -> Vec<LirVar> {
    match term {
        LirTerminator::Return(v) => vec![*v],
        LirTerminator::Jump(_) => Vec::new(),
        LirTerminator::Branch { cond, .. } => vec![*cond],
        LirTerminator::Switch { scrutinee, .. } => vec![*scrutinee],
        LirTerminator::TailCall { func, args, kind } => {
            let mut v = vec![*func];
            v.extend(args.iter().copied());
            if let CallKind::DirectClosure { captures, .. } = kind {
                v.extend(captures.iter().copied());
            }
            v
        }
        LirTerminator::Call {
            func,
            args,
            kind,
            yield_cont,
            ..
        } => {
            let mut v = vec![*func];
            v.extend(args.iter().copied());
            if let CallKind::DirectClosure { captures, .. } = kind {
                v.extend(captures.iter().copied());
            }
            if let Some((_, captures)) = yield_cont {
                v.extend(captures.iter().copied());
            }
            v
        }
        LirTerminator::MatchCtor { scrutinee, .. } => vec![*scrutinee],
        LirTerminator::Unreachable => Vec::new(),
    }
}

/// Successor block ids for a terminator.
fn terminator_successors(term: &LirTerminator) -> Vec<BlockId> {
    match term {
        LirTerminator::Return(_) | LirTerminator::TailCall { .. } | LirTerminator::Unreachable => {
            Vec::new()
        }
        LirTerminator::Jump(b) => vec![*b],
        LirTerminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        LirTerminator::Switch { cases, default, .. } => {
            let mut v: Vec<BlockId> = cases.iter().map(|(_, b)| *b).collect();
            v.push(*default);
            v
        }
        LirTerminator::Call { cont, .. } => vec![*cont],
        LirTerminator::MatchCtor { arms, default, .. } => {
            let mut v: Vec<BlockId> = arms.iter().map(|a: &CtorArm| a.target).collect();
            v.push(*default);
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::FluxRep;
    use crate::lir::{CallKind, LirFuncId, LirProgram};

    fn empty_function() -> LirFunction {
        LirFunction {
            name: "test".to_string(),
            id: LirFuncId(0),
            qualified_name: "test".to_string(),
            params: Vec::new(),
            blocks: Vec::new(),
            next_var: 0,
            capture_vars: Vec::new(),
            param_reps: Vec::new(),
            result_rep: FluxRep::TaggedRep,
        }
    }

    #[test]
    fn simple_return_live_in_has_returned_var() {
        let mut f = empty_function();
        let v0 = LirVar(0);
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Return(v0),
        });
        f.next_var = 1;

        let live = compute(&f);
        assert!(live.live_in(BlockId(0)).contains(&v0));
        assert!(live.live_out(BlockId(0)).is_empty());
    }

    #[test]
    fn instr_def_kills_later_use() {
        // bb0: %0 = const 7 ; return %0
        // Since %0 is defined inside bb0, it should NOT appear in live_in.
        let mut f = empty_function();
        let v0 = LirVar(0);
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: vec![LirInstr::Const {
                dst: v0,
                value: crate::lir::LirConst::Int(7),
            }],
            terminator: LirTerminator::Return(v0),
        });
        f.next_var = 1;

        let live = compute(&f);
        assert!(!live.live_in(BlockId(0)).contains(&v0));
    }

    #[test]
    fn call_cont_propagates_live_in_as_live_out() {
        // bb0: call f(%0) → bb1(dst=%1)   (dst bound in bb1 as a block param)
        // bb1: return %1
        //
        // Expected: live_in(bb0) contains %0 (use) and neither %1 (defined by
        // Call's dst, never live-in to bb0); live_out(bb0) is empty because
        // bb1's live_in is just %1, which is bb1's block param so it's killed
        // at entry → live_in(bb1) = {}. Thus live_out(bb0) = live_in(bb1) = {}.
        let mut f = empty_function();
        let v0 = LirVar(0);
        let v1 = LirVar(1);
        let func = LirVar(2);
        f.params = vec![v0, func];
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Call {
                dst: v1,
                func,
                args: vec![v0],
                cont: BlockId(1),
                kind: CallKind::Indirect,
                yield_cont: None,
            },
        });
        f.blocks.push(LirBlock {
            id: BlockId(1),
            params: vec![v1],
            instrs: Vec::new(),
            terminator: LirTerminator::Return(v1),
        });
        f.next_var = 3;

        let live = compute(&f);
        let in0 = live.live_in(BlockId(0));
        assert!(in0.contains(&v0));
        assert!(in0.contains(&func));
        assert!(!in0.contains(&v1));
        // bb1's param %1 kills its own live-in.
        assert!(!live.live_in(BlockId(1)).contains(&v1));
    }

    #[test]
    fn loop_reaches_fixed_point() {
        // bb0 → bb1 → bb0 (infinite loop).
        // Any var used inside should be live in both blocks; absent vars stay
        // absent. Goal: the iteration terminates.
        let mut f = empty_function();
        let v0 = LirVar(0);
        f.params = vec![v0];
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Jump(BlockId(1)),
        });
        f.blocks.push(LirBlock {
            id: BlockId(1),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Branch {
                cond: v0,
                then_block: BlockId(0),
                else_block: BlockId(1),
            },
        });
        f.next_var = 1;

        let live = compute(&f);
        assert!(live.live_in(BlockId(0)).contains(&v0));
        assert!(live.live_in(BlockId(1)).contains(&v0));
    }

    // Unused in the test body but keeps the import graph realistic.
    #[allow(dead_code)]
    fn _program_shape_check(_p: &LirProgram) {}
}
