//! Outline excess `run_async_with*` native call sites before LLVM sees them.
//!
//! LLVM can hang optimizing one function that contains many calls into the
//! `run_async_with*` wrapper chain plus a suspending async body. This pass
//! preserves the first few call sites and moves later ones into tiny noinline
//! helpers, keeping each generated LLVM function below the known threshold.

use std::collections::HashSet;

use crate::core::{CorePrimOp, FluxRep};
use crate::lir::{
    BlockId, CallKind, LirBlock, LirConst, LirFuncId, LirFunction, LirInstr, LirProgram,
    LirTerminator, LirVar,
};

pub const RUN_ASYNC_WITH_INLINE_SITE_LIMIT: usize = 8;
const OUTLINE_HELPER_PREFIX: &str = "__flux_run_async_with_outline_";

pub fn is_run_async_with_outline_helper(func: &LirFunction) -> bool {
    func.qualified_name.starts_with(OUTLINE_HELPER_PREFIX)
}

pub fn outline_excess_run_async_with_sites(mut program: LirProgram) -> LirProgram {
    outline_excess_run_async_with_sites_in_place(&mut program);
    program
}

pub fn outline_excess_run_async_with_sites_in_place(program: &mut LirProgram) {
    let run_async_funcs = collect_run_async_with_boundary_funcs(program);

    let mut func_idx = 0usize;
    while func_idx < program.functions.len() {
        if is_run_async_with_outline_helper(&program.functions[func_idx]) {
            func_idx += 1;
            continue;
        }

        let mut seen = 0usize;
        let mut block_idx = 0usize;
        while block_idx < program.functions[func_idx].blocks.len() {
            let outline_site = {
                let block = &program.functions[func_idx].blocks[block_idx];
                let mut found = None;
                for (instr_idx, instr) in block.instrs.iter().enumerate() {
                    if is_run_async_with_primcall(instr) {
                        seen += 1;
                        if seen > RUN_ASYNC_WITH_INLINE_SITE_LIMIT {
                            found = Some(OutlineSite::Instr(instr_idx));
                            break;
                        }
                    }
                }
                if found.is_none()
                    && terminator_run_async_with_target(&block.terminator, &run_async_funcs)
                        .is_some()
                {
                    seen += 1;
                    if seen > RUN_ASYNC_WITH_INLINE_SITE_LIMIT {
                        found = Some(OutlineSite::Terminator);
                    }
                }
                found
            };

            if let Some(site) = outline_site {
                let helper_id = program.alloc_synthetic_func_id();
                let helper = match site {
                    OutlineSite::Instr(instr_idx) => outline_instr_site(
                        &mut program.functions[func_idx],
                        block_idx,
                        instr_idx,
                        helper_id,
                    ),
                    OutlineSite::Terminator => outline_terminator_site(
                        &mut program.functions[func_idx],
                        block_idx,
                        helper_id,
                    ),
                };
                program.push_function(helper);
            }

            block_idx += 1;
        }

        suppress_run_async_boundary_yield_checks(
            &mut program.functions[func_idx],
            &run_async_funcs,
        );
        func_idx += 1;
    }
}

#[derive(Debug, Clone, Copy)]
enum OutlineSite {
    Instr(usize),
    Terminator,
}

fn collect_run_async_with_boundary_funcs(program: &LirProgram) -> HashSet<LirFuncId> {
    let mut funcs = HashSet::new();

    loop {
        let mut changed = false;
        for func in &program.functions {
            if funcs.contains(&func.id) {
                continue;
            }
            if function_reaches_run_async_with(func, &funcs) {
                funcs.insert(func.id);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    funcs
}

fn function_reaches_run_async_with(func: &LirFunction, known: &HashSet<LirFuncId>) -> bool {
    func.blocks.iter().any(|block| {
        block.instrs.iter().any(is_run_async_with_primcall)
            || terminator_run_async_with_target(&block.terminator, known).is_some()
    })
}

fn is_run_async_with_primcall(instr: &LirInstr) -> bool {
    matches!(
        instr,
        LirInstr::PrimCall {
            dst: Some(_),
            op: CorePrimOp::FiberRunAsyncWith,
            ..
        }
    )
}

fn terminator_run_async_with_target(
    term: &LirTerminator,
    run_async_funcs: &HashSet<LirFuncId>,
) -> Option<LirFuncId> {
    match term {
        LirTerminator::Call {
            kind: CallKind::Direct { func_id },
            ..
        }
        | LirTerminator::TailCall {
            kind: CallKind::Direct { func_id },
            ..
        } if run_async_funcs.contains(func_id) => Some(*func_id),
        _ => None,
    }
}

fn outline_instr_site(
    func: &mut LirFunction,
    block_idx: usize,
    instr_idx: usize,
    helper_id: LirFuncId,
) -> LirFunction {
    let helper_name = helper_name(func, helper_id);
    let dummy_func = func.fresh_var();
    let cont_id = BlockId(func.blocks.len() as u32);

    let (dst, args, after_instrs, old_terminator) = {
        let block = &mut func.blocks[block_idx];
        let after_instrs = block.instrs.split_off(instr_idx + 1);
        let outlined = block
            .instrs
            .pop()
            .expect("outline position should have an instruction");
        let LirInstr::PrimCall {
            dst: Some(dst),
            op: CorePrimOp::FiberRunAsyncWith,
            args,
        } = outlined
        else {
            unreachable!("outline position must be a FiberRunAsyncWith PrimCall");
        };

        block.instrs.push(LirInstr::Const {
            dst: dummy_func,
            value: LirConst::None,
        });
        let old_terminator = std::mem::replace(
            &mut block.terminator,
            LirTerminator::Call {
                dst,
                func: dummy_func,
                args: args.clone(),
                cont: cont_id,
                kind: CallKind::Direct { func_id: helper_id },
                suppress_yield_check: true,
                yield_cont: None,
            },
        );
        (dst, args, after_instrs, old_terminator)
    };

    func.blocks.push(LirBlock {
        id: cont_id,
        params: vec![dst],
        instrs: after_instrs,
        terminator: old_terminator,
    });

    make_primcall_helper(helper_id, helper_name, args.len())
}

fn outline_terminator_site(
    func: &mut LirFunction,
    block_idx: usize,
    helper_id: LirFuncId,
) -> LirFunction {
    let helper_name = helper_name(func, helper_id);
    let (target_id, arity) = {
        let block = &mut func.blocks[block_idx];
        match &mut block.terminator {
            LirTerminator::Call {
                args,
                suppress_yield_check,
                kind: CallKind::Direct { func_id },
                ..
            } => {
                let original = *func_id;
                let arity = args.len();
                *func_id = helper_id;
                *suppress_yield_check = true;
                (original, arity)
            }
            LirTerminator::TailCall {
                args,
                kind: CallKind::Direct { func_id },
                ..
            } => {
                let original = *func_id;
                let arity = args.len();
                *func_id = helper_id;
                (original, arity)
            }
            _ => unreachable!("outline terminator must be a direct call"),
        }
    };

    make_direct_call_helper(helper_id, helper_name, target_id, arity)
}

fn suppress_run_async_boundary_yield_checks(
    func: &mut LirFunction,
    run_async_funcs: &HashSet<LirFuncId>,
) {
    for block in &mut func.blocks {
        if let LirTerminator::Call {
            kind: CallKind::Direct { func_id },
            suppress_yield_check,
            ..
        } = &mut block.terminator
            && run_async_funcs.contains(func_id)
        {
            *suppress_yield_check = true;
        }
    }
}

fn helper_name(func: &LirFunction, helper_id: LirFuncId) -> String {
    format!(
        "{OUTLINE_HELPER_PREFIX}{}_{}",
        sanitize_name_fragment(&func.qualified_name),
        helper_id.0
    )
}

fn make_primcall_helper(id: LirFuncId, qualified_name: String, arity: usize) -> LirFunction {
    let params: Vec<LirVar> = (0..arity).map(|idx| LirVar(idx as u32)).collect();
    let result = LirVar(arity as u32);

    LirFunction {
        name: qualified_name.clone(),
        id,
        qualified_name,
        is_dict_def: false,
        params: params.clone(),
        blocks: vec![LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: vec![LirInstr::PrimCall {
                dst: Some(result),
                op: CorePrimOp::FiberRunAsyncWith,
                args: params,
            }],
            terminator: LirTerminator::Return(result),
        }],
        next_var: arity as u32 + 1,
        capture_vars: Vec::new(),
        param_reps: vec![FluxRep::TaggedRep; arity],
        result_rep: FluxRep::TaggedRep,
    }
}

fn make_direct_call_helper(
    id: LirFuncId,
    qualified_name: String,
    target_id: LirFuncId,
    arity: usize,
) -> LirFunction {
    let params: Vec<LirVar> = (0..arity).map(|idx| LirVar(idx as u32)).collect();
    let dummy_func = LirVar(arity as u32);

    LirFunction {
        name: qualified_name.clone(),
        id,
        qualified_name,
        is_dict_def: false,
        params: params.clone(),
        blocks: vec![LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: vec![LirInstr::Const {
                dst: dummy_func,
                value: LirConst::None,
            }],
            terminator: LirTerminator::TailCall {
                func: dummy_func,
                args: params,
                kind: CallKind::Direct { func_id: target_id },
            },
        }],
        next_var: arity as u32 + 1,
        capture_vars: Vec::new(),
        param_reps: vec![FluxRep::TaggedRep; arity],
        result_rep: FluxRep::TaggedRep,
    }
}

fn sanitize_name_fragment(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_async_call(idx: u32) -> LirInstr {
        LirInstr::PrimCall {
            dst: Some(LirVar(100 + idx)),
            op: CorePrimOp::FiberRunAsyncWith,
            args: vec![LirVar(0), LirVar(1), LirVar(2), LirVar(3)],
        }
    }

    fn empty_function(id: u32, name: &str) -> LirFunction {
        LirFunction {
            name: name.to_string(),
            id: LirFuncId(id),
            qualified_name: name.to_string(),
            is_dict_def: false,
            params: Vec::new(),
            blocks: vec![LirBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs: Vec::new(),
                terminator: LirTerminator::Return(LirVar(0)),
            }],
            next_var: 1,
            capture_vars: Vec::new(),
            param_reps: Vec::new(),
            result_rep: FluxRep::TaggedRep,
        }
    }

    fn count_run_async_primcalls(func: &LirFunction) -> usize {
        func.blocks
            .iter()
            .flat_map(|block| &block.instrs)
            .filter(|instr| {
                matches!(
                    instr,
                    LirInstr::PrimCall {
                        op: CorePrimOp::FiberRunAsyncWith,
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn outlines_ninth_run_async_with_primcall_site() {
        let mut program = LirProgram::new();
        let mut instrs = Vec::new();
        for idx in 0..9 {
            instrs.push(run_async_call(idx));
        }
        program.push_function(LirFunction {
            name: "main".to_string(),
            id: LirFuncId(1),
            qualified_name: "main".to_string(),
            is_dict_def: false,
            params: Vec::new(),
            blocks: vec![LirBlock {
                id: BlockId(0),
                params: Vec::new(),
                instrs,
                terminator: LirTerminator::Return(LirVar(108)),
            }],
            next_var: 109,
            capture_vars: Vec::new(),
            param_reps: Vec::new(),
            result_rep: FluxRep::TaggedRep,
        });

        outline_excess_run_async_with_sites_in_place(&mut program);

        assert_eq!(program.functions.len(), 2);
        let main = &program.functions[0];
        let helper = &program.functions[1];
        assert_eq!(
            count_run_async_primcalls(main),
            RUN_ASYNC_WITH_INLINE_SITE_LIMIT
        );
        assert_eq!(count_run_async_primcalls(helper), 1);
        assert!(is_run_async_with_outline_helper(helper));
        assert!(matches!(
            main.blocks[0].terminator,
            LirTerminator::Call {
                kind: CallKind::Direct { .. },
                ..
            }
        ));
        assert!(matches!(
            helper.blocks[0].terminator,
            LirTerminator::Return(_)
        ));
    }

    #[test]
    fn outlines_ninth_direct_call_to_run_async_wrapper() {
        let mut program = LirProgram::new();
        let mut prim = empty_function(10, "run_async_with_prim");
        prim.params = vec![LirVar(0), LirVar(1), LirVar(2), LirVar(3)];
        prim.blocks[0].instrs = vec![LirInstr::PrimCall {
            dst: Some(LirVar(4)),
            op: CorePrimOp::FiberRunAsyncWith,
            args: prim.params.clone(),
        }];
        prim.blocks[0].terminator = LirTerminator::Return(LirVar(4));
        prim.next_var = 5;
        program.push_function(prim);

        let mut main = empty_function(1, "main");
        main.blocks.clear();
        for idx in 0..9 {
            main.blocks.push(LirBlock {
                id: BlockId(idx),
                params: if idx == 0 {
                    Vec::new()
                } else {
                    vec![LirVar(100 + idx - 1)]
                },
                instrs: vec![LirInstr::Const {
                    dst: LirVar(200 + idx),
                    value: LirConst::None,
                }],
                terminator: if idx == 8 {
                    LirTerminator::Call {
                        dst: LirVar(108),
                        func: LirVar(200 + idx),
                        args: vec![LirVar(0), LirVar(1), LirVar(2), LirVar(3)],
                        cont: BlockId(9),
                        kind: CallKind::Direct {
                            func_id: LirFuncId(10),
                        },
                        suppress_yield_check: false,
                        yield_cont: None,
                    }
                } else {
                    LirTerminator::Call {
                        dst: LirVar(100 + idx),
                        func: LirVar(200 + idx),
                        args: vec![LirVar(0), LirVar(1), LirVar(2), LirVar(3)],
                        cont: BlockId(idx + 1),
                        kind: CallKind::Direct {
                            func_id: LirFuncId(10),
                        },
                        suppress_yield_check: false,
                        yield_cont: None,
                    }
                },
            });
        }
        main.blocks.push(LirBlock {
            id: BlockId(9),
            params: vec![LirVar(108)],
            instrs: Vec::new(),
            terminator: LirTerminator::Return(LirVar(108)),
        });
        main.next_var = 300;
        program.push_function(main);

        outline_excess_run_async_with_sites_in_place(&mut program);

        assert_eq!(program.functions.len(), 3);
        let main = &program.functions[1];
        let helper = &program.functions[2];
        assert!(is_run_async_with_outline_helper(helper));
        assert!(matches!(
            main.blocks[8].terminator,
            LirTerminator::Call {
                kind: CallKind::Direct { func_id },
                ..
            } if func_id == helper.id
        ));
        assert!(matches!(
            helper.blocks[0].terminator,
            LirTerminator::TailCall {
                kind: CallKind::Direct {
                    func_id: LirFuncId(10)
                },
                ..
            }
        ));
    }
}
