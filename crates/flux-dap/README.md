# flux-dap

The Debug Adapter Protocol (DAP) server for Flux (proposal 0188 S4 and S6).
VS Code starts it to debug `.flx` programs:

- on the VM (the default): breakpoints, stepping, the call stack, variables,
  watches and hovers, and going backwards with Step Back and Reverse
  Continue;
- natively, with `"backend": "native"`: the program built with `-g` and run
  under `lldb-dap`, with the same Flux views but no going back.

## Install

`./scripts/rebuild-vsix.sh` builds `flux-lsp` and `flux-dap`, bundles both
into the VS Code extension, and installs it. To use a build of your own, set
`flux.dapPath` to the `flux-dap` binary. Otherwise the extension uses the
bundled one, then `flux-dap` on `PATH`.

The adapter runs the program with `flux`. By default the extension uses the
`flux.runCommand` setting (`cargo run --` in this repository). The `flux`
it runs must be from the same build as `flux-dap`.

## Launch configuration

Pressing F5 with a `.flx` file open debugs that file with no configuration.
Otherwise, add one to `.vscode/launch.json`:

```json
{
  "type": "flux",
  "request": "launch",
  "name": "Debug main",
  "program": "${workspaceFolder}/main.flx"
}
```

| Attribute | Meaning |
|---|---|
| `program` | The `.flx` file to debug. Required. |
| `args` | Arguments for the program. |
| `cwd` | Working directory; the program's folder by default. |
| `stopOnEntry` | Stop at the program's first line. |
| `showInternals` | Show compiler-made frames and variables (type-class dictionaries, wrappers). |
| `backend` | `"vm"` (the default) or `"native"`. |
| `lldbDap` | For `"native"`: the `lldb-dap` to run (the `flux.lldbDapPath` setting). |
| `record` | Also write the run's effects to this tape (`--record`). |
| `replay` | Answer effects from this tape (`--replay`). |
| `fluxCommand` | How to run `flux`, such as `["flux"]` or `["cargo", "run", "--"]`. |
| `fluxArgs` | Options for `flux` itself, such as `["--no-cache"]`. |

Every session keeps its effects in memory, so Step Back and Reverse Continue
work without a tape. Once a program runs async code or starts a task, going
back is off for the rest of the session, and the step-back requests answer
with the reason (0188 S8 lifts this).

## Native sessions

With `"backend": "native"`, the adapter:

1. builds the program with `<fluxCommand> <program> --native -g
   --emit-binary -o <temporary dir>/<name>`, showing the build's messages
   in the Debug Console. The `flux` it runs needs the native backend:
   `cargo run --features llvm --` in this repository, not plain
   `cargo run --`;
2. writes `flux_lldb.py` next to the binary;
3. starts `lldb-dap`: `lldbDap`, else `FLUX_LLDB_DAP`, else `lldb-dap` or
   `lldb-dap-18` … `-22` on `PATH`, else `xcrun --find lldb-dap` on macOS;
4. passes DAP between VS Code and `lldb-dap`, launching the binary with
   `flux_lldb.py` imported.

Most messages pass unchanged, renumbered. The adapter changes what makes the
views Flux's:

- **Call Stack:** only frames in `.flx` source; the runtime's frames show,
  de-emphasised, with `showInternals`.
- **Variables:** the Locals scope only. Values are Flux values (`[1, 2]`,
  `Point(1, 2)`), and a variable not bound yet is left out. A value the
  program has freed shows `<dropped>`: native code frees a value at its last
  use, where the VM keeps it.
- **Source paths:** sent to `lldb-dap` with symlinks resolved, since lldb
  matches breakpoints on the file's real path, and returned in the editor's
  spelling.
- **No Step Back:** the capabilities say so, and the reverse requests and
  console commands answer that they need the VM.
- `record` and `replay` reach the program as `FLUX_TAPE_RECORD` /
  `FLUX_TAPE_REPLAY`.

On Windows, `"native"` is refused until CodeView comes (0188 S11).

## Effects and exceptions

While a handler arm runs, the Call Stack shows the frames the `perform`
suspended below it, after a label line:

```text
<handler Flip.flip>         app.flx 20:25
⟂ handling Flip.flip — performed at app.flx:9:13
decide (suspended)          app.flx 9:13
main                        app.flx 19:18
```

Selecting a suspended frame shows its variables. While `resume` runs the
continuation again, its frames sit above the arm, which waits at its
`resume` call. A handler that never resumes marks its frames
`(suspended, not resumable)`. The `Handlers` scope lists the active effect
handlers, innermost first, with a parameterized handler's state.

The Breakpoints view offers the runtime errors to stop at: panics, division
by zero and unhandled effects (on by default) and other runtime errors.
Such a stop shows the error where it happened, with the stack as it was;
continuing ends the program with the error.

## The Debug Console

Typing a variable, or a part of one (`pair.0`, `tree.2.1`), shows its value.
Watch expressions and hovers take the same form. Evaluating other
expressions comes in a later stage.

The console also takes commands for what DAP has no button for:

| Command | Meaning |
|---|---|
| `.where` | The position: how many stop sites have run. |
| `.goto <n>` | Go to a position, earlier or later. |
| `.events` | The effects the program has performed, with their positions. |
| `.event <n>` | Go to the last stop before effect `n`. |
| `.rs` `.rn` `.rf` `.rc` | Reverse step, next, finish, continue. |
| `.handlers` | The active effect handlers, innermost first. |
| `.internals on\|off` | Show or hide compiler-made frames and variables. |
| `.help` | This list. |

In a native session, `.bt`, `.locals` and `.print <name>` run
`flux_lldb.py`'s `flux` command, and lldb commands work behind a backtick:
`` `bt all ``.

## Architecture

```text
VS Code ──DAP over stdio──▶ flux-dap ──JSON lines over 127.0.0.1──▶ flux debug app.flx --engine <addr>
                               ▲                                        │
                               └────── program stdout and stderr ───────┘

VS Code ──DAP over stdio──▶ flux-dap ──DAP over stdio──▶ lldb-dap ──▶ app (built with -g)
```

- `flux-dap` (`src/server.rs`) translates DAP requests into
  `flux::debug_protocol` requests, and engine events into DAP events.
- The program runs in a child process, `flux debug --engine`
  (`src/driver/debug_engine.rs` in the `flux` crate). There, the S2 and S3
  engine (`src/vm/debug/`) does the stopping, stepping and going back.
  - A crash or `exit` in the program cannot take the adapter down.
  - Its output comes back on pipes as DAP `output` events.
  - The program's stdin is not connected, since DAP has no channel for it.
- While the program runs, the child reads requests at its next stop site:
  new breakpoints apply there, and a pause stops there.
- There is one thread, `main`. Fibers become threads in 0188 S8.
- A native session (`src/native.rs`) builds the program, starts `lldb-dap`
  and leaves the debugging to it; the session renumbers messages and
  rewrites the stack, scopes, variables and source paths.

Set `FLUX_DAP_LOG=1` to log every DAP message to stderr. VS Code shows that
stderr in the "Flux" debug adapter output.

## Tests

```sh
cargo test -p flux-dap
```

`tests/dap_tests.rs` plays the editor against a real `flux debug --engine`
child, and snapshots the transcript of requests, responses and events. The
tests use the `flux` binary in the same target directory, building it first
if it is missing. Set `FLUX_BIN` to use another one, and
`FLUX_DAP_TEST_TRACE=1` to print each transcript line as it happens.
The native tests need a `flux` with the native backend and `lldb-dap`; they
skip with a note when either is missing.

## Manual checklist (VS Code)

Run this after changing the adapter or the extension, using
`examples/guide` programs and a fresh `./scripts/rebuild-vsix.sh`:

1. With a `.flx` file open and no `launch.json`, F5 starts a session, and
   the program's output appears in the Debug Console.
2. A gutter breakpoint is hit. A breakpoint on a blank line moves to the
   next line with code.
3. The Call Stack shows the Flux function names, each with its file and
   line. A tail-recursive loop shows "… N calls elided by tail calls".
4. Variables shows Locals, plus Captured inside a closure. Tuples, lists and
   data constructors expand.
5. Step Over, Step Into and Step Out move as expected. Stepping into a
   `|>` pipeline stops at each stage.
6. Step Back and Reverse Continue go back, and the variables match what
   they were going forward.
7. Pause stops a long-running loop. Stop ends the session.
8. Watch shows `name.0` parts, and hovering a local shows its value.
9. The Debug Console's `.events` lists printed text, and `.event 0` goes
   to it.
10. A program with a compile error shows the error in the Debug Console and
    ends the session.
11. `"replay"` with a tape recorded by `flux app.flx --record run.jsonl`
    shows the recorded values.
12. `"showInternals": true` shows the dictionary frames that are hidden by
    default.
17. In a handler arm (`examples/effects`), the Call Stack shows the
    `⟂ handling …` label and the suspended frames; selecting one shows its
    variables.
18. Step Into on `resume(…)` stops at the `perform`; Step Over on it stops
    back in the arm.
19. The `Handlers` scope lists the handlers, with a parameterized handler's
    state.
20. With "Panics" ticked in the Breakpoints view, a `panic` stops where it
    happens, the editor shows the exception and its message, and Continue
    ends the program with the error. Unticked, the program just ends.

With `"backend": "native"` and `flux.runCommand` set to
`cargo run --features llvm --`:

17. F5 builds the program (the build's progress in the Debug Console) and
    stops at a gutter breakpoint, which turns solid.
18. The Call Stack shows only Flux frames, `main` at the bottom.
19. Variables shows Locals with Flux values; lists and constructors expand,
    a record's parts by field name; a `let` below the current line is
    absent.
20. Step Over, Step Into and Step Out move through Flux lines, never into
    the runtime. Step Back is not offered.
21. `.locals` in the Debug Console prints the frame's variables.
