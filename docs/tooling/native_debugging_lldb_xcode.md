# Debugging native Flux programs with lldb and Xcode

A native build with `-g` carries DWARF line tables (proposal 0188 S5).
Debuggers can then set breakpoints at `app.flx:12`, show Flux function names
and lines in backtraces, and step through Flux source. Variables and Flux
value printing come in S6; until then, use the VM debugger (`flux debug`, or
VS Code) to inspect values.

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

What you see:
- A breakpoint may resolve to two or three locations: the compiler emits a
  function's body more than once (boxed, unboxed and closure-capture
  variants). They are all the same Flux line.
- Lambdas are named `<lambda app.flx:4:34>`, handler clauses
  `<handler op>`. Code after a call that can suspend is
  `name (resumed after L:C)`.
- Reference counting and other compiler-made code is at line 0, so stepping
  passes over it. So does the C runtime: `main(argc, argv)` in `flux_rt.c`
  is the frame below the Flux `main`.

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
