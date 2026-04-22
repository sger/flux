//! Continuation-splitting pre-pass (Proposal 0162 Phase 3 slice 3b).
//!
//! For every `LirTerminator::Call` whose dst may be observed after a yield
//! (enabled by `FLUX_YIELD_CHECKS=1`), synthesize a fresh `LirFunction`
//! representing "the rest of the work after the call completes". That
//! synthesized function is then packaged as a closure and passed to
//! `flux_yield_extend` on the yield path (slice 3b-ii handles the LLVM wiring).
//!
//! The synthesized function's body is a slice of the parent: its entry block
//! copies the resume value and captured live vars into the parent's own
//! variables, then jumps to the `cont` block. All CFG-reachable blocks from
//! `cont` are copied in verbatim (SSA ids stay unique because they were
//! unique in the parent).
//!
//! Scope notes
//! -----------
//! - Only runs when `FLUX_YIELD_CHECKS=1` is set in the environment (same
//!   gate as slice 3a's emission).
//! - Leaves `Call.yield_cont = None` when the cont block has no successors
//!   reachable from entry — nothing to continue.
//! - Nested yield checks inside the synthesized function itself are left for
//!   a follow-up. The first-level split is the interesting case.

use std::collections::HashSet;

use crate::core::FluxRep;
use crate::lir::liveness::{self, FunctionLiveness};
use crate::lir::{
    BlockId, CtorArm, LirBlock, LirFuncId, LirFunction, LirProgram, LirTerminator, LirVar,
};

/// Run the continuation-splitting pass, producing a new program with
/// synthesized continuation functions and `Call.yield_cont` populated.
///
/// No-op when `FLUX_YIELD_CHECKS` is unset (or set to `"0"`/empty) — matches
/// slice 3a's gating so default output is unchanged.
pub fn split_continuations(mut program: LirProgram) -> LirProgram {
    if !yield_checks_enabled() {
        return program;
    }

    // Collect work items first to avoid borrow conflicts (we'll mutate
    // `program` while iterating).
    let mut work: Vec<(usize, Vec<CallSite>)> = Vec::new();
    for (idx, func) in program.functions.iter().enumerate() {
        let sites = collect_call_sites(func);
        if !sites.is_empty() {
            work.push((idx, sites));
        }
    }

    for (func_idx, sites) in work {
        // Pre-compute liveness once per function; all call sites share it.
        let live = liveness::compute(&program.functions[func_idx]);

        for mut site in sites {
            // Attempt synthesis first so we only burn a synthetic id when the
            // synthesis actually produces a function.
            let (mut synthesized, live_captures) =
                match synthesize_continuation(&program, func_idx, &site, &live) {
                    Some(x) => x,
                    None => continue,
                };

            // Allocate a real synth id now and stamp it on the synthesized fn.
            let synth_id = program.alloc_synthetic_func_id();
            site.synth_id = synth_id;
            synthesized.id = synth_id;

            // Register the synthesized function.
            program.push_function(synthesized);

            // Populate the original Call's yield_cont pointer.
            let func = &mut program.functions[func_idx];
            let block = func
                .blocks
                .iter_mut()
                .find(|b| b.id == site.block_id)
                .expect("call site block disappeared");
            if let LirTerminator::Call { yield_cont, .. } = &mut block.terminator {
                *yield_cont = Some((site.synth_id, live_captures));
            }
        }
    }

    program
}

/// True when the `FLUX_YIELD_CHECKS` environment variable opts in.
/// Mirrors `emit_llvm::yield_checks_enabled` — intentionally duplicated so
/// this module stays feature-flag-agnostic at the type level.
fn yield_checks_enabled() -> bool {
    match std::env::var("FLUX_YIELD_CHECKS") {
        Ok(v) => !v.is_empty() && v != "0",
        Err(_) => false,
    }
}

/// A call site that needs a continuation function synthesized for it.
struct CallSite {
    block_id: BlockId,
    /// The `dst` var defined by the Call (bound in the cont block).
    dst: LirVar,
    /// The `cont` block id the Call branches to on normal completion.
    cont: BlockId,
    /// Fresh LirFuncId reserved for the synthesized continuation.
    synth_id: LirFuncId,
}

fn collect_call_sites(func: &LirFunction) -> Vec<CallSite> {
    let mut sites = Vec::new();
    for block in &func.blocks {
        if let LirTerminator::Call {
            dst,
            cont,
            suppress_yield_check,
            yield_cont,
            ..
        } = &block.terminator
        {
            if *suppress_yield_check || yield_cont.is_some() {
                // Already populated (e.g., by a previous pass run) — skip.
                continue;
            }
            // We can't allocate the synth id here without &mut program; defer
            // to synthesize_continuation.
            sites.push(CallSite {
                block_id: block.id,
                dst: *dst,
                cont: *cont,
                synth_id: LirFuncId(u32::MAX), // placeholder, filled in below
            });
        }
    }
    sites
}

/// Synthesize a continuation LirFunction for one call site. Returns
/// `(function, live_capture_vars)` or `None` if the cont subgraph is trivial
/// enough that no synthesis is required.
fn synthesize_continuation(
    program: &LirProgram,
    func_idx: usize,
    site: &CallSite,
    live: &FunctionLiveness,
) -> Option<(LirFunction, Vec<LirVar>)> {
    let parent = &program.functions[func_idx];

    // The closure's capture set: live_in(cont) ∪ {dst}. `dst` is the Call's
    // result, which the cont block expects to have been bound. Including it
    // in the capture set lets the synthesized function's entry block copy it
    // into position from the resume value.
    let mut live_capture_set: Vec<LirVar> = live.live_in(site.cont).iter().copied().collect();
    if !live_capture_set.contains(&site.dst) {
        live_capture_set.push(site.dst);
    }
    // Deterministic order for stable emission + reproducible closure payloads.
    live_capture_set.sort_by_key(|v| v.0);

    // Collect all blocks CFG-reachable from `cont`. This is the "rest of the
    // function" we need to materialize inside the synthesized fn.
    let reachable = collect_reachable(parent, site.cont);
    // Trivial case: `cont` has no successors and contains no instrs — no work
    // to synthesize. Skip. (Also catches unreachable-code scenarios.)
    if reachable.is_empty() {
        return None;
    }

    // The synthesized function's layout follows the existing direct-capture
    // convention used by `emit_llvm` for nested closures with captures:
    //
    //   params       = [dst]                         (user-visible resume arg)
    //   capture_vars = [live_1, live_2, ...]          (sans dst; loaded from
    //                                                  the closure payload at
    //                                                  entry by the wrapper)
    //
    // By naming the resume parameter `dst` itself, the cont block's existing
    // uses of `dst` refer to the param directly — no Copy needed. Captures
    // retain their original LirVar ids, and the closure-entry wrapper
    // auto-loads them from the payload into those names.
    let capture_vars: Vec<LirVar> = live_capture_set
        .iter()
        .copied()
        .filter(|v| *v != site.dst)
        .collect();

    // The native emitter relies on the invariant `blocks[i].id == BlockId(i)`
    // — its per-function reachability walk indexes `blocks` by `BlockId.0`.
    // The reachable slice from the parent has sparse/non-zero ids (cont may
    // be BlockId(5), its successors might be BlockId(8), etc.). Remap every
    // copied block to a dense 0..N range, with the synthesized entry at id 0.
    //
    // Block-id remap: parent_id → new_dense_id.
    let mut id_remap: std::collections::HashMap<BlockId, BlockId> = Default::default();
    // Entry block gets id 0.
    id_remap.insert(BlockId(u32::MAX), BlockId(0)); // sentinel for entry
    let mut next_id = 1u32;
    // Copy the reachable blocks in a deterministic order (parent order),
    // assigning fresh dense ids.
    for block in &parent.blocks {
        if reachable.contains(&block.id) {
            id_remap.insert(block.id, BlockId(next_id));
            next_id += 1;
        }
    }

    // Trivial entry block (id=0) that jumps to the remapped cont.
    let entry_block = LirBlock {
        id: BlockId(0),
        params: Vec::new(),
        instrs: Vec::new(),
        terminator: LirTerminator::Jump(id_remap[&site.cont]),
    };

    let mut synth_blocks: Vec<LirBlock> = Vec::with_capacity(reachable.len() + 1);
    synth_blocks.push(entry_block);
    for block in &parent.blocks {
        if reachable.contains(&block.id) {
            let mut copied = block.clone();
            copied.id = id_remap[&block.id];
            remap_terminator(&mut copied.terminator, &id_remap);
            synth_blocks.push(copied);
        }
    }

    // `next_var` must stay above every LirVar used inside the slice so that
    // the synthesized function can allocate fresh vars if a later pass needs
    // to. The safest upper bound is the parent's own next_var.
    let next_var = parent.next_var;

    let qualified_name = format!("{}$cont${}", parent.qualified_name, site.block_id.0);
    let synthesized = LirFunction {
        name: qualified_name.clone(),
        id: site.synth_id, // caller replaces with a real id after synthesis
        qualified_name,
        params: vec![site.dst],
        blocks: synth_blocks,
        next_var,
        capture_vars,
        param_reps: vec![FluxRep::TaggedRep; 1],
        result_rep: parent.result_rep,
    };

    Some((synthesized, live_capture_set))
}

/// Rewrite a terminator's block-id references using `id_remap`. All
/// successor ids the parent referenced must appear in the map (they were
/// added when their respective blocks were emitted into the synthesized fn).
fn remap_terminator(
    term: &mut LirTerminator,
    id_remap: &std::collections::HashMap<BlockId, BlockId>,
) {
    let remap = |b: BlockId| *id_remap.get(&b).unwrap_or(&b);
    match term {
        LirTerminator::Return(_) | LirTerminator::TailCall { .. } | LirTerminator::Unreachable => {}
        LirTerminator::Jump(b) => *b = remap(*b),
        LirTerminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            *then_block = remap(*then_block);
            *else_block = remap(*else_block);
        }
        LirTerminator::Switch { cases, default, .. } => {
            for (_, t) in cases.iter_mut() {
                *t = remap(*t);
            }
            *default = remap(*default);
        }
        LirTerminator::Call { cont, .. } => *cont = remap(*cont),
        LirTerminator::MatchCtor { arms, default, .. } => {
            for arm in arms.iter_mut() {
                arm.target = remap(arm.target);
            }
            *default = remap(*default);
        }
    }
}

/// Collect the set of BlockIds reachable from `start` by following successors
/// in `func`'s CFG.
fn collect_reachable(func: &LirFunction, start: BlockId) -> HashSet<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut stack = vec![start];
    while let Some(bid) = stack.pop() {
        if !visited.insert(bid) {
            continue;
        }
        let Some(block) = func.blocks.iter().find(|b| b.id == bid) else {
            continue;
        };
        for succ in terminator_successors(&block.terminator) {
            stack.push(succ);
        }
    }
    visited
}

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
    use crate::lir::{CallKind, LirBlock, LirFuncId, LirInstr, LirProgram};

    fn empty_function(id: u32, name: &str) -> LirFunction {
        LirFunction {
            name: name.to_string(),
            id: LirFuncId(id),
            qualified_name: name.to_string(),
            params: Vec::new(),
            blocks: Vec::new(),
            next_var: 0,
            capture_vars: Vec::new(),
            param_reps: Vec::new(),
            result_rep: FluxRep::TaggedRep,
        }
    }

    /// Build a trivial program: one function with one Call whose cont returns
    /// the call's dst. Verify that synthesize_continuation produces a fn
    /// whose entry block copies resume→dst and jumps to cont.
    #[test]
    fn synthesize_basic_continuation() {
        let mut program = LirProgram::new();
        let mut f = empty_function(0, "f");
        let v_func = LirVar(0);
        let v_dst = LirVar(1);
        f.params = vec![v_func];
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Call {
                dst: v_dst,
                func: v_func,
                args: Vec::new(),
                cont: BlockId(1),
                kind: CallKind::Indirect,
                suppress_yield_check: false,
                yield_cont: None,
            },
        });
        f.blocks.push(LirBlock {
            id: BlockId(1),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Return(v_dst),
        });
        f.next_var = 2;
        program.push_function(f);

        // Simulate the public API by hand (yield_checks_enabled gates it, but
        // we call synthesize_continuation directly for test determinism).
        let func = &program.functions[0];
        let live = liveness::compute(func);
        let site = CallSite {
            block_id: BlockId(0),
            dst: v_dst,
            cont: BlockId(1),
            synth_id: LirFuncId(100),
        };
        let (synth, captures) = synthesize_continuation(&program, 0, &site, &live)
            .expect("synthesize should produce a function");

        // Capture set: just {dst} (nothing else live at cont since bb1 only
        // returns dst).
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0], v_dst);

        // Params = [dst] (the resume value arrives in the dst var directly).
        assert_eq!(synth.params, vec![v_dst]);
        // No extra captures since the only live var is dst.
        assert!(synth.capture_vars.is_empty());

        // Two blocks: synthesized entry + cloned bb1.
        assert_eq!(synth.blocks.len(), 2);
        let entry = &synth.blocks[0];
        assert!(matches!(entry.terminator, LirTerminator::Jump(BlockId(1))));
        // Entry instrs are empty — the resume value arrives via the param.
        assert!(entry.instrs.is_empty());
    }

    /// Program with no Call terminators: pass is a no-op.
    #[test]
    fn no_calls_no_synthesis() {
        let mut program = LirProgram::new();
        let mut f = empty_function(0, "f");
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Return(LirVar(0)),
        });
        f.params = vec![LirVar(0)];
        f.next_var = 1;
        program.push_function(f);
        let before = program.functions.len();

        // Manually drive (skipping env gate).
        // Run the synthesis directly — no sites found → no changes.
        let sites = collect_call_sites(&program.functions[0]);
        assert!(sites.is_empty());
        assert_eq!(program.functions.len(), before);
    }

    /// When a live var (other than dst) is live at cont, it should appear as
    /// a capture param and get a Copy in the synthesized entry.
    #[test]
    fn extra_live_var_becomes_capture() {
        let mut program = LirProgram::new();
        let mut f = empty_function(0, "f");
        let v_func = LirVar(0);
        let v_arg = LirVar(1);
        let v_dst = LirVar(2);
        f.params = vec![v_func, v_arg];
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Call {
                dst: v_dst,
                func: v_func,
                args: Vec::new(),
                cont: BlockId(1),
                kind: CallKind::Indirect,
                suppress_yield_check: false,
                yield_cont: None,
            },
        });
        // bb1: uses v_arg (keeping it live through the call) + v_dst.
        f.blocks.push(LirBlock {
            id: BlockId(1),
            params: Vec::new(),
            instrs: vec![LirInstr::IAdd {
                dst: LirVar(3),
                a: v_arg,
                b: v_dst,
            }],
            terminator: LirTerminator::Return(LirVar(3)),
        });
        f.next_var = 4;
        program.push_function(f);

        let live = liveness::compute(&program.functions[0]);
        let site = CallSite {
            block_id: BlockId(0),
            dst: v_dst,
            cont: BlockId(1),
            synth_id: LirFuncId(100),
        };
        let (synth, captures) = synthesize_continuation(&program, 0, &site, &live).unwrap();

        // Live capture set: {v_arg, v_dst} (sorted).
        assert_eq!(captures, vec![v_arg, v_dst]);
        // Params = [dst] (resume value). Captures = [v_arg] (loaded from
        // closure payload at entry by the wrapper).
        assert_eq!(synth.params, vec![v_dst]);
        assert_eq!(synth.capture_vars, vec![v_arg]);
        // Entry is still empty (no Copies needed — closure wrapper loads
        // captures automatically).
        let entry = &synth.blocks[0];
        assert!(entry.instrs.is_empty());
    }

    #[test]
    fn suppressed_calls_are_not_split() {
        let mut f = empty_function(0, "f");
        let v_func = LirVar(0);
        let v_dst = LirVar(1);
        f.params = vec![v_func];
        f.blocks.push(LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Call {
                dst: v_dst,
                func: v_func,
                args: Vec::new(),
                cont: BlockId(1),
                kind: CallKind::Indirect,
                suppress_yield_check: true,
                yield_cont: None,
            },
        });
        f.blocks.push(LirBlock {
            id: BlockId(1),
            params: vec![v_dst],
            instrs: Vec::new(),
            terminator: LirTerminator::Return(v_dst),
        });

        assert!(collect_call_sites(&f).is_empty());
    }
}
