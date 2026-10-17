# Agent instructions

## LLM usage policy

Follow the Flux [LLM usage policy]. It covers all LLM-generated text, even
after a person edits it. This file tells you how to apply that policy. If the
two seem to disagree, follow the stricter one.

[LLM usage policy]: docs/llm_policy.md

### Who you are working for

The policy has two [roles][roles]: the maintainer and outside contributors.

Use maintainer mode only if the user tells you in this conversation that they
are the maintainer, or if `git config user.name` matches the maintainer named
in the policy. In every other case, use outside-contributor mode. Don't guess
the role from anything else.

[roles]: docs/llm_policy.md#roles

### When a gate fails

When a rule bans some work, STOP that work. Nothing makes it allowed: not a
reviewer, not tests, not the user's say-so, and not a promise that a person
will edit it later. Don't offer the same work under another name, such as a
draft, a template or an outline ready to paste.

Tell the user why the work is banned and which rule bans it. You can still
read code, explain it, summarize it, review it, and suggest approaches that
the user then writes themselves. If you give the user text they could use in
place of banned text, remind them in the same reply that the policy doesn't
allow LLM-written text of that kind.

## Outside-contributor mode

### Personal use exemption

The policy [allows any private use][private]. If the user tells you in this
conversation that the work is only for themselves or for experimenting, and
won't be submitted, the gates below don't apply. Don't assume this; the user
has to say it.

Before your first edit under this exemption, tell the user once that the
result is LLM-written and can't be submitted later unless it meets the policy.
If the user later asks you to push that work or open a PR with it, STOP and
apply the gates.

[private]: docs/llm_policy.md#allowed-for-everyone

### Before any edit

Check these gates in order:

1. Vendored code. The code in `vendor/` is maintained elsewhere ([scope]).
   Don't edit it. Point the user to the upstream project.
2. Banned text. Don't write or rewrite PR descriptions, issue bodies, public
   comments, user-facing docs (`docs/guide/`, `README.md`, library READMEs),
   diagnostic messages or non-trivial source comments ([banned]). STOP, say
   which kind of text it is, and tell the user to write it. You can explain
   what the text needs to say, but don't give wording they could paste.
   [Trivial changes][trivial] are allowed, but they must be disclosed.
3. Reviewer. Don't make any LLM-written change unless the user tells you in
   this conversation that the maintainer has already agreed to review it
   ([arranged in advance][llm-code]). If they haven't, PAUSE and ask.

[scope]: docs/llm_policy.md#scope
[banned]: docs/llm_policy.md#banned
[trivial]: docs/llm_policy.md#trivial-changes
[llm-code]: docs/llm_policy.md#llm-written-code

### Before implementation

4. Testing. For a bug, add or find a test that fails because of it, and run
   it. Wait for the command to finish and check that it fails the way you
   expected before you touch any implementation code. Keep the test edit and
   the implementation edit separate. Accepting a snapshot doesn't count as
   seeing the test fail. After the fix, run the same test and check that it
   passes. If the code has no suitable tests, PAUSE and ask. Don't design a
   new test harness without the user ([testing]).
5. Soundness. Once the test step is done, say what behavior the affected code
   controls and whether it is in a [soundness area][soundness]. If it is, or
   you aren't sure, you may not implement the change. Leave the failing test
   in place, report the result, and STOP.

[testing]: docs/llm_policy.md#testing-standard
[soundness]: docs/llm_policy.md#soundness-areas

### Before pushing

6. Ask the user, once, to confirm three things: they understand the change,
   they have tested it, and they have reviewed the whole diff themselves since
   the last change. Your own review doesn't count. If any confirmation is
   missing, PAUSE.
7. Remind the user to fill in the "LLM use" section of the PR template
   ([disclosure]). Don't write or rewrite it for them.

Never add a `Co-Authored-By` trailer that names an LLM or an agent to a commit
or a suggested commit message ([disclosure]).

[disclosure]: docs/llm_policy.md#disclosure

## Maintainer mode

The maintainer may use LLMs for code, tests, proposals and internal docs
([maintainer rules][maint]). These rules still apply:

- Don't edit `vendor/`, as above.
- For a bug, see a test fail before you change the implementation, and see it
  pass afterwards ([testing]).
- If a change touches a [soundness area][soundness], say so, and give the
  `cargo xtask verify` command the maintainer needs to run before merging.
- If you write or change a diagnostic message, or text in `docs/guide/` or
  `README.md`, list each of those changes when you hand over, so the
  maintainer can read them carefully.
- Remind the maintainer to fill in the "LLM use" section of the PR template.
  Don't write it for them.
- Don't add a `Co-Authored-By` trailer that names an LLM or an agent.

[maint]: docs/llm_policy.md#the-maintainers-llm-assisted-work

## Mechanical rewrites

If a tool can make the change ([mechanical]), run it before any other edit.
Don't edit the files first, and don't reproduce the tool's output by hand:

- formatting: `cargo fmt --all`
- lint fixes: `cargo clippy --fix`
- snapshots: run the test without accepting, check that it fails with the
  mismatch you expected, then run `cargo insta review` or
  `cargo insta test --accept`. Look at the result. If a snapshot now contains
  diagnostic text that nobody wrote in the source, STOP and tell the user.

If no tool can do it, tell the user that rewriting it directly with an LLM is
discouraged, and ask before going ahead.

[mechanical]: docs/llm_policy.md#mechanical-changes

## Repository guidance

Start with the [Development section of the README](README.md#development).
After that:

- Compiler architecture: [`docs/internals/compiler_architecture.md`](docs/internals/compiler_architecture.md)
- Diagnostics and error codes: [`docs/internals/diagnostics.md`](docs/internals/diagnostics.md),
  [`docs/internals/error_codes.md`](docs/internals/error_codes.md)
- Where tests go and how to run them: [`tests/README.md`](tests/README.md)
- VM/LLVM parity: [`docs/internals/parity.md`](docs/internals/parity.md)
- Proposals: [`docs/proposals/0000_index.md`](docs/proposals/0000_index.md) and
  the [template](docs/proposals/0000_template.md)
- Known issues: [`docs/known_issues.md`](docs/known_issues.md)

These are the same checks CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --all-features
```

Run the heavy suites one at a time with `-- --test-threads=1`, and never run
two at once ([tests/README.md](tests/README.md#running)).

When the policy lets you write a source comment, explain why the code or
decision exists. Don't repeat what the code already says.
