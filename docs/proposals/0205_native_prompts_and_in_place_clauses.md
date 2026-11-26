- Feature Name: Native prompts that wrap a foreign yield, and tail-resumptive clauses run in place
- Start Date: 2026-11-26
- Status: Draft
- Proposal PR:
- Flux Issue: KI-238, KI-243, KI-244

## Summary
[summary]: #summary

Three native issues have one cause: where a handler clause runs, and what a
prompt does with a yield meant for another handler.

- **KI-238:** an outer clause's work after `resume` lands in the wrong place.
- **KI-243:** a tail-resumptive clause sees handlers it shouldn't.
- **KI-244:** a perform forwarded from a clause leaves C stack behind.

This proposal makes the native runtime follow Koka's design in two parts:

- **Part A.** A clause that resumes exactly once, as its last act, runs in
  place at the perform site, under the evidence of its own `handle`. No
  continuation is captured. This is Koka's `under1`, which Koka uses for
  clauses written with `fun` or `val`. Flux finds these clauses by analysis,
  not from a keyword.
- **Part B.** Every other clause runs at its own handler's prompt. A prompt
  that sees a yield for another handler wraps the continuation captured so
  far, re-entering itself around it, and passes the yield outwards (Koka's
  `prompt` and `yield-cont`). The inline service of such yields, slice
  5-tr-nested, goes.

The VM already gives the semantics these parts give. It changes only where
it agreed with native code on KI-243.

Koka, OCaml 5 and GHC all give the rule this proposal relies on: a clause
runs outside its own handler, and resuming puts back the handlers inside the
continuation (see [Prior art](#prior-art)).

## Motivation
[motivation]: #motivation

### Where things stand

A native `handle` installs an evidence entry and runs its body. When the body
yields, `flux_yield_prompt` (`runtime/c/effects.c`) loops over the yields
that reach it:

- **A yield for its own handler:** it composes the frames captured so far
  into a `resume` and runs the clause.
  - For a clause that resumes only as its last act, the native flag
    `EVV_TAIL_OFF`, the `resume` is a plain composition (`flux_compose_conts`),
    and the clause runs under the yield site's evidence.
  - For any other, the `resume` is a delimiting trampoline that installs the
    evidence it captured and re-enters the prompt (KI-225). The clause runs
    under the handle's evidence (KI-233).
- **A yield for another handler** whose marker is in the yield-site
  evidence: it runs that handler's clause itself, inline ("slice
  5-tr-nested"). The comment gives the reason: the other prompt's C frame
  "may have already unwound".

### What goes wrong

- **KI-238, wrong values.** The inline service gives the outer clause a
  `resume` holding only the frames up to the inner prompt, and its result
  becomes the inner handle's. Work after `resume` is applied at the wrong
  place.
  - `.verify/ki227/09` should print 120; native code prints 60.
  - An escaped resume (`.verify/ki227/10`) traps.
  - `effect_clause_forwards_to_an_aborting_outer_ki233.flx` prints 1007,
    not 7.
- **KI-244, stack overflow.** What the outer clause's `resume` runs, the rest
  of the program, runs on top of the inline service's frames, which never
  unwind. Every forwarded perform adds a layer, and 160,000–320,000 steps
  crash with SIGBUS. handler_sieve, in which each prime's handler forwards to
  the previous one, crashes from about 2,500 (`-O`). The VM runs it to
  10,000 in 3.2 s.
- **KI-243, the wrong handler.** A tail-resumptive clause runs under the
  yield site's evidence, so a perform in it reaches a handler installed
  between its `handle` and the perform. Both backends agree on the wrong
  answer: the VM copies native code's rule so that the two print the same.

### What an attempt showed (2026-11-26, `.verify/ki238/outward_prompt_wip.patch`)

Making a prompt pass a foreign yield outwards, with a continuation of its
own added after the frames below it, fixed KI-238's three cases. It broke
five other fixtures natively: `effect_tr_nested` and the KI-227, KI-236 and
KI-237 fixtures among them.

- **The continuation has to wrap, not follow.** A frame below the prompt
  that yields to the inner handler again has to meet the re-entered prompt
  on its way out. Run after those frames, the prompt never sees the yield.
- **Re-installing a prompt touches KI-236.** Its evidence has to agree with
  KI-236's per-resume state copies.
- **A forwarding loop grows even when wrapped right.** Each turn nests the
  clause's pending prompt around the next resume, so the crash becomes
  quadratic time: 0.01 s at 2,000 turns and 0.24 s at 8,000. Only running a
  tail-resumptive clause in place keeps such a loop flat. Koka has the same
  limit, for the same reason (see [Part B](#part-b-prompts-that-wrap-a-foreign-yield)).

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Nothing changes for Flux programmers. Programs that print the wrong value
natively today (KI-238, KI-243) print what the VM prints. Programs that
crash (KI-244) run, when their forwarding clauses resume as their last act.

For compiler and runtime contributors, two rules:

1. **A clause runs in its handle's context.** A perform in a clause reaches
   the handlers outside its `handle`, never its own handler or one installed
   between the `handle` and the perform. This holds on every path, in place
   or captured, VM or native.
2. **A clause that resumes exactly once, as its last act, runs in place.** It
   is called where the perform was, as a function, with the evidence switched
   to its handle's for the duration. Its `resume` returns its argument, and
   for a parameterized handler it sets the state. Nothing is captured unless
   the clause itself yields, and then only its own frames are.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Part A: in-place tail-resumptive clauses

**Which clauses.** A clause is *in-place* when every path through it ends
in exactly one call of `resume`, and `resume` appears nowhere else.
- This is Core's tail-resumptive rule (`core/passes/tail_resumptive.rs`).
  It is what Koka's `fun` clause means: Koka builds it with `clause-tail`,
  "resumes exactly once at the end".
- **Koka chooses by keyword; Flux infers.** Koka turns `fun` and `val` into
  `clause-tail` and every `ctl` into a yield to the prompt, even a `ctl`
  that ends in `resume(x)`. Its resume analysis is not used. Flux has no
  such keywords, so it keeps the analysis.
- The native flag's rule, `resumes_only_as_last_act`
  (`aether/tail_resumptive.rs`), is looser. It accepts a branch that never
  resumes, which a clause running in place can't express, because it can't
  leave its handle except by performing.
- **KI-233's exclusion goes for in-place clauses.** A clause that performs
  its own effect can be in-place, because running under its handle's
  evidence already takes the perform to the next handler out. Koka's
  `cancelation-scope` (`lib/std/async/async.kk`) relies on this: in its
  `override handle`, `fun is-scope-canceled` performs `is-scope-canceled`,
  which reaches the enclosing scope's handler.
- A handle whose clauses are all in-place is an *in-place handle*. Its
  evidence entry needs a flag value of its own (see
  [Unresolved questions](#unresolved-questions)).

**Clauses that perform nothing.** An in-place clause that performs no
effect needs no evidence switch: nothing in it reads the evidence. Koka's
simplifier turns `clause-tail` into `clause-tail-noop` in that case, which
calls the clause with no switch at all. Flux does the same: such a clause is
called directly, with no switch and no restore. countdown's, iterator's and
counter's clauses are of this kind, so the switch costs them nothing.

**Native.**
- A perform whose handler entry is in-place calls the clause where the
  perform was, with `current_evv` set to the entry's parent
  (`EVV_PARENT_OFF`). `resume` is the identity closure, or a closure that
  sets the entry's state cell.
- After the clause, `current_evv` goes back to the perform's.
- **If the clause yields** (it performed something a capturing handler
  serves), the perform extends a continuation that re-enters the in-place
  call. That is Koka's `under1x`. When it is resumed, it reads the evidence
  it is called under, sets the entry's parent, runs the rest of the clause,
  and restores what it read. It does not restore the evidence of the
  original perform, which is stale after an escaped or async resume.
- **The perform site.** Today it is a `CallKind::YieldTo` terminator that
  always yields. It becomes a call that may return a value, with the usual
  yield check after it. `flux_perform_direct` is the starting point, without
  its E1200/E1201 resume counting, which an in-place clause can't trip.

**VM.** `OpPerform` gets the same branch:
- an in-place handler's arm is called as a frame on top of the perform,
  with the identity `resume`, under the handler's `saved_evv`;
- the perform's evidence is restored when the arm returns, through a
  boundary like KI-237's `resume_boundary`.

The VM's `tail_resumptive` field, which copies the native flag so that both
backends answer KI-243 the same way, is replaced by the in-place flag.

**Core.** The evidence pass calls a tail-resumptive clause directly at a
`perform` written in the handle's body. That call needs the same evidence
switch. There are two choices:
- the pass leaves alone any clause that performs at all, which the runtime
  then runs in place;
- the pass wraps its call in an evidence switch.

The first is smaller and is proposed. It keeps the direct call exactly where
Koka drops the switch, for clauses that perform nothing.

**Effects.**
- **KI-243:** an in-place clause runs under its handle's evidence.
- **KI-244:** handler_sieve's clauses are in-place, so a forwarded test is a
  chain of nested calls, as deep as the chain, unwound when the test
  returns. countdown, iterator and counter stay flat, as their clauses are
  in-place too.

### Part B: prompts that wrap a foreign yield

**The prompt.** When `flux_yield_prompt` sees a yield whose marker isn't its
own:
1. It composes the continuations captured so far, the frames between the
   yield and this prompt, into one closure, `inner`.
2. It extends a *prompt re-entry* closure that holds `inner`, the marker and
   the handle's entry. The closure is Koka's `yield-cont(fn(cont, res) ...)`
   in `prompt`.
3. It returns the sentinel, with `current_evv` restored to the handle's
   parent.

The inline service goes, and with it the evidence choice KI-233 added for
it.

**Re-entry.** Called with `x`, the closure:
- reads the evidence it is called under, w0′ (the resumer's, outside this
  handler);
- installs w1′ = w0′ plus this handle's entry, with the entry's parent
  (`EVV_PARENT_OFF`) set to w0′;
- runs `inner(x)` under w1′;
- runs the prompt loop on what that gives (a value, or a yield it now
  serves), and restores w0′ on the way out.

This is Koka's `w1' = if evv-eq(w0, w0') then w1 else evv-insert(w0', ev)`.

**The entry's parent follows the re-entry.** Koka's `evv-insert` sets the
handler's parent evidence (`ev.hevv`) on every insert, "at init, or due to
non-scoped resumptions". Every rule that reads the parent depends on it: a
clause at the prompt runs under it (KI-233), and an in-place clause switches
to it (Part A). Flux copies an entry's words into each evidence array, so the
re-entry writes w0′ into the copy it inserts, and the entry the handle was
installed with is left alone. Without that, a clause reached after an
escaped or async resume would run under the evidence of the place the handle
was installed.

**Evidence.** It is rebuilt from the outside in as prompts are re-entered.
So a delimiting `resume` installs only its own handler's level, not the
whole yield-site evidence it captures today.

**State, KI-236.**
- **A handle outside the resumed computation** is not re-entered: its
  prompt is still on the stack. Its state cell is shared, so a `set` while
  one branch runs is seen by the next.
- **A handle inside the resumed computation** is re-entered by every resume
  of it. Its re-entry closure keeps the value the state had at the yield,
  and each re-entry writes that value back into the entry's cell before it
  runs `inner`. That is Koka's `prompt-local-var`: it saves `v = loc` at the
  yield, and its re-entry does `loc := v`, into the same cell. A yield that
  later passes the re-entered prompt saves the value as it is then.
- The cell already lives in the entry (`EVV_STATE_OFF`), shared by every
  copy of it, so write-back allocates nothing. It replaces
  `evv_copy_inner_states`.

**LIR.** A prompt that passes a yield outwards returns the sentinel. So the
code after a `handle` in the same function needs a continuation and a yield
check. The prompt becomes a call terminator, `CallKind::Prompt`, that calls
`flux_yield_prompt` and that `cont_split` treats as a call that may yield.
- A prompt whose continuation only returns its result needs none, as
  KI-241's rule gives for calls into Flux code. `main`'s chain of default
  handlers is that case.
- The enclosing handles' evidence, held across the prompt for KI-133's
  landing pad, moves to the call's `HeldAcross::scoped`.

**Composed continuations resumed twice.** A continuation that holds a
re-entry closure may be resumed more than once. Koka's `kcompose` runs each
part in order and, when the composed closure is shared, copies each part
before it calls it, so the captured chain is never used up. GHC copies the
captured stack on every resume for the same reason. Flux's composition does
the same: it consumes the chain only when it holds the only reference.

**Effects.**
- **KI-238:** a clause runs at its own prompt with the whole continuation.
- **KI-244:** no frames are left under a resume by a foreign yield.
- The escaped resume of `.verify/ki227/10` gets its `Ask` served by `main`'s
  prompt.

**What part B does not fix.** A clause still runs inside its prompt's C
frame, as in Koka, where `clause(resume)` carries the comment "TODO: we
should exit prompt first, and then execute clause to use constant stack
space when resuming". Koka composes continuations by nested calls, with no
trampoline, so a resume there rebuilds as much C stack as it captured. So a
forwarding loop of clauses that are not in-place still grows, in Koka as in
Flux: the attempt's quadratic time. KI-244 is fixed for in-place clauses,
which handler_sieve's are. A clause that does work after its `resume` and
forwards in a loop keeps a size limit (see [Testing](#testing) and
[Rationale](#rationale-and-alternatives)).

### Part C: async

The async marker (`FLUX_ASYNC_MARKER`) is a foreign yield like any other.
Today it passes prompts without leaving anything behind, and a fiber
resumed inside a `handle` runs without that handle's prompt. Performs there
are then served inline by whatever prompt is around. With part B the
fiber's continuation re-enters each prompt it passed. That makes a
suspended fiber resume under its handlers as the VM's does.

Koka's async works this way. Its scheduler is a stack of ordinary handlers,
and `do-await` hands its resume to the event loop. The event loop calls that
resume under evidence of its own, so each prompt the fiber passed is
re-entered through its `Yielding` closure, and `evv-insert` gives each entry
the event loop's context as its parent. Part C depends on that last step in
part B: a fiber resumed by the scheduler must not see the evidence of the
place where it suspended.

Every async, task, channel and fs fixture is the check.

### Testing

- **The tests committed for KI-238 and KI-244** (`fea7aefa`, branch
  `fix/ki238`):
  - `effect_outer_clause_works_after_resume_ki238.flx` (120, 11);
  - `effect_forwarded_performs_ki244.flx` (200000);
  - `effect_clause_forwards_to_an_aborting_outer_ki233.flx`, without its
    skip-way (7).
- **The fixtures the attempt broke:** `effect_tr_nested`,
  `effect_multishot_restores_inner_handlers_ki227`,
  `effect_handler_state_threads_through_resumes_ki236`,
  `effect_resume_then_outer_perform_ki237` and
  `arith_raw_ints_in_handlers_and_closures`.
- **KI-243:** `.verify/ki233/05` should print 1 on both backends, which
  becomes a fixture.
- **New fixtures, from the other implementations:**
  - **A non-tail resume, then a perform to the outer handler**, in the shape
    of OCaml's `testsuite/tests/backtrace/backtrace_effects_nested.ml`: an
    inner clause does `1 + resume(())`, and the resumed body then performs
    to the outer handler, which resumes. OCaml prints 43.
  - **Multi-shot through nested prompts**, in the shape of GHC's
    `testsuite/tests/rts/continuations/cont_nondet_handler.hs`: a choice
    effect performed inside another handler's body, resumed twice, with
    every combination printed.
  - **The entry's parent after a resume from elsewhere:** an in-place
    clause that performs, reached through a resume that escaped its clause,
    and again through an async resume. Its perform reaches the handler
    outside its `handle` at the resume, not where the handle was installed.
  - **A forwarding loop of clauses that are not in-place**, at a size that
    runs, with the limit part B leaves recorded in a known issue.
- **The probes:** `.verify/ki238`, `.verify/ki244` (chains of `d` handlers
  forwarding `t` performs) and `.verify/hs`.
- **handler_sieve** at 60,000 natively (171848738).
- **Every effect, async, task, channel and fs fixture** on `vm, llvm,
  llvm_opt, llvm_heap`, and the native debugger suite.
- **Timings** of the effects benchmarks before and after each stage,
  natively at their full sizes. More than 10% slower on any of them is a
  stop.

### Stages

- **S1, part A:** in-place clauses on both backends and in Core. It fixes
  KI-243 and KI-244 and unblocks handler_sieve. It doesn't depend on part B,
  as in-place clauses never go through the inline service.
- **S2, part B:** prompts wrap foreign yields, the prompt becomes a call
  terminator, evidence is rebuilt on re-entry with the entry's parent set,
  and KI-236's state is written back on re-entry. It fixes KI-238. The
  in-place clause's parent update lands here too, with its test.
- **S3, part C:** async through prompts, with the fixtures it moves.

Each stage goes on its own branch: tests first, then the fix, then
`cargo xtask verify`.

## Drawbacks
[drawbacks]: #drawbacks

- **Every native program with nested handlers changes how it runs** in S2.
  Today's inline service is fast in the common case where an outer clause
  resumes as its last act. Each foreign yield will allocate a re-entry
  closure and compose the frames under it. Koka pays the same.
- **Part A adds an evidence switch** and a restore to each in-place perform
  whose clause performs, and a branch on the entry's flag to every perform.
  Clauses that perform nothing skip the switch.
- **The runtime has a third kind of clause call.** In-place joins the plain
  composition and the delimiting trampoline. Each needs its own tests.
- **A forwarding loop of clauses that are not in-place still grows.** Part B
  keeps the clause inside its prompt's frame, as Koka does. Only GHC's and
  OCaml's way, below, removes that.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Keep the inline service and fix KI-244 alone,** for example by running
  the resumed code after unwinding the service frames. It would leave
  KI-238 and the escaped-resume trap. It also keeps a path whose comment
  says it exists for a prompt that has unwound, which part B makes
  impossible.
- **A prompt continuation that follows the frames below it,** the attempt.
  It is wrong whenever those frames yield again.
- **Flatten a tail `resume` against its clause's pending prompt,** as the
  VM's tail resume puts the continuation back at the handler's entry. It
  would keep a forwarding loop flat without part A. But it needs the
  composed continuation to know that a re-entry closure follows a tail call
  of `resume`. Part A gives the same for the clauses that matter and fixes
  KI-243 as well.
- **Run a clause after its prompt has returned.** This is how GHC and OCaml
  keep every forwarding loop flat, whatever the clause does.
  - GHC's `control0#` removes the prompt and runs the clause where the
    prompt was. A deep handler pushes the prompt again inside `resume`.
  - OCaml runs the clause on the parent fiber, in place of the call that
    installed or resumed the handler, and a `continue` in tail position is
    a jump.
  - In a runtime built on yield checks, the prompt would have to return to
    the code at the `handle`, which would call the clause in a loop and
    re-enter the prompt from `resume`. That is Koka's TODO. It is a larger
    change to the prompt and to LIR than part B, and part A already covers
    the clauses that forward in practice. It stays a future possibility.
- **Stack switching, as OCaml 5's fibers do.**
  - **What doesn't fit:** it needs stacks the runtime manages, a calling
    convention the LLVM backend and C calls into the runtime don't have, and
    a different design from the yield checks that native code is built on
    (proposal 0162). OCaml's continuations are one-shot, but that is a
    choice, not a limit of fibers. Its runtime copied fibers to clone a
    continuation from 2016 until 2021, and dropped that for cost and for
    linear resources. Multi-shot on fibers means copying stacks, as GHC does.
  - **What carries over:**
    - The handler lives in the captured segment, so resuming puts it back.
      Part B's re-entry closure does the same for a runtime without fibers.
    - The performing chain is detached before the clause runs, which is
      KI-243's rule.
    - A handler that declines appends its fiber to the continuation and
      jumps outwards; it never serves the effect itself. OCaml's append
      changes the chain in place, which only one-shot continuations allow.
      Flux's re-entry closures are the multi-shot form of it.

## Prior art
[prior-art]: #prior-art

The versions read are koka `2d0d6f4851`, ocaml `bc1c440270` and ghc
`56bb91d95f`.

- **Koka.**
  - `lib/std/core/hnd.kk`:
    - `prompt` (lines 384-418): its `Yielding` arm wraps a foreign yield
      with `yield-cont` and re-inserts its evidence on re-entry. Its `Yield`
      arm restores the evidence outside the handler (`evv-set(w0)`) and then
      calls the clause.
    - `under1` and `under1x` (lines 588-603) run an operation in place under
      the handler's evidence. `under1x` re-reads the evidence when resumed.
    - The clause builders (lines 655-667): `clause-control1` yields to the
      prompt; `clause-tail1` calls `under1`; `clause-tail-noop1` calls the
      operation with no evidence switch.
    - `prompt-local-var` (lines 469-472) writes a local's saved value back
      before each resume.
    - The comment at `clause(resume)`, line 418, is the TODO quoted in part
      B.
  - `lib/std/core/inline/hnd.c`:
    - `kk_evv_insert` (lines 140-146) sets the handler's parent evidence on
      every insert.
    - `kcompose` (lines 266-306) runs composed continuations by nested
      calls, copying them when they are shared.
  - `src/Type/Infer.hs`, lines 846-866, maps `fun` and `val` to
    `clause-tail` and `ctl` to `clause-control`. `src/Core/Simplify.hs`,
    lines 463-465, turns `clause-tail` into `clause-tail-noop` for a clause
    that performs nothing.
  - `lib/std/async/async.kk`, lines 266-287: the async scheduler as
    ordinary handlers.
  - Xie and Leijen, *Generalized Evidence Passing for Effect Handlers* (ICFP
    2021).
- **OCaml 5.**
  - Deep handlers on one-shot fibers (`runtime/amd64.S`: `caml_perform`,
    `caml_reperform`, `caml_resume`). A clause runs on the parent fiber,
    outside its handler, and `continue k (perform e)` reaches the handler
    outside, which is handler_sieve's shape. A declining handler forwards in
    constant time and stack.
  - `testsuite/tests/backtrace/backtrace_effects_nested.ml`: a clause's work
    after `continue` belongs to the outer continuation, KI-238's shape.
  - The manual on one-shot continuations
    (`manual/src/refman/extensions/effects.etex`, lines 571-580), and
    `caml_clone_continuation`, removed in `6b33e61c97` (2021).
  - Sivaramakrishnan et al., *Retrofitting Effect Handlers onto OCaml* (PLDI
    2021).
- **GHC.**
  - The primops `prompt#` and `control0#`
    (`compiler/GHC/Builtin/primops.txt.pp`, lines 3085-3109): `control0#`
    runs its function with the prompt removed, so the clause runs outside
    its handler.
  - `rts/Continuation.c` and `rts/ContinuationOps.cmm`: capture copies the
    stack up to the matching prompt, inner prompts included, and every
    resume copies it back, leaving the captured continuation as it was.
  - `testsuite/tests/rts/continuations/cont_nondet_handler.hs`, line 17: a
    deep handler pushes its prompt again inside `resume`
    (`prompt tag . k`), which is part B's re-entry.
- **The Flux VM:** captured continuations put back at the handler's entry
  for a tail resume, `saved_evv` for a clause's evidence (KI-233), and
  `resume_boundary` (KI-237).

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **The native flag.** Today's flag can't mark in-place separately from
  "resumes only as its last act, with branches that don't". Koka's
  precedent is to have only the in-place rule: a clause that is not in-place
  captures. That is proposed, with a third flag value kept only if S1's
  timings show clauses with a non-resuming branch, which today keep a loop
  flat, getting slower.
- **How the native debugger shows an in-place clause.** It would appear as a
  frame on top of the perform, where today it is the prompt's. The debug
  suite will say whether stepping and backtraces need work.
- **What the evidence switch costs** per in-place perform whose clause
  performs, against today's yield and compose. Clauses that perform nothing
  don't switch.
- **Async in S3:** whether a fiber suspended inside a `handle` and resumed on
  another worker needs anything more than the re-entry closure. State
  written back into a shared cell assumes one resume of a continuation runs
  at a time. If one continuation could run on two workers at once, each
  re-entry would need a fresh cell, as `evv_copy_inner_states` gives today.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Run every clause after its prompt has returned**, as GHC and OCaml do
  and as Koka's TODO asks. It would make every resume constant in C stack,
  whatever the clause does after `resume`.
- **The evidence pass could keep its direct calls** for clauses that perform,
  with an explicit evidence switch in Core, if S1's timings show the
  runtime's in-place branch costs more than the pass saves.
