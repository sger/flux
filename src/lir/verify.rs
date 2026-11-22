//! Structural checks on LIR (proposal 0190 §14), so a pass that breaks the
//! IR is named where it breaks it rather than seen later as a wrong program
//! or an LLVM error.
//!
//! LIR has no static types: every value is a tagged word, and the runtime
//! checks a word's tag before counting it. So the reference-counting rule
//! here is narrower than "only on heap values": a `dup` of a tagged `Int` is
//! legal, and what this rejects is counting a word LIR itself made raw.
//!
//! Definitions before uses are checked on reachable blocks only:
//! `promote_tail_calls` leaves a promoted call's continuation behind, with
//! a parameter nothing defines any more.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use super::liveness::{instr_def, instr_uses, terminator_successors, terminator_uses};
use super::{
    BlockId, CallKind, LirFuncId, LirFunction, LirInstr, LirProgram, LirTerminator, LirVar,
};

/// The rule a [`LirVerifyError`] broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// A function has no blocks, or `blocks[i].id` is not `i`.
    Blocks,
    /// A branch, call continuation or match arm names a block that does not
    /// exist.
    Target,
    /// A call, closure or yield continuation names a function that does not
    /// exist.
    Callee,
    /// A known call or closure passes a different number of values than its
    /// callee takes.
    Arity,
    /// A variable is defined twice.
    Redefined,
    /// A variable is read on a path where nothing defined it.
    UseBeforeDef,
    /// A reference-counting operation is applied to a raw word.
    RcOnRaw,
    /// An edge into a block with parameters does not write one of them: the
    /// emitter's phi for it would have no value from that predecessor.
    Join,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::Blocks => "blocks",
            Rule::Target => "target",
            Rule::Callee => "callee",
            Rule::Arity => "arity",
            Rule::Redefined => "redefined",
            Rule::UseBeforeDef => "use before definition",
            Rule::RcOnRaw => "reference count on a raw word",
            Rule::Join => "join",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LirVerifyError {
    pub function: String,
    pub block: Option<BlockId>,
    pub rule: Rule,
    pub message: String,
}

impl fmt::Display for LirVerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.block {
            Some(block) => write!(
                f,
                "[{}] in `{}`, block {}: {}",
                self.rule.name(),
                self.function,
                block.0,
                self.message
            ),
            None => write!(f, "[{}] in `{}`: {}", self.rule.name(), self.function, self.message),
        }
    }
}

/// Every error in `program`, or `Ok` when there are none.
pub fn verify_program(program: &LirProgram) -> Result<(), Vec<LirVerifyError>> {
    let mut errors = Vec::new();
    for function in &program.functions {
        verify_function(program, function, &mut errors);
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

thread_local! {
    /// The first stage after which the program failed [`verify_program`],
    /// under `--verify-each`.
    static STAGE_FAILURE: std::cell::RefCell<Option<(&'static str, Vec<LirVerifyError>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Under `--verify-each`, verifies `program` after `stage` and keeps the
/// first failure. Lowering and the LIR passes return plain programs, so the
/// caller that can return an error reports it ([`take_stage_failure`]).
pub fn check_stage(program: &LirProgram, stage: &'static str) {
    if !crate::shared::verify_each::is_active() {
        return;
    }
    STAGE_FAILURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none()
            && let Err(errors) = verify_program(program)
        {
            *slot = Some((stage, errors));
        }
    });
}

/// The failure [`check_stage`] kept since the last call, as an `E997`
/// diagnostic naming the stage.
pub fn take_stage_failure() -> Option<crate::diagnostics::Diagnostic> {
    use crate::diagnostics::{
        DiagnosticBuilder, DiagnosticPhase, compiler_errors::IR_VERIFY_FAILURE, diagnostic_for,
    };
    let (stage, errors) = STAGE_FAILURE.with(|slot| slot.borrow_mut().take())?;
    let detail: Vec<String> = errors.iter().map(|error| format!("  {error}")).collect();
    Some(
        diagnostic_for(&IR_VERIFY_FAILURE)
            .with_message(format!(
                "The LIR is invalid after `{stage}`, {} violation(s):\n{}",
                errors.len(),
                detail.join("\n")
            ))
            .with_phase(DiagnosticPhase::Validation),
    )
}

fn verify_function(program: &LirProgram, function: &LirFunction, errors: &mut Vec<LirVerifyError>) {
    let mut report = |block: Option<BlockId>, rule: Rule, message: String| {
        errors.push(LirVerifyError { function: function.name.clone(), block, rule, message });
    };
    if function.blocks.is_empty() {
        report(None, Rule::Blocks, "the function has no blocks".into());
        return;
    }
    for (index, block) in function.blocks.iter().enumerate() {
        if block.id.0 as usize != index {
            report(
                Some(block.id),
                Rule::Blocks,
                format!("block {} is at position {index}", block.id.0),
            );
        }
    }
    let count = function.blocks.len();
    let exists = |target: BlockId| (target.0 as usize) < count;

    for block in &function.blocks {
        let at = Some(block.id);
        for target in terminator_successors(&block.terminator) {
            if !exists(target) {
                report(at, Rule::Target, format!("block {} does not exist", target.0));
            }
        }
        for instr in &block.instrs {
            if let LirInstr::MakeClosure { func_id, captures, .. } = instr {
                match program.func_by_id(*func_id) {
                    None => report(at, Rule::Callee, missing(*func_id, "a closure")),
                    Some(callee) if callee.capture_vars.len() != captures.len() => report(
                        at,
                        Rule::Arity,
                        format!(
                            "a closure of `{}` captures {} value(s), and it takes {}",
                            callee.name,
                            captures.len(),
                            callee.capture_vars.len()
                        ),
                    ),
                    Some(_) => {}
                }
            }
        }
        let (args, kind) = match &block.terminator {
            LirTerminator::TailCall { args, kind, .. } => (args, kind),
            LirTerminator::Call { args, kind, yield_cont, .. } => {
                if let Some(cont) = yield_cont
                    && program.func_by_id(cont.func).is_none()
                {
                    report(at, Rule::Callee, missing(cont.func, "a yield continuation"));
                }
                (args, kind)
            }
            _ => continue,
        };
        let (func_id, captures) = match kind {
            CallKind::Direct { func_id } => (*func_id, 0),
            CallKind::DirectClosure { func_id, captures } => (*func_id, captures.len()),
            _ => continue,
        };
        match program.func_by_id(func_id) {
            None => report(at, Rule::Callee, missing(func_id, "a call")),
            Some(callee)
                if callee.params.len() != args.len() || callee.capture_vars.len() != captures =>
            {
                report(
                    at,
                    Rule::Arity,
                    format!(
                        "a call to `{}` passes {} argument(s) and {captures} capture(s); it takes {} and {}",
                        callee.name,
                        args.len(),
                        callee.params.len(),
                        callee.capture_vars.len()
                    ),
                )
            }
            Some(_) => {}
        }
    }

    check_definitions(function, &mut report);
    check_uses(function, &exists, &mut report);
    check_counting(function, &mut report);
}

fn missing(id: LirFuncId, what: &str) -> String {
    format!("{what} names function {}, which the program does not have", id.0)
}

/// What a block has on entry besides what its predecessors leave: its
/// parameters, the fields a match arm binds there, and the result of a call
/// that continues there.
fn entry_definitions(function: &LirFunction) -> Vec<HashSet<LirVar>> {
    let mut entry: Vec<HashSet<LirVar>> =
        function.blocks.iter().map(|block| block.params.iter().copied().collect()).collect();
    for block in &function.blocks {
        match &block.terminator {
            LirTerminator::MatchCtor { arms, .. } => {
                for arm in arms {
                    if let Some(set) = entry.get_mut(arm.target.0 as usize) {
                        set.extend(arm.field_binders.iter().copied());
                    }
                }
            }
            LirTerminator::Call { dst, cont, .. } => {
                if let Some(set) = entry.get_mut(cont.0 as usize) {
                    set.insert(*dst);
                }
            }
            _ => {}
        }
    }
    entry
}

/// Each variable has one definition. A block parameter is the exception:
/// each predecessor writes it with a `Copy` (the emitter turns those into a
/// phi), and a call's result may be its continuation's parameter.
fn check_definitions(
    function: &LirFunction,
    report: &mut impl FnMut(Option<BlockId>, Rule, String),
) {
    let joined: HashSet<LirVar> = entry_definitions(function).into_iter().flatten().collect();
    let mut seen: HashMap<LirVar, Option<BlockId>> = HashMap::new();
    let mut define = |var: LirVar,
                      at: Option<BlockId>,
                      report: &mut dyn FnMut(Option<BlockId>, Rule, String)| {
        if let Some(first) = seen.insert(var, at) {
            let first = match first {
                Some(block) => format!("block {}", block.0),
                None => "the function's entry".to_string(),
            };
            report(at, Rule::Redefined, format!("v{} is defined again; first in {first}", var.0));
        }
    };
    for var in function.params.iter().chain(&function.capture_vars) {
        define(*var, None, report);
    }
    for block in &function.blocks {
        for instr in &block.instrs {
            let Some(var) = instr_def(instr) else { continue };
            let bridges = matches!(instr, LirInstr::Copy { .. }) && joined.contains(&var);
            if !bridges {
                define(var, Some(block.id), report);
            }
        }
    }
    // A variable the entry of a block defines must not also be defined by an
    // instruction. It may be the function's own parameter: `cont_split` names
    // a continuation's resume parameter after the block it resumes in, so the
    // copied blocks read it with no `Copy`.
    let entry: HashSet<LirVar> =
        function.params.iter().chain(&function.capture_vars).copied().collect();
    for block in &function.blocks {
        for var in &block.params {
            if seen.contains_key(var) && !entry.contains(var) {
                report(
                    Some(block.id),
                    Rule::Redefined,
                    format!("v{} is a block parameter and is also defined elsewhere", var.0),
                );
            }
        }
    }
}

/// Every variable read on a reachable block is defined on every path to the
/// read: a forward must-analysis, starting from the function's parameters and
/// captures.
fn check_uses(
    function: &LirFunction,
    exists: &impl Fn(BlockId) -> bool,
    report: &mut impl FnMut(Option<BlockId>, Rule, String),
) {
    let count = function.blocks.len();
    let successors: Vec<Vec<usize>> = function
        .blocks
        .iter()
        .map(|block| {
            terminator_successors(&block.terminator)
                .into_iter()
                .filter(|target| exists(*target))
                .map(|target| target.0 as usize)
                .collect()
        })
        .collect();
    let mut reachable = vec![false; count];
    let mut queue = VecDeque::from([0]);
    reachable[0] = true;
    while let Some(index) = queue.pop_front() {
        for &next in &successors[index] {
            if !reachable[next] {
                reachable[next] = true;
                queue.push_back(next);
            }
        }
    }
    let mut predecessors = vec![Vec::new(); count];
    for (index, nexts) in successors.iter().enumerate() {
        if reachable[index] {
            for &next in nexts {
                predecessors[next].push(index);
            }
        }
    }
    check_joins(function, &reachable, report);
    let entry = entry_definitions(function);
    let generated: Vec<HashSet<LirVar>> = function
        .blocks
        .iter()
        .zip(&entry)
        .map(|(block, entry)| {
            let mut set = entry.clone();
            set.extend(block.instrs.iter().filter_map(instr_def));
            set
        })
        .collect();
    let start: HashSet<LirVar> =
        function.params.iter().chain(&function.capture_vars).copied().collect();

    // `None` is "everything", the top of the lattice, for blocks not yet
    // reached by the iteration.
    let mut out: Vec<Option<HashSet<LirVar>>> = vec![None; count];
    let mut defined_in: Vec<HashSet<LirVar>> = vec![HashSet::new(); count];
    let mut changed = true;
    while changed {
        changed = false;
        for index in (0..count).filter(|index| reachable[*index]) {
            let mut incoming = if index == 0 { Some(start.clone()) } else { None };
            for &pred in &predecessors[index] {
                if let Some(pred_out) = &out[pred] {
                    incoming = Some(match incoming {
                        None => pred_out.clone(),
                        Some(set) => set.intersection(pred_out).copied().collect(),
                    });
                }
            }
            let Some(incoming) = incoming else { continue };
            let mut next = incoming.clone();
            next.extend(generated[index].iter().copied());
            if out[index].as_ref() != Some(&next) {
                out[index] = Some(next);
                defined_in[index] = incoming;
                changed = true;
            }
        }
    }

    for (index, block) in function.blocks.iter().enumerate() {
        if !reachable[index] {
            continue;
        }
        let mut defined = defined_in[index].clone();
        defined.extend(entry[index].iter().copied());
        let mut check = |var: LirVar, what: &str, defined: &HashSet<LirVar>| {
            if !defined.contains(&var) {
                report(
                    Some(block.id),
                    Rule::UseBeforeDef,
                    format!("v{} is read by {what} on a path where nothing defined it", var.0),
                );
            }
        };
        for instr in &block.instrs {
            for var in instr_uses(instr) {
                check(var, "an instruction", &defined);
            }
            if let Some(var) = instr_def(instr) {
                defined.insert(var);
            }
        }
        for var in terminator_uses(&block.terminator) {
            check(var, "the terminator", &defined);
        }
        // `lent` is every variable lowering had in scope, which `cont_split`
        // narrows to the live ones; only the references the frame owns must
        // exist on this path.
        if let LirTerminator::Call { held: Some(held), .. } = &block.terminator {
            for var in held.owned.iter().chain(&held.scoped) {
                check(*var, "what the call holds across", &defined);
            }
        }
    }
}

/// Each reachable edge into a block with parameters writes every parameter:
/// with a `Copy` in the predecessor (the emitter makes a phi of those), as
/// the result of a call that continues there, or as a field a match arm
/// binds there. A parameter that is the function's own is exempt: it is
/// `cont_split`'s resume parameter, defined on entry.
fn check_joins(
    function: &LirFunction,
    reachable: &[bool],
    report: &mut impl FnMut(Option<BlockId>, Rule, String),
) {
    let own: HashSet<LirVar> =
        function.params.iter().chain(&function.capture_vars).copied().collect();
    for (index, block) in function.blocks.iter().enumerate() {
        if !reachable[index] {
            continue;
        }
        let copied: HashSet<LirVar> = block
            .instrs
            .iter()
            .filter_map(|instr| match instr {
                LirInstr::Copy { dst, .. } => Some(*dst),
                _ => None,
            })
            .collect();
        let mut targets = terminator_successors(&block.terminator);
        targets.sort_by_key(|target| target.0);
        targets.dedup();
        for target in targets {
            let Some(next) = function.blocks.get(target.0 as usize) else { continue };
            for param in &next.params {
                let written = own.contains(param)
                    || copied.contains(param)
                    || match &block.terminator {
                        LirTerminator::Call { dst, cont, .. } => *cont == target && dst == param,
                        LirTerminator::MatchCtor { arms, .. } => arms
                            .iter()
                            .any(|arm| arm.target == target && arm.field_binders.contains(param)),
                        _ => false,
                    };
                if !written {
                    report(
                        Some(block.id),
                        Rule::Join,
                        format!(
                            "the edge to block {} does not write its parameter v{}",
                            target.0, param.0
                        ),
                    );
                }
            }
        }
    }
}

/// `dup`, `drop`, `FreeShell`, `IsUnique` and `DropReuse` never take a word
/// LIR made raw: an untagged integer, float or boolean, a raw pointer, a
/// constructor tag, or the result of raw arithmetic. The runtime would read
/// it as a pointer.
fn check_counting(function: &LirFunction, report: &mut impl FnMut(Option<BlockId>, Rule, String)) {
    let mut raw: HashSet<LirVar> = HashSet::new();
    for block in &function.blocks {
        for instr in &block.instrs {
            if let Some(var) = raw_result(instr) {
                raw.insert(var);
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for block in &function.blocks {
            for instr in &block.instrs {
                if let LirInstr::Copy { dst, src } = instr
                    && raw.contains(src)
                    && raw.insert(*dst)
                {
                    changed = true;
                }
            }
        }
    }
    for block in &function.blocks {
        for instr in &block.instrs {
            let (val, what) = match instr {
                LirInstr::Dup { val } => (val, "dup"),
                LirInstr::Drop { val } => (val, "drop"),
                LirInstr::FreeShell { val } => (val, "FreeShell"),
                LirInstr::IsUnique { val, .. } => (val, "IsUnique"),
                LirInstr::DropReuse { val, .. } => (val, "DropReuse"),
                _ => continue,
            };
            if raw.contains(val) {
                report(
                    Some(block.id),
                    Rule::RcOnRaw,
                    format!("{what} of v{}, which is raw", val.0),
                );
            }
        }
    }
}

fn raw_result(instr: &LirInstr) -> Option<LirVar> {
    match instr {
        LirInstr::Alloc { dst, .. }
        | LirInstr::UntagInt { dst, .. }
        | LirInstr::UntagFloat { dst, .. }
        | LirInstr::UntagBool { dst, .. }
        | LirInstr::UntagPtr { dst, .. }
        | LirInstr::GetTag { dst, .. }
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
        | LirInstr::DropReuse { dst, .. } => Some(*dst),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::FluxRep;
    use crate::lir::{CtorArm, CtorTag, HeldAcross, LirBlock, LirConst};

    fn function(id: u32, name: &str, params: &[u32], blocks: Vec<LirBlock>) -> LirFunction {
        LirFunction {
            debug: None,
            name: name.to_string(),
            id: LirFuncId(id),
            qualified_name: name.to_string(),
            is_dict_def: false,
            params: params.iter().map(|p| LirVar(*p)).collect(),
            blocks,
            next_var: 100,
            capture_vars: Vec::new(),
            param_reps: Vec::new(),
            result_rep: FluxRep::TaggedRep,
        }
    }

    fn block(
        id: u32,
        params: &[u32],
        instrs: Vec<LirInstr>,
        terminator: LirTerminator,
    ) -> LirBlock {
        LirBlock {
            id: BlockId(id),
            params: params.iter().map(|p| LirVar(*p)).collect(),
            instrs,
            terminator,
            locs: Vec::new(),
            term_loc: Default::default(),
        }
    }

    fn v(n: u32) -> LirVar {
        LirVar(n)
    }

    fn constant(dst: u32) -> LirInstr {
        LirInstr::Const { dst: v(dst), value: LirConst::Int(1) }
    }

    fn program(functions: Vec<LirFunction>) -> LirProgram {
        let mut program = LirProgram::new();
        for function in functions {
            program.push_function(function);
        }
        program
    }

    fn rules(program: &LirProgram) -> Vec<Rule> {
        match verify_program(program) {
            Ok(()) => Vec::new(),
            Err(errors) => errors.into_iter().map(|e| e.rule).collect(),
        }
    }

    fn call(dst: u32, func: u32, args: &[u32], cont: u32, kind: CallKind) -> LirTerminator {
        LirTerminator::Call {
            dst: v(dst),
            func: v(func),
            args: args.iter().map(|a| v(*a)).collect(),
            cont: BlockId(cont),
            kind,
            suppress_yield_check: false,
            yield_cont: None,
            held: None,
        }
    }

    /// `f(x) = if x { g(x) } else { x }`, joined through a `Copy` into the
    /// join block's parameter, plus a match whose arm binds a field.
    fn valid() -> LirProgram {
        let g =
            function(2, "g", &[0], vec![block(0, &[], Vec::new(), LirTerminator::Return(v(0)))]);
        let f = function(
            1,
            "f",
            &[0],
            vec![
                block(
                    0,
                    &[],
                    vec![LirInstr::UntagBool { dst: v(1), val: v(0) }],
                    LirTerminator::Branch {
                        cond: v(1),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                ),
                block(
                    1,
                    &[],
                    vec![LirInstr::Const { dst: v(2), value: LirConst::None }],
                    call(3, 2, &[0], 3, CallKind::Direct { func_id: LirFuncId(2) }),
                ),
                block(
                    2,
                    &[],
                    vec![LirInstr::Copy { dst: v(4), src: v(0) }],
                    LirTerminator::Jump(BlockId(4)),
                ),
                block(
                    3,
                    &[3],
                    vec![LirInstr::Copy { dst: v(4), src: v(3) }],
                    LirTerminator::Jump(BlockId(4)),
                ),
                block(
                    4,
                    &[4],
                    vec![LirInstr::Dup { val: v(4) }],
                    LirTerminator::MatchCtor {
                        scrutinee: v(4),
                        arms: vec![CtorArm {
                            tag: CtorTag::Some,
                            field_binders: vec![v(5)],
                            target: BlockId(5),
                        }],
                        default: BlockId(6),
                    },
                ),
                block(5, &[], Vec::new(), LirTerminator::Return(v(5))),
                block(6, &[], Vec::new(), LirTerminator::Return(v(4))),
            ],
        );
        program(vec![f, g])
    }

    #[test]
    fn a_join_a_call_and_a_match_pass() {
        assert_eq!(rules(&valid()), Vec::<Rule>::new());
    }

    #[test]
    fn a_block_out_of_place_is_reported() {
        let mut program = valid();
        program.functions[0].blocks[5].id = BlockId(9);
        assert!(rules(&program).contains(&Rule::Blocks));
    }

    #[test]
    fn a_function_without_blocks_is_reported() {
        let program = program(vec![function(1, "f", &[], Vec::new())]);
        assert_eq!(rules(&program), vec![Rule::Blocks]);
    }

    #[test]
    fn a_jump_to_a_missing_block_is_reported() {
        let mut program = valid();
        program.functions[0].blocks[2].terminator = LirTerminator::Jump(BlockId(40));
        assert!(rules(&program).contains(&Rule::Target));
    }

    #[test]
    fn a_call_to_a_missing_function_is_reported() {
        let mut program = valid();
        program.functions[0].blocks[1].terminator =
            call(3, 2, &[0], 3, CallKind::Direct { func_id: LirFuncId(77) });
        assert_eq!(rules(&program), vec![Rule::Callee]);
    }

    #[test]
    fn a_call_with_the_wrong_number_of_arguments_is_reported() {
        let mut program = valid();
        program.functions[0].blocks[1].terminator =
            call(3, 2, &[0, 0], 3, CallKind::Direct { func_id: LirFuncId(2) });
        assert_eq!(rules(&program), vec![Rule::Arity]);
    }

    #[test]
    fn a_closure_with_the_wrong_captures_is_reported() {
        let mut program = valid();
        program.functions[0].blocks[1].instrs.push(LirInstr::MakeClosure {
            dst: v(20),
            func_id: LirFuncId(2),
            captures: vec![v(0)],
        });
        assert_eq!(rules(&program), vec![Rule::Arity]);
    }

    #[test]
    fn a_variable_defined_twice_is_reported() {
        let mut program = valid();
        program.functions[0].blocks[1].instrs.push(LirInstr::UntagBool { dst: v(1), val: v(0) });
        assert!(rules(&program).contains(&Rule::Redefined));
    }

    #[test]
    fn a_variable_defined_on_one_path_only_is_reported() {
        let mut program = valid();
        // v2 is defined in block 1 only; block 4 is reached from 2 as well.
        program.functions[0].blocks[4].instrs.push(LirInstr::Drop { val: v(2) });
        assert_eq!(rules(&program), vec![Rule::UseBeforeDef]);
    }

    #[test]
    fn a_variable_read_before_its_definition_is_reported() {
        let mut program = valid();
        let instrs = &mut program.functions[0].blocks[1].instrs;
        instrs.insert(0, LirInstr::Dup { val: v(2) });
        assert_eq!(rules(&program), vec![Rule::UseBeforeDef]);
    }

    #[test]
    fn what_a_call_holds_across_must_be_defined() {
        let mut program = valid();
        if let LirTerminator::Call { held, .. } = &mut program.functions[0].blocks[1].terminator {
            *held = Some(Box::new(HeldAcross { owned: vec![v(30)], ..HeldAcross::default() }));
        }
        assert_eq!(rules(&program), vec![Rule::UseBeforeDef]);
    }

    #[test]
    fn an_unreachable_block_is_not_checked_for_definitions() {
        // A promoted tail call's old continuation reads its parameter, which
        // nothing defines any more.
        let g =
            function(2, "g", &[0], vec![block(0, &[], Vec::new(), LirTerminator::Return(v(0)))]);
        let f = function(
            1,
            "f",
            &[0],
            vec![
                block(
                    0,
                    &[],
                    vec![constant(1)],
                    LirTerminator::TailCall {
                        func: v(1),
                        args: vec![v(0)],
                        kind: CallKind::Direct { func_id: LirFuncId(2) },
                    },
                ),
                block(1, &[], vec![LirInstr::Drop { val: v(9) }], LirTerminator::Return(v(9))),
            ],
        );
        assert_eq!(rules(&program(vec![f, g])), Vec::<Rule>::new());
    }

    #[test]
    fn counting_a_raw_word_is_reported() {
        let mut program = valid();
        // v1 is the raw boolean `UntagBool` made; a copy of it is raw too.
        let instrs = &mut program.functions[0].blocks[2].instrs;
        instrs.push(LirInstr::Copy { dst: v(21), src: v(1) });
        instrs.push(LirInstr::Drop { val: v(21) });
        assert_eq!(rules(&program), vec![Rule::RcOnRaw]);
    }

    #[test]
    fn a_loop_is_checked_to_a_fixed_point() {
        // block 1 loops on itself, reading v1 that block 0 defines.
        let f = function(
            1,
            "f",
            &[0],
            vec![
                block(0, &[], vec![constant(1)], LirTerminator::Jump(BlockId(1))),
                block(
                    1,
                    &[],
                    vec![LirInstr::Dup { val: v(1) }],
                    LirTerminator::Branch {
                        cond: v(0),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                ),
                block(2, &[], Vec::new(), LirTerminator::Return(v(1))),
            ],
        );
        assert_eq!(rules(&program(vec![f])), Vec::<Rule>::new());
    }

    #[test]
    fn a_block_parameter_may_be_the_functions_own_parameter() {
        // A continuation `cont_split` made: its resume parameter v4 is named
        // after the parameter of the block it resumes in.
        let f = function(
            1,
            "f$cont$0",
            &[4],
            vec![
                block(0, &[], Vec::new(), LirTerminator::Jump(BlockId(1))),
                block(1, &[4], Vec::new(), LirTerminator::Return(v(4))),
            ],
        );
        assert_eq!(rules(&program(vec![f])), Vec::<Rule>::new());
    }

    #[test]
    fn a_block_parameter_an_instruction_also_defines_is_reported() {
        let f = function(
            1,
            "f",
            &[0],
            vec![
                block(0, &[], vec![constant(4)], LirTerminator::Jump(BlockId(1))),
                block(1, &[4], Vec::new(), LirTerminator::Return(v(4))),
            ],
        );
        assert!(rules(&program(vec![f])).contains(&Rule::Redefined));
    }

    #[test]
    fn verify_each_names_the_lir_stage_that_broke_the_program() {
        let mut broken = valid();
        broken.functions[0].blocks[2].terminator = LirTerminator::Jump(BlockId(40));
        check_stage(&broken, "promote_tail_calls");
        assert!(take_stage_failure().is_none(), "nothing is checked without the flag");
        crate::shared::verify_each::with_enabled(|| {
            check_stage(&valid(), "lowering");
            check_stage(&broken, "promote_tail_calls");
            check_stage(&broken, "cont_split");
            let error = take_stage_failure().expect("the broken program is kept");
            assert_eq!(error.code(), Some("E997"));
            let message = error.message().unwrap_or_default();
            assert!(message.contains("after `promote_tail_calls`"), "{message}");
            assert!(message.contains("[target]"), "{message}");
        });
    }

    #[test]
    fn an_edge_that_does_not_write_a_join_parameter_is_reported() {
        let mut program = valid();
        // Block 2 jumps to the join (block 4) without its `Copy` into v4.
        program.functions[0].blocks[2].instrs.clear();
        assert_eq!(rules(&program), vec![Rule::Join]);
    }
}
