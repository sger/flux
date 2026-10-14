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
- On Windows, `-g` builds have no debug information yet; CodeView for Visual
  Studio comes in 0188 S11.

`-g` keeps the build's optimisation level and turns on frame pointers. Tail
calls stay tail calls, so deep recursion behaves as it does without `-g`.
`-g` does keep every `let` the program wrote, which a build without it may
inline away, so that the debugger can show it.

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
- `frame variable` and the variables views of Xcode and VS Code use the same
  formatting, and expand lists, tuples, arrays and constructors.
- A value shows as `<dropped>` once the program has released it: native code
  frees a value at its last use, and the debugger does not read freed
  memory. The VM keeps such values, so `flux debug` still shows them.

What you see:
- A breakpoint may resolve to two or three locations: the compiler emits a
  function's body more than once (boxed, unboxed and closure-capture
  variants). They are all the same Flux line.
- Without the script, `frame variable` shows raw 64-bit words: an Int `n`
  is `2n+1`, and other values are addresses. In an unboxed worker frame,
  Ints and Bools are raw.
- Lambdas are named `<lambda app.flx:4:34>`, handler clauses
  `<handler op>`. Code after a call that can suspend is
  `name (resumed after L:C)`.
- Reference counting and other compiler-made code is at line 0, so stepping
  passes over it. So does the C runtime: `main(argc, argv)` in `flux_rt.c`
  is the frame below the Flux `main`.

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
Flux frames, variables as Flux values, stepping through Flux lines. Step
Back needs the VM. In this repository, set `flux.runCommand` to
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
