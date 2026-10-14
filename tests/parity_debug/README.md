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
cargo test --features llvm --test parity_debug_tests
```

- `vm_debug` runs the commands with `flux debug <file> --script`.
- `llvm_debug` builds the file with `--native -g` and runs the same commands
  in `lldb -b`, through `flux_lldb.py`'s `flux` command: `flux next`, `flux
  step` and `flux finish` step as the VM does, across performs too, and
  `continue` first drops a step a breakpoint interrupted. After each command
  that runs the program, `flux where` says where it stopped. The session
  starts with `flux catch panic division unhandled`, the VM's default.
- Besides `break`, `delete`, `continue`, `step`, `next`, `finish`, `bt`,
  `locals` and `print`, a fixture can use `handlers` and `catch <kinds>`.

Both transcripts are reduced to stops and frames (function and
`file:line`), `exception <kind>` after a stop at a runtime error, handlers
(`handler Effect state = v`), variables (`name = value`, each group sorted)
and `finished`. Columns, positions, stop reasons, the VM's tail-call labels,
the `⟂ handling …` labels, the `(suspended)` marks, where a handler was
installed and the program's output are left out. Stops and frames must
match line for line, except that native code may show a handler's arm the
VM does not: an arm whose last call is `resume` waits natively under the
frames it resumes.
In each group of variables, every variable the VM shows must be there
natively with the same value. Native code may still hold a value the VM
has moved out of its slot, so the native side may show more; a value native
code has freed (`<dropped>`) counts as not shown.

Writing a fixture:

- Put the `// dbg:` lines in the header, and count lines from the top of
  the file: a breakpoint on a comment or a blank line moves to the next line
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
- Async code is 0188 S8.
