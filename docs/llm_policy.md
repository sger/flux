# LLM usage policy

This page explains how you may use large language models (LLMs) and coding
agents when you contribute to Flux. Flux has one maintainer, who uses LLM
tools in their own work, so the maintainer and outside contributors follow
different rules.

Agents working in this repository follow [`AGENTS.md`](../AGENTS.md), which
turns this page into step-by-step instructions. Every rule in `AGENTS.md`
comes from this page. `AGENTS.md` can be stricter than this page, but never
looser.

## Principles

- Use an LLM to write better code, not to write code faster. It doesn't
  replace understanding the code you change.
- You are responsible for everything you submit. "The LLM wrote that" is not
  an answer to a review comment.
- LLM-assisted work must be at least as good as human-written work, and it
  needs more tests.
- You never need an LLM to contribute.

## Scope

This policy covers the whole repository and its issues and pull requests on
GitHub and Forgejo. That includes the compiler (`src/`), the workspace crates
(`crates/`), the C runtime (`runtime/c/`), the Flux libraries (`lib/`),
`xtask/`, `editors/`, `tools/`, `docs/`, `tests/` and `examples/`.

The code in `vendor/` is maintained elsewhere. Send changes to its upstream
project, and only update the copy here.

### What counts as LLM-written

Text an LLM generated is LLM-written, even after you edit it. This applies to
chat output and to editor autocompletion. Autocompletion is usually a
[trivial change](#trivial-changes), but it isn't exempt.

## Roles

- Maintainer: Spiros Gerokostas.
- Outside contributor: anyone else.

The maintainer reviews everything that merges, their own work included, and is
responsible for it. An LLM-written PR from an outside contributor adds review
work for the maintainer, so the maintainer has to agree to it before you
start.

## Allowed for everyone

You can use an LLM privately, where you are the only one who sees the output.
For example, you can:

- ask questions about the codebase;
- summarize issues, PRs or proposals for yourself (posting the summary
  publicly is a different matter);
- review your own code or writing;
- write tools for your own use;
- generate a possible solution, learn from it, and then write your own
  version from scratch.

You can also use an LLM to find bugs, as long as you reproduce each bug
yourself. Report it with your reproduction, not with the LLM's analysis.

Draft PRs that are clearly experimental and not meant for review, such as one
opened just to get a CI run, are fine too. Disclose LLM use when the PR comes
out of draft.

## The maintainer's LLM-assisted work

The maintainer may use LLMs for code, tests, proposals and internal
documentation (`docs/internals/`, `docs/proposals/`, `docs/tooling/`), on
these conditions:

- The PR includes a [disclosure](#disclosure).
- The maintainer reviews the complete diff after its last change. An LLM
  review doesn't count.
- The change meets the [testing standard](#testing-standard).
- A change in a [soundness area](#soundness-areas) passes `cargo xtask verify`
  before it merges.
- The maintainer reads diagnostic messages and user-facing docs
  (`docs/guide/`, `README.md`) carefully before merging. Users read this text
  directly, so it should sound like the project and not like a model.

## Outside contributors

### Banned

You may not submit LLM-written:

- PR descriptions, issue bodies, or review and discussion comments. You can
  quote LLM output if you mark it clearly and your own comment makes sense
  without it;
- user-facing documentation: the language guide (`docs/guide/`), `README.md`
  and library READMEs;
- diagnostic messages. An LLM can help with the code around a diagnostic, but
  you must write the message yourself. See
  [`docs/internals/diagnostics.md`](internals/diagnostics.md);
- non-trivial source comments: doc comments, `SAFETY` comments and comments
  longer than a paragraph.

You also may not treat an LLM review as enough to merge or reject a PR, or
skip reviewing your own diff. Review it before you open the PR and again after
every change.

### LLM-written code

You may submit LLM-written code only if all of the following are true:

1. The maintainer agreed to review it before you started. Ask in an issue
   first. You can work on a draft while you wait, but don't open the PR until
   the maintainer says yes.
2. The change is outside the [soundness areas](#soundness-areas).
3. The code is at least as good as code you would write by hand. PRs that
   were clearly "vibe-coded" will be closed.
4. It meets the [testing standard](#testing-standard).
5. You understand every line and can explain it, and you have reviewed the
   whole diff yourself.

### Trivial changes

A change is trivial when there is really only one way to write it: a typo
fix, a Markdown link, a synonym, or a signature the compiler dictates. You may
submit trivial LLM-written changes if you disclose them. A PR made only of
trivial changes will get a closer look.

## Soundness areas

The code in these areas works with types, effects, ownership or generated
code. A bug here can make Flux accept a wrong program or print wrong output
with no error at all, and bugs like that are hard to catch with tests.

- Type inference, type classes and effects: `src/types/`,
  `src/ast/type_infer/`, `src/core/passes/`
- Aether ownership and reference counting: `src/aether/`
- Lowering and code generation: `src/core/lower_ast/`, `src/cfg/`, `src/lir/`,
  `src/bytecode/`, `src/llvm/`
- Runtimes: `src/vm/`, `src/runtime/`, `runtime/c/`

These are usually outside the soundness areas: `xtask/`, `src/parity/`,
`src/cli/`, `src/repl/`, `crates/flux-lsp/`, `crates/flux-dap/`, `editors/`,
`tools/`, tests and docs. If you're not sure, treat your change as being in a
soundness area and ask.

## Testing standard

- A bug fix starts with a failing test. Add or find a test that shows the bug,
  run it, and watch it fail before you change any code. After the fix, run
  the same test and watch it pass.
- New behavior needs tests for every edge case you or the reviewer can think
  of.
- If the code you're changing has no suitable tests, write them, or close the
  PR. Talk to the maintainer before you build a new test harness.

[`tests/README.md`](../tests/README.md) explains where tests go and how to run
them. Parity fixtures are covered in
[`docs/internals/parity.md`](internals/parity.md), and Flux's own test runner
in [chapter 8 of the guide](guide/08_testing.md).

## Mechanical changes

When a tool can make a change, run the tool. Don't have an LLM imitate its
output:

- formatting: `cargo fmt --all`
- lint fixes: `cargo clippy --fix`
- snapshots: `cargo insta review` or `cargo insta test --accept`, once you have
  seen the test fail with the mismatch you expected

Snapshots with diagnostic text in them are only ever updated by the tool, after
a person has written the new message in the source. Never edit them by hand.

## Disclosure

Fill in the "LLM use" section of the
[pull request template](../.github/pull_request_template.md) in every PR. Say
what the LLM did: whether it came up with the idea, helped write the code,
wrote tests, or only reviewed. Write this section yourself.

Don't add `Co-Authored-By` trailers that name an LLM or an agent to your
commits. An LLM isn't an author: you are responsible for the change, and the
PR is where you disclose how an LLM helped.

## Enforcement

Be honest about LLM use. A PR that hides or misrepresents it will be closed,
and future PRs from the same person may be refused.

Don't go looking for LLM use in other people's work. Writing style proves
nothing. People who are writing in a second language, who are neurodivergent,
or who simply explain things at length are the most likely to be wrongly
accused. If you see a clear violation, link to this policy. Don't accuse
anyone.

For minor violations, you'll get help fixing them. Major violations, and PRs
that cost more to review than they're worth, will be closed.

## Changes to this policy

The maintainer decides any changes to this policy.

---

This policy was inspired by the Rust project's
[LLM usage policy](https://forge.rust-lang.org/policies/llm-usage.html), and
`AGENTS.md` by the `AGENTS.md` in rust-lang/rust. A few of Rust's rules only
make sense for a project with many teams, so they are left out for now: team
ratification, the cap on the share of LLM-written PRs, mentoring channels and
the rules for review bots.
