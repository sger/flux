# Commit messages

Every commit is one line:

```
type(scope): what is true now (reference)
```

For example:

```
fix(native): a caught panic unwinds through landing pads that release what each frame held (KI-126)
test(debug-heap): a caught panic releases what a skipped handle built (KI-133)
feat(xtask): cargo xtask census tags features, joins KIs and writes the support matrix (0190 S16)
docs(known-issues): file KI-134 and KI-135
```

There is no body. The reasons for a change, its trade-offs and what it does
not cover go in the PR description, or in `docs/known_issues.md` for a known
issue. The PR description is also the changelog entry, so that is where
readers look.

## The hook

`.githooks/commit-msg` checks each message as you commit: the type and
scope, a summary, no final full stop, and no body or trailer. It can't check
that the summary says what is true now, so that part is up to you. Turn it on
once in each clone:

```sh
git config core.hooksPath .githooks
```

Messages git writes itself (`Merge …`, `Revert "…"`) and `fixup!`,
`squash!` and `amend!` commits pass. To skip the check for one commit, use
`git commit --no-verify`.

A commit that only reformats goes in `.git-blame-ignore-revs`, so `git blame`
shows the change that last meant something. GitHub reads the file on its own;
locally, run `git config blame.ignorerevsfile .git-blame-ignore-revs` once.

## Type

| Type | Use it for |
|---|---|
| `feat` | something new a user or a contributor can do: a language feature, a flag, an `xtask` command |
| `fix` | a bug fix |
| `test` | tests only, including the failing test that comes before a fix |
| `docs` | documentation only |
| `refactor` | a change in structure that keeps behavior the same |
| `perf` | a change that makes something faster or smaller and keeps behavior the same |
| `ci` | the CI workflows |
| `chore` | anything else that changes no behavior, such as tidying output or bumping a dependency |
| `style` | formatting only |
| `revert` | undoing an earlier commit by hand; `git revert`'s own `Revert "…"` line is fine too |

## Scope

The scope is the part of the project the commit changes, one of the names
below. The hook reads this table, so a new area is added here first.

| Scope | Covers |
|---|---|
| `syntax` | the lexer, parser and AST (`src/syntax`, `src/ast`) |
| `types` | type inference, unification and generics (`src/types`) |
| `typeclasses` | classes, instances, deriving and dispatch |
| `effects` | effect rows, handlers, `perform` and `resume` |
| `diagnostics` | diagnostic messages, error codes and their rendering |
| `compiler` | the compiler driver, modules, interfaces and the cache (`src/compiler`) |
| `core` | Core IR and its passes (`src/core`) |
| `aether` | reference counting and reuse (`src/aether`) |
| `cfg` | the CFG IR (`src/cfg`) |
| `lir` | LIR and continuation splitting (`src/lir`) |
| `llvm` | LLVM IR emission and the native pipeline (`src/llvm`) |
| `bytecode` | the bytecode compiler (`src/bytecode`) |
| `vm` | the bytecode VM (`src/vm`) |
| `runtime` | the C runtime (`runtime/c`) and the Rust runtime (`src/runtime`) |
| `native` | native behavior that spans `lir`, `llvm` and `runtime` |
| `async` | fibers, tasks, channels and the scheduler |
| `replay` | recording and replaying a run (the tape) |
| `stdlib` | the standard library (`lib/Flow`) |
| `cli` | the `flux` command line (`src/cli`, `src/driver`) |
| `repl` | the REPL (`src/repl`) |
| `lsp` | the language server (`crates/flux-lsp`) |
| `debugger` | debug information, DAP and lldb (`crates/flux-dap`, `tools/lldb`, `src/debuginfo`) |
| `vscode` | the VS Code extension (`editors/vscode`) |
| `flume` | the package manager (`lib/Flume`) |
| `xtask` | `cargo xtask` (`xtask/`) |
| `parity` | `parity-check` and its fixtures (`src/parity`, `tests/parity`) |
| `census` | `cargo xtask census` and the support matrix |
| `ci` | the CI workflows |
| `tests` | test harnesses and suites that belong to no single area above |
| `debug-heap` | the debug-heap test suite (`tests/debug/native_debug_heap_tests.rs`) |
| `examples` | `examples/` |
| `benchmarks` | `benches/` and `benchmarks/` |
| `known-issues` | `docs/known_issues.md` |
| `internals` | `docs/internals/` |
| `guide` | `docs/guide/` |
| `proposals` | `docs/proposals/` as a whole |
| `roadmap` | `docs/roadmaps/` |
| `readme` | `README.md` and library READMEs |

Two more kinds of scope need no entry:
- a proposal number, for work on one proposal: `docs(0190): …`;
- a version, for release work: `chore(0.0.8): …`.

How to write it:
- For a commit that changes several parts, list them with commas and no
  spaces: `fix(lir,runtime): …`.
- A test commit takes the scope of what it tests, or its suite's.
- Leave the scope out only when the commit really touches the whole project.

## Summary

Write what is true after the commit, as a sentence about the program, not
about the edit:

- `fix(runtime): a map node is a counted block, so a map releases everything under it`,
  not `fix(runtime): fix map leak`;
- `test(debug-heap): a failed read_file counts the path its error holds`,
  for the test that checks it.

Start in lowercase, unless the first word is a name such as `KI-127` or
`VM`, and leave out the final full stop. One line can be long
when the change needs it, but most commits fit in about 100 characters.

Don't start the line with a file path, and don't add the date or the author:
git records both.

## Reference

End with what the commit belongs to, in parentheses:

- a known issue: `(KI-133)`;
- a proposal and its step: `(0190 S4)`;
- several of them: `(PA R1, KI-121)`.

## A bug fix is two commits

1. `test(…)`: the test that fails because of the bug, on its own, so anyone
   can check out this commit and see it fail.
2. `fix(…)`: the fix, after which the test passes.

Both commits carry the same reference.

## Trailers

Don't add a `Co-Authored-By` trailer that names an LLM or an agent. How an
LLM helped is disclosed in the PR instead (see the
[LLM usage policy](../llm_policy.md#disclosure)).
