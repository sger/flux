# Debug parity fixtures

Each `.flx` file here scripts one debugging session in its header. The
session must read the same in the VM debugger and in lldb (proposal 0188 S6):

```flux
// dbg: break 15
// dbg: continue
// dbg: locals
// dbg: step
// dbg: bt
// dbg: finish
// dbg: print xs.1
// dbg: delete 15
// dbg: continue
```

Run them with:

```sh
flux parity-check tests/parity_debug --ways vm_debug,llvm_debug
cargo test --features llvm --test debug parity_debug_tests::
```

## How a session runs

- `vm_debug` runs the commands with `flux debug <file> --script`.
- `llvm_debug` builds the file with `--native -g` and runs the same commands
  in `lldb -b`, through the `flux` command from `flux_lldb.py`:
  - `flux next`, `flux step` and `flux finish` step as the VM does, across
    performs too;
  - `continue` first drops a step that a breakpoint interrupted;
  - after each command that runs the program, `flux where` says where it
    stopped.
  
  The session starts with `flux catch panic division unhandled fibers`, the
  VM's default. It runs the program with `FLUX_WORKERS=1`, because the VM
  debugger runs async code on one worker.
- `llvm_debug` reports a skip on a machine without lldb, with an lldb that
  can't run Python, and on Windows, whose debug builds carry CodeView for
  Visual Studio and cdb (0188 S11).
- Besides `break`, `delete`, `continue`, `step`, `next`, `finish`, `bt`,
  `locals` and `print`, a fixture can use `handlers`, `catch <kinds>`,
  `threads` and `thread N` (0188 S8).

## What is compared

Both transcripts are reduced to lines that mean the same thing on both
debuggers:

- stops and frames, as a function and a `file:line`, with ` thread N` added
  to a stop in a fiber other than thread 1;
- `exception <kind>` after a stop at a runtime error;
- handlers, as `handler Effect state = v`;
- variables, as `name = value`, with each group sorted by name;
- `thread N name [state] file:line` for each fiber `threads` lists;
- `finished` when the program ends.

A frame in a standard-library file (`Async.flx`, or any file whose name starts
with a capital) reads `<library>`, one line for a run of them, because the
backends shape those frames differently. The reduced transcript leaves out
columns, positions, stop reasons, the VM's tail-call labels, the
`⟂ handling …` labels, the `(suspended)` marks, where a handler was
installed, and the program's own output.

Stops and frames must match line for line, with one exception: native code
may show a handler's arm that the VM doesn't. An arm whose last call is
`resume` waits natively under the frames it resumes.

In each group of variables, every variable the VM shows must be there
natively, with the same value. Native code may still hold a value the VM has
moved out of its slot, so the native side may show more. A value native code
has freed (`<dropped>`) counts as not shown.

## Checks: `// dbg-check:`

Each fixture ends with `// dbg-check:` lines (proposal 0190 §5) that say what
each debugger must show. Each debugger is checked on its own, so a change that
breaks both in the same way still fails:

```flux
// dbg-check: stop <lambda closures.flx:13:31> closures.flx:14
// dbg-check: offset = 10 (captured)
// dbg-check: x = 1
// dbg-check: frame main closures.flx:13
```

- Each check is a whole line of the reduced transcript. Checks match in
  order, each on a later line than the one before it. Other lines may sit
  between them, which is what lets native code show more.
- They go at the end of the file. The script names line numbers, and lines
  added above the program would move it.
- `--bless` writes them from the VM debugger's transcript when both debuggers
  agree. After a change that is meant to alter what a session shows, bless
  again and read the diff:

  ```sh
  flux parity-check tests/parity_debug --ways vm_debug,llvm_debug --bless
  ```
- `parity_debug_tests` fails a fixture that has no checks.

## Writing a fixture

- Put the `// dbg:` lines in the header, and count lines from the top of the
  file. Bless the checks last. A breakpoint on a comment or a blank line
  moves to the next line with code on the VM, and resolves nowhere in lldb.
- Write one statement per line, because stepping goes by line.
- Look at values while the program still uses them. Both backends release a
  value after its last use.
- A function that prints needs `with IO`.
- With effects, look at values in frames that run, not in suspended ones,
  which have none natively. Don't look at `resume` either: it is a
  continuation on the VM and a closure natively. Let arms call `resume`
  themselves. An arm that tail-calls a helper loses its frame natively, and
  the frames it suspended go with it.
- A line that is only a value (`n`) has code on the VM but not natively, so
  step over it in neither.
- With async code, keep one fiber running at a time where the order matters,
  so both backends number and schedule fibers alike. `both` runs its
  children in order, and a channel's sender and receiver take turns. Timers
  keep counting while a session is stopped, so make a `timeout` long enough
  not to fire during it. Look at values in the fiber that stopped, since a
  waiting fiber's frames have no variables natively.
