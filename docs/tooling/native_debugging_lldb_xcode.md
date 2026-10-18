# Debugging native Flux programs with lldb and Xcode

A native build with `-g` carries DWARF (proposal 0188 S5 and S6). Debuggers
can then set breakpoints at `app.flx:12`, show Flux function names and lines
in backtraces, step through Flux source, and show variables. With
`flux_lldb.py` loaded, values print as Flux values (`[1, 2, 3]`,
`Some(4)`, `Point(1, 2)`), and the `flux` command shows frames and variables
the way `flux debug` does on the VM.

## Build

```sh
flux app.flx --native -g -o app       # build, run, and keep ./app
flux app.flx --native -g --emit-binary  # build ./app without running it
```

- On macOS, `-o` and `--emit-binary` also write `app.dSYM` next to the
  binary, so the debug information survives cleaning the build cache.
- On Linux, the DWARF is inside the binary.
- On Windows, the debug information is CodeView, in `app.pdb` next to
  `app.exe`, for Visual Studio, WinDbg and cdb. They show Flux call stacks
  with `.flx` lines; see [Visual Studio, WinDbg and cdb](#visual-studio-windbg-and-cdb-windows).

`-g` keeps the build's optimisation level and turns on frame pointers. Tail
calls stay tail calls, so deep recursion behaves as it does without `-g`.
`-g` does keep every `let` the program wrote, which a build without it may
inline away, so that the debugger can show it.

## Panic traces without a debugger

Every native build, with or without `-g`, carries a DWARF line table
(proposal 0188 S10a): functions and lines only, without variables. A program
that dies of an error (a panic, a division by zero, a failed `unwrap`, an
unhandled effect) walks its own stack and prints the Flux frames it was in,
as the VM does:

```text
panic: too big

Stack trace:
  at c (/path/to/app.flx:3:16)
  at b (/path/to/app.flx:7:5)
  at main (/path/to/app.flx:15:21)
  at <main>
```

`flux app.flx --native` renders this as its usual diagnostic, pointing at
the innermost frame's line. A program run on its own (`./app`) prints it as
above. A long trace keeps its first 20 frames and its last 10, with a count
of the ones between. A panic a `try` or `assert_throws` catches prints
nothing. Frames can differ from the VM's: a tail call leaves no frame, and
code inlined into another function shows as that function's line.

On macOS a plain build's line table stays in the build cache's object files,
which the binary names. A binary copied to another machine without them
prints `panic:` without its trace; a `-g` build's `app.dSYM` keeps it.

## The debug heap

A native program frees a value when its last reference goes. If the compiler
or the runtime gets a count wrong, the program goes on using memory it has
freed. The usual runtime rarely shows it: blocks from its arena are never
reused, so the stale value still reads as it was, and the program prints a
plausible answer.

`--debug-heap` links another build of the C runtime, `libflux_rt_debug.a`
(proposal 0188 S10c), which catches it:

- no arena: every block comes from `malloc`, with a prefix recording the
  stack where it was allocated;
- a freed block is never given back: its count stays 0, its contents are
  overwritten with `0xDEADDEAD…` (an odd word, so a stale field reads as an
  Int and never as a pointer), and the stack that freed it is recorded;
- the next dup, drop or reuse check of a freed block stops the program:

```sh
flux app.flx --native --debug-heap
```

```text
error[E1009]: use after free: copying a constructor value that was already freed (allocated at numbers (app.flx:2:5), freed at main (app.flx:15:16))

  app.flx:10:5
   |
10 |     sum(xs)
   |     ^^^^^^^

Stack trace:
  at total (app.flx:10:5)
  at main (app.flx:16:23)
  at <main>
```

The sites are the innermost Flux frame of each recorded stack. A `try`
does not catch the error, since the heap can no longer be trusted. Under
lldb or VS Code the program stops there first, as at a panic (`flux catch
panic`), with the stack of the bad copy or drop.

What it does not catch: a read of a freed block alone, with no count
operation. The read shows the poison (`0xDEADDEAD…`), in `print` and in
lldb. A block freed early and never touched again is not an error.

The cost: every allocation records its stack and nothing is ever freed, so a
debug-heap run is slower and its memory only grows. Use it on a reproducer,
not in production. It combines with `-g`, `--emit-binary` and `-o`; its
cached binary (`<name>.debug-heap.bin`) is kept apart from the plain one, so
a plain run never picks it up.

## The lldb script

`flux_lldb.py` teaches lldb to read Flux values.

- **macOS:** `-o` and `--emit-binary` put it in the dSYM, as
  `app.dSYM/Contents/Resources/Python/app.py`. lldb and Xcode load it once
  you allow scripts from dSYMs, which you do once in `~/.lldbinit`:

  ```text
  settings set target.load-script-from-symbol-file true
  ```

  Until then, lldb warns that the dSYM "contains an untrusted debug script"
  and tells you how to load it.
- **Anywhere:** write it out and import it:

  ```sh
  flux debug --lldb-script > flux_lldb.py
  lldb -o "command script import flux_lldb.py" ./app
  ```

## lldb

```text
$ lldb ./app
(lldb) breakpoint set --file app.flx --line 12
(lldb) run
(lldb) bt
* frame #0: app`square at app.flx:12:13
  frame #1: app`sum_squares at app.flx:7:14
  frame #2: app`main at app.flx:13:17
(lldb) thread step-over      # next Flux line
(lldb) thread step-in        # into the called Flux function
(lldb) thread step-out
```

With the script loaded:

```text
(lldb) flux locals
n = 7
name = "flux"
xs = [1, 2, 3]
shape = Square(3)
p = Point(1, 2)
offset = 10 (captured)
(lldb) flux print p
p = Point(1, 2)
  .0 = 1
  .1 = 2
(lldb) flux bt
#0 describe at app.flx:14:5
#1 main at app.flx:26:17
(lldb) frame variable shape
(FluxValue) shape = Square(3)
```

- `flux locals` shows the variables bound so far, the innermost of each
  name, and the captured ones last, marked `(captured)`.
- `flux print xs.2` follows parts by number, or a record's fields by name.
- `flux bt` shows Flux frames only, from the current one down to `main`.
  A lambda the compiler inlined where it was written, such as
  `(fn() { … })()`, is still a frame, marked `[inlined]`:

  ```text
  (lldb) flux bt
  #0 <lambda app.flx:4:14> at app.flx:6:9   [inlined]
  #1 main at app.flx:4:14
  ```

  Under `-g` the compiler keeps every binding the program wrote, so a lambda
  with a parameter stays a call of its own, as in `flux debug`.
- `frame variable` and the variables views of Xcode and VS Code use the same
  formatting, and expand lists, tuples, arrays and constructors.
- A value shows as `<dropped>` once the program has released it: native code
  frees a value at its last use, and the debugger does not read freed
  memory. The VM keeps such values, so `flux debug` still shows them.
- `flux refs xs` reads the value's reference count from its header: how
  many references the program holds, and whether it is unique, which is
  when the compiled code can reuse it in place (0188 S10b):

  ```text
  (lldb) flux refs xs
  xs: 3 references (shared)
  (lldb) flux refs pair
  pair: 1 reference (unique: can be reused in place)
  (lldb) flux refs n
  n: not reference counted (Int)
  ```

  A count shared between threads (a value sent on a channel or to a task)
  reads `shared across threads`. A debug slot holds no reference, so the
  count is the program's own. `flux debug`'s `refs` counts the VM's
  references instead, which can differ: the VM keeps a variable until its
  frame returns, where native code releases it at its last use. In VS Code,
  `.refs xs` in the Debug Console does the same on either backend.

### Effects

When a handler arm runs, the frames that performed the operation have
already returned: what is left of each is a continuation, which the arm
holds as `resume`. `flux bt` reads them from there and shows them below the
arm, as `flux debug` does:

```text
(lldb) flux bt
#0 <handler Flip.flip> at app.flx:17:25
   ⟂ handling Flip.flip — performed at app.flx:6:13
#1 decide at app.flx:6:13   (suspended)
#2 twice at app.flx:11:13   (suspended)
#3 main at app.flx:16:18
(lldb) flux handlers
#0 Flip
#1 Counter state = 5
```

- While `resume` runs them, the resumed frames show above the arm, and
  those still to run below the one running.
- `flux handlers` lists the active handlers, innermost first, with a
  parameterized handler's state. In an arm, the handlers inside it are
  suspended with the frames that performed, so they are not listed. The
  default handlers the compiler puts around `main` (Console, FileSystem, …)
  are left out.
- lldb's own `bt` shows the arm called from the runtime's
  `flux_yield_prompt`, and a resumed frame as `decide (resumed after 6:13)`.
- A suspended frame has no variables natively: they are in the
  continuation, which the debugger cannot take apart.
- A handler that never resumes still shows its frames as `(suspended)`:
  native code cannot know ahead that the arm will not call `resume`.
- An arm whose last call is `resume` stays on the stack under the frames it
  resumes. On the VM, a tail `resume` replaces the arm.

### Stepping

lldb's own steps treat a perform as a call that returns: `thread
step-over` on a perform line stops in the function that installed the
handler. `flux next`, `flux step` and `flux finish` step as `flux debug`
does:
- **`flux next` on a perform** runs the handler and stops at the next line of
  the same call, once `resume` brings it back; a handler that never resumes
  ends it at the handle site's next line.
- **`flux step` on a perform** stops in the handler's arm.
- **`flux step` on `resume(…)`** stops in the frame it resumes.
- Returning into a caller does not stop again on the line of its call.

VS Code's step buttons use them in native sessions.

### Runtime errors

```text
(lldb) flux catch panic division
stopping at: panic, division by zero
(lldb) run
(lldb) flux where
stopped: exception (division by zero) at app.flx:11:5 in ratio
(lldb) flux bt
#0 ratio at app.flx:11:5
#1 main at app.flx:17:21
```

`flux catch` takes `panic`, `division`, `unhandled` (effects), `errors` (the
runtime's other errors: a failed `unwrap`, a wrong argument count, …), `all`
or `none`; alone, it lists what it stops at. It breaks on the runtime's
`flux_debug_on_panic` and `flux_debug_on_unhandled_effect`, which the
runtime calls before it reports an error, and selects the Flux frame the
error happened in. A panic inside a `try` stops too, before it is caught.
`flux where` says where the program stopped, and why, in `flux debug`'s
words.

What you see:
- A breakpoint may resolve to two or three locations: the compiler emits a
  function's body more than once (boxed, unboxed and closure-capture
  variants). They are all the same Flux line.
- Without the script, `frame variable` shows raw 64-bit words: an Int `n`
  is `2n+1`, and other values are addresses. In an unboxed worker frame,
  Ints and Bools are raw.
- Lambdas are named `<lambda app.flx:4:34>`, handler clauses
  `<handler Effect.op>`. Code after a call that can suspend is
  `name (resumed after L:C)`. A tail-resumptive handler whose operation is
  performed in the function that handles it is also compiled as a lambda,
  called directly: the stack then shows `<lambda app.flx:L:C>` where
  `flux debug` shows `<handler Effect.op>`.
- Reference counting and other compiler-made code is at line 0, so stepping
  passes over it. So does the C runtime: `main(argc, argv)` in `flux_rt.c`
  is the frame below the Flux `main`.

### Fibers

Native fibers are not OS threads: lldb's `thread list` shows the runtime's
workers, and a fiber that waits is a continuation the scheduler holds.
`flux threads` lists the fibers as `flux debug` does, numbered in spawn
order from 1, the root fiber, with what each waits for:

```text
(lldb) flux where
stopped: breakpoint at app.flx:13:9 in fast (thread 3)
(lldb) flux threads
  1 main [waiting both (threads 2, 3)] at app.flx:31:16
  2 slow [waiting sleep 20 ms] at app.flx:6:5
* 3 fast [stopped] at app.flx:13:9
(lldb) flux thread 2
thread 2: slow [waiting sleep 20 ms]
(lldb) flux bt
#0 slow at app.flx:6:5   (suspended)
```

- `flux thread N` makes `flux bt` show thread N until the next stop, and
  `flux bt --thread N` shows it once. A waiting fiber's frames are read from
  its continuation, without variables; a fiber that has not started shows
  its function.
- `flux next`, `flux step` and `flux finish` stay in the fiber they start
  in: `flux next` over `sleep` or a channel `recv` lets the other fibers run
  and stops at the next line of the same fiber.
- `flux catch fibers` stops where `Async.fail` is raised, in the failing
  fiber, before `Async.try`, `race` or `timeout` recovers it. A failed root
  fiber stops once, not again when `run_async` fails with its error.
- Run the program with `FLUX_WORKERS=1` (`settings set target.env-vars
  FLUX_WORKERS=1`), as `flux debug` and VS Code do: with several workers,
  fibers run in parallel and the numbering follows whichever runs first.
- `flux threads` runs `flux_debug_fibers()` in the stopped program. It
  takes no lock it cannot get at once: if the program stopped inside the
  scheduler, `flux threads` shows only thread 1, the stopped one.
- A task (`Task.spawn`) runs on an OS thread of its own: it is in lldb's
  `thread list`, not in `flux threads`.

### Recording and replaying

A native program records and replays its effects on its own, without the
`flux` driver: set `FLUX_TAPE_RECORD` or `FLUX_TAPE_REPLAY` to a tape path.
The build embeds the program's hash, so the program writes a new tape's
header itself and refuses a tape recorded from another program, or from
this one before it changed (E1026), as `flux --replay` does.

```text
(lldb) settings set target.env-vars FLUX_WORKERS=1 FLUX_TAPE_REPLAY=/path/run.jsonl
(lldb) flux event 1
stopped: event 1 (clock_now()) at app.flx:4:18 in main
(lldb) flux events
#0 clock_now() (done)
#1 clock_now() (done)
#2 stdout("6299870\n") (next)
#3 stdout("true\n") (ahead)
#4 a normal exit (ahead)
(lldb) flux event 0
stopped: event 0 (clock_now()) at app.flx:3:17 in main
```

- `flux event N` runs to the effect that made event N and stops in the
  Flux frame that performed it, past the breakpoints on the way (not past a
  runtime error `flux catch` stops at). It starts the program if it is not
  running.
- An event the replay has passed is reached by running the replay again
  from the start: it reaches every event as it did before. In a recording,
  going back to an event is refused; replay the tape instead.
- `flux events` (`--json` for tools) lists the tape's events, or, in a
  recording, those recorded so far. It runs `flux_debug_tape_events()` in
  the stopped program, as `flux threads` does.
- Every event passes through `flux_debug_on_tape_event(seq)`, an empty
  function: a breakpoint there with a condition on `seq` stops at any event.

## VS Code

Set `"backend": "native"` in the launch configuration:

```json
{
  "type": "flux",
  "request": "launch",
  "name": "Debug main (native)",
  "program": "${workspaceFolder}/main.flx",
  "backend": "native"
}
```

The adapter builds the program with `--native -g`, runs it under `lldb-dap`
(Xcode's on macOS, `lldb-dap-18` or later on Linux, or the
`flux.lldbDapPath` setting), and shows the same Flux views as a VM session:
Flux frames, variables as Flux values, stepping through Flux lines, the
frames an effect handler suspended (without variables), `.handlers` in the
Debug Console, the runtime errors to stop at in the Breakpoints view, and
each fiber as a thread with what it waits for (`.threads` in the Debug
Console). It runs the program with `FLUX_WORKERS=1`. `"record"` and
`"replay"` work as on the VM, and the Flux Timeline goes forward to an
effect. Step Back needs the VM: open the same tape with `"backend": "vm"`. In this repository, set `flux.runCommand` to
`cargo run --features llvm --`, since native builds need the `llvm`
feature. See `crates/flux-dap/README.md`.

## Xcode

Xcode cannot build Flux itself, but it can debug a Flux binary through an
External Build System target.

1. **File → New → Project… → Other → External Build System.** Set the
   build tool to the `flux` binary (or `/usr/bin/env` with `flux` as its
   first argument).
2. In the target's **Info** tab, set the arguments to
   `app.flx --native -g --emit-binary`, and the directory to the folder
   holding `app.flx`.
3. **Product → Scheme → Edit Scheme… → Run → Info:** set the executable to
   the built `app`.
4. Add `app.flx` to the project, so Xcode can show it and take breakpoints
   in it.

### Manual checklist

Run this after changing native debug information:

1. ⌘B builds, and `app` and `app.dSYM` appear next to `app.flx`.
2. A breakpoint set by clicking the gutter of `app.flx` turns solid
   (resolved) once the program starts.
3. ⌘R stops at it, with the line highlighted in `app.flx`.
4. The Debug navigator's call stack shows the Flux function names, with
   `main` and the runtime's `main` below them.
5. Clicking a caller's frame opens `app.flx` at the line that made the call.
6. Step Over (F6), Step Into (F7) and Step Out (F8) move through Flux lines,
   not reference-counting code.
7. A breakpoint inside a lambda stops there, and the frame is
   `<lambda app.flx:L:C>`.
8. A program with a type class shows its dictionary functions only when
   stepping into them, and never stops there on its own.
9. With `settings set target.load-script-from-symbol-file true` in
   `~/.lldbinit`, the Variables view shows Flux values (`[1, 2, 3]`,
   `Point(1, 2)`), not raw numbers, and a variable not bound yet is absent.
10. A list, tuple or constructor in the Variables view expands into its
    parts; a record's parts carry its field names.
11. `flux locals` typed in Xcode's lldb console prints the frame's
    variables.
12. With the script loaded, a breakpoint in a handler arm and `flux bt` in
    the lldb console show the suspended frames below the arm; Xcode's own
    call stack shows the arm above `flux_yield_prompt`.
13. `flux catch panic`, then ⌘R, stops a panicking program with the Flux
    frame that panicked selected.

## Visual Studio, WinDbg and cdb (Windows)

On Windows a `-g` build writes `app.pdb` next to `app.exe`: CodeView, made
from the same metadata as the DWARF above (0188 S11). Visual Studio, WinDbg
and cdb read it and show Flux function names and `.flx` lines in call
stacks. A run without `-o` keeps its PDB in the build cache, next to the
cached binary.

```sh
flux app.flx --native -g --emit-binary -o app.exe   # app.exe and app.pdb
```

This is call stacks only. Variables show as raw 64-bit words (the
`FluxValue` typedef), `flux_lldb.py` has no Windows counterpart, and stepping
is the debugger's own, which does not know about effects or fibers. For
those, debug in VS Code (`flux-dap`), on the VM or through lldb.

### cdb and WinDbg

A Flux panic ends the process, so stop where the runtime reports it,
`flux_debug_on_panic`:

```text
cdb -lines -c "bp app!flux_debug_on_panic; g; k; q" app.exe
```

`k` lists the Flux frames, as `app!check+0x2a [C:\src\app.flx @ 4]`.
`bp app!main` and `bu` on a Flux name work too, as in a C program. cdb comes
with the Windows SDK's *Debugging Tools for Windows*.

### Visual Studio

1. **File → Open → Project/Solution…**, pick `app.exe`. Visual Studio makes
   an EXE project around it.
2. Open `app.flx` (**File → Open → File…**) and set breakpoints in it.
3. **Debug → Start Debugging** (F5).

### Manual checklist

Run this on Windows after changing native debug information:

1. `flux app.flx --native -g --emit-binary -o app.exe` writes `app.exe` and
   `app.pdb`.
2. A breakpoint on a line of `app.flx` turns solid once the program starts,
   and F5 stops there with the line highlighted.
3. The **Call Stack** window shows the Flux function names (`main`,
   `<lambda app.flx:L:C>`), each with `app.flx` and its line.
4. Double-clicking a caller's frame opens `app.flx` at the line that made the
   call.
5. A breakpoint on `flux_debug_on_panic` (**Debug → New Breakpoint →
   Function Breakpoint**) stops a panicking program with the Flux frame that
   panicked below the runtime's frames.
6. In a program of two modules, frames from each module open their own
   `.flx` file.
