# flux-dap

The Debug Adapter Protocol (DAP) server for Flux (proposal 0188 S4). VS Code
starts it to debug `.flx` programs on the VM: breakpoints, stepping, the call
stack, variables, watches and hovers, and going backwards with Step Back and
Reverse Continue.

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
| `backend` | `"vm"`. `"native"` is refused until 0188 S6. |
| `record` | Also write the run's effects to this tape (`--record`). |
| `replay` | Answer effects from this tape (`--replay`). |
| `fluxCommand` | How to run `flux`, such as `["flux"]` or `["cargo", "run", "--"]`. |
| `fluxArgs` | Options for `flux` itself, such as `["--no-cache"]`. |

Every session keeps its effects in memory, so Step Back and Reverse Continue
work without a tape. Once a program runs async code or starts a task, going
back is off for the rest of the session, and the step-back requests answer
with the reason (0188 S8 lifts this).

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
| `.internals on\|off` | Show or hide compiler-made frames and variables. |
| `.help` | This list. |

## Architecture

```text
VS Code ──DAP over stdio──▶ flux-dap ──JSON lines over 127.0.0.1──▶ flux debug app.flx --engine <addr>
                               ▲                                        │
                               └────── program stdout and stderr ───────┘
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
