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
  continuation is captured (Koka's `under1`).
- **Part B.** Every other clause runs at its own handler's prompt. A prompt
  that sees a yield for another handler wraps the continuation captured so
  far, re-entering itself around it, and passes the yield outwards (Koka's
  `prompt` and `yield-cont`). The inline service of such yields, slice
  5-tr-nested, goes.

The VM already gives the semantics these parts give. It changes only where
it agreed with native code on KI-243.

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
  tail-resumptive clause in place keeps such a loop flat.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Nothing changes for Flux programmers. Programs that print the wrong value
natively today (KI-238, KI-243) print what the VM prints. Programs that
crash (KI-244) run.

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
- The native flag's rule, `resumes_only_as_last_act`
  (`aether/tail_resumptive.rs`), is looser. It accepts a branch that never
  resumes, which a clause running in place can't express, because it can't
  leave its handle except by performing.
- **KI-233's exclusion goes for in-place clauses.** A clause that performs
  its own effect can be in-place, because running under its handle's
  evidence already takes the perform to the next handler out.
- A handle whose clauses are all in-place is an *in-place handle*. Its
  evidence entry gets a third flag value beside the native flag's two, so
  the runtime can tell an in-place handle from one that only resumes last.

**Native.**
- A perform whose handler entry is in-place calls the clause where the
  perform was, with `current_evv` set to the entry's parent
  (`EVV_PARENT_OFF`). `resume` is the identity closure, or a closure that
  sets the entry's state cell.
- After the clause, `current_evv` goes back to the perform's.
- **If the clause yields** (it performed something a capturing handler
  serves), the perform extends a continuation that re-enters the in-place
  call: it runs the rest of the clause under the parent evidence, then
  restores the perform's. That is Koka's `under1x`.
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

The first is smaller and is proposed.

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
- installs w1′ = w0′ plus this handle's entry;
- runs `inner(x)` under w1′;
- runs the prompt loop on what that gives (a value, or a yield it now
  serves), and restores w0′ on the way out.

This is Koka's `w1' = if evv-eq(w0, w0') then w1 else evv-insert(w0', ev)`.

**Evidence.** It is rebuilt from the outside in as prompts are re-entered.
So a delimiting `resume` installs only its own handler's level, not the
whole yield-site evidence it captures today.

**State, KI-236.**
- **A handle outside the resumed computation** is not re-entered: its
  prompt is still on the stack. Its state cell is shared, so a `set` while
  one branch runs is seen by the next.
- **A handle inside the resumed computation** is re-entered by every resume
  of it. Its re-entry closure keeps the state the handle had at the yield,
  and each re-entry installs a fresh cell with that value. That is what
  Koka's `prompt-local-var` does (`loc := v` before the resume), and it
  replaces `evv_copy_inner_states`.

**LIR.** A prompt that passes a yield outwards returns the sentinel. So the
code after a `handle` in the same function needs a continuation and a yield
check. The prompt becomes a call terminator, `CallKind::Prompt`, that calls
`flux_yield_prompt` and that `cont_split` treats as a call that may yield.
- A prompt whose continuation only returns its result needs none, as
  KI-241's rule gives for calls into Flux code. `main`'s chain of default
  handlers is that case.
- The enclosing handles' evidence, held across the prompt for KI-133's
  landing pad, moves to the call's `HeldAcross::scoped`.

**Effects.**
- **KI-238:** a clause runs at its own prompt with the whole continuation.
- **KI-244:** no frames are left under a resume.
- The escaped resume of `.verify/ki227/10` gets its `Ask` served by `main`'s
  prompt.

### Part C: async

The async marker (`FLUX_ASYNC_MARKER`) is a foreign yield like any other.
Today it passes prompts without leaving anything behind, and a fiber
resumed inside a `handle` runs without that handle's prompt. Performs there
are then served inline by whatever prompt is around. With part B the
fiber's continuation re-enters each prompt it passed. That makes a
suspended fiber resume under its handlers as the VM's does. Every async,
task, channel and fs fixture is the check.

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
  terminator, evidence is rebuilt on re-entry, and KI-236's state follows
  the re-entry closure. It fixes KI-238.
- **S3, part C:** async through prompts, with the fixtures it moves.

Each stage goes on its own branch: tests first, then the fix, then
`cargo xtask verify`.

## Drawbacks
[drawbacks]: #drawbacks

- **Every native program with nested handlers changes how it runs** in S2.
  Today's inline service is fast in the common case where an outer clause
  resumes as its last act. Each foreign yield will allocate a re-entry
  closure and compose the frames under it.
- **Part A adds an evidence switch** and a restore to every in-place perform,
  and a branch on the entry's flag to every perform.
- **The runtime has a third kind of clause call.** In-place joins the plain
  composition and the delimiting trampoline. Each needs its own tests.

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
- **Stack switching, as OCaml 5's fibers do.** One-shot, it fits neither
  multi-shot resume nor the yield-check design the native backend is built
  on (proposal 0162).

## Prior art
[prior-art]: #prior-art

- **Koka.**
  - `lib/std/core/hnd.kk`: `prompt` wraps a foreign yield with `yield-cont`
    and re-inserts its evidence on re-entry. `under1` and `under1x` run an
    operation in place under the handler's evidence. `prompt-local-var`
    restores a local's value before each resume.
  - The comment at `clause(resume)` in `prompt` notes the stack the clause
    uses inside the prompt: "we should exit prompt first ... to use constant
    stack space when resuming".
  - Xie and Leijen, *Generalized Evidence Passing for Effect Handlers* (ICFP
    2021).
- **OCaml 5:** deep handlers on one-shot fibers. A clause runs outside its
  handler, and `continue k (perform e)` reaches the handler outside, which
  is handler_sieve's shape.
- **The Flux VM:** captured continuations put back at the handler's entry
  for a tail resume, `saved_evv` for a clause's evidence (KI-233), and
  `resume_boundary` (KI-237).

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **The third value of the native flag.** Today's flag can't mark in-place
  separately from "resumes only as its last act, with branches that don't".
  Either a third value, or the in-place rule replaces the looser one and
  those clauses capture.
- **How the native debugger shows an in-place clause.** It would appear as a
  frame on top of the perform, where today it is the prompt's. The debug
  suite will say whether stepping and backtraces need work.
- **What the evidence switch costs** per in-place perform, against today's
  yield and compose, on countdown, iterator and counter.
- **Async in S3:** whether a fiber suspended inside a `handle` and resumed on
  another worker needs anything more than the re-entry closure.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Koka's TODO for clauses inside the prompt.** Running a non-in-place
  clause after unwinding its prompt would make every resume constant in C
  stack.
- **The evidence pass could keep its direct calls** for clauses that perform,
  with an explicit evidence switch in Core, if S1's timings show the
  runtime's in-place branch costs more than the pass saves.
