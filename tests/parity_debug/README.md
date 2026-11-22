# Debug parity fixtures

Each `.flx` file here scripts one debugging session in its header, and the
session must read the same on the VM debugger and on lldb (proposal 0188
S6):

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

- `vm_debug` runs the commands with `flux debug <file> --script`.
- `llvm_debug` builds the file with `--native -g` and runs the same commands
  in `lldb -b`, through `flux_lldb.py`'s `flux` command: `flux next`, `flux
  step` and `flux finish` step as the VM does, across performs too, and
  `continue` first drops a step a breakpoint interrupted. After each command
  that runs the program, `flux where` says where it stopped. The session
  starts with `flux catch panic division unhandled fibers`, the VM's
  default, and runs the program with `FLUX_WORKERS=1`, as the VM debugger
  runs async code on one worker.
- `llvm_debug` reports a skip on a machine without lldb, with an lldb that
  runs no Python, and on Windows, whose debug builds carry CodeView for
  Visual Studio and cdb (0188 S11).
- Besides `break`, `delete`, `continue`, `step`, `next`, `finish`, `bt`,
  `locals` and `print`, a fixture can use `handlers`, `catch <kinds>`,
  `threads` and `thread N` (0188 S8).

Both transcripts are reduced to stops and frames (function and
`file:line`), `exception <kind>` after a stop at a runtime error, handlers
(`handler Effect state = v`), variables (`name = value`, each group sorted)
and `finished`. A stop in a fiber other than thread 1 adds ` thread N`, and
`threads` reduces to `thread N name [state] file:line` per fiber. Frames in
standard-library files (`Async.flx`, any file starting with a capital),
whose shape differs between the backends, read `<library>`, one line for a
run of them. Columns, positions, stop reasons, the VM's tail-call labels,
the `⟂ handling …` labels, the `(suspended)` marks, where a handler was
installed and the program's output are left out. Stops and frames must
match line for line, except that native code may show a handler's arm the
VM does not: an arm whose last call is `resume` waits natively under the
frames it resumes.
In each group of variables, every variable the VM shows must be there
natively with the same value. Native code may still hold a value the VM
has moved out of its slot, so the native side may show more; a value native
code has freed (`<dropped>`) counts as not shown.

Each fixture ends with `// dbg-check:` lines (proposal 0190 §5) that say
what each debugger must show, checked on each way on its own, so a change
that breaks both alike still fails:

```flux
// dbg-check: stop <lambda closures.flx:13:31> closures.flx:14
// dbg-check: offset = 10 (captured)
// dbg-check: x = 1
// dbg-check: frame main closures.flx:13
```

- Each check is a whole line of the reduced transcript, matched in order,
  each on a later line than the one before it. Lines between checks are
  allowed, so native code may show more, as above.
- They go at the end of the file: the script names line numbers, and lines
  added above the program would move it.
- `--bless` writes them from the VM debugger's transcript, when both
  debuggers agree. After a change that means to alter what a session shows,
  run it again and read the diff:
  ```sh
  flux parity-check tests/parity_debug --ways vm_debug,llvm_debug --bless
  ```
- `parity_debug_tests` fails a fixture without checks.

Writing a fixture:

- Put the `// dbg:` lines in the header, and count lines from the top of
  the file (bless the checks last): a breakpoint on a comment or a blank line moves to the next line
  with code on the VM, and resolves nowhere in lldb.
- One statement per line; stepping is by line.
- Look at values while the program still uses them: both backends release a
  value after its last use.
- A function that prints needs `with IO`.
- Effects: look at values in frames that run, not in suspended ones (they
  have none natively), nor at `resume` (a continuation on the VM, a closure
  natively). Let arms call `resume` themselves: an arm that tail-calls a
  helper loses its frame natively, and the frames it suspended with it.
- A line that is only a value (`n`) has code on the VM but not natively:
  step over it in neither.
- Async code: keep one fiber running at a time where the order matters,
  so both backends number and schedule fibers alike (`both` runs its
  children in order; a channel's sender and receiver take turns). Timers
  keep counting while a session is stopped, so let a `timeout` be long
  enough not to fire during it. Look at values in the fiber that stopped:
  a waiting fiber's frames have no variables natively.
