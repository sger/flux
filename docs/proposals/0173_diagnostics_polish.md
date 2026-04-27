- Feature Name: Diagnostics Polish — Doc Layer, Smarter Suggestions, Context-Sensitive Hints
- Start Date: 2026-04-26
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: none (touches `src/diagnostics/` and a small surface in `src/syntax/parse_*` for parser hints)

# Proposal 0173: Diagnostics Polish

## Summary

After 0.0.5 shipped a feature-heavy effect-system / type-class / pattern
exhaustiveness release, the diagnostics surface is solid in *infrastructure*
(structured `Diagnostic`, JSON renderer, dedup aggregator, error-code
registry, `text_similarity`, `quality.rs` provenance helpers) but conservative
in *authorship*: messages are built as `String` with manual `push_str` and
manual ANSI escapes, typo suggestions use plain Levenshtein, parser hints are
fixed per error code (no peek-ahead at the actual following token), and there
is no convention for linking hints to versioned documentation.

This proposal collects nine focused improvements under a single 0.0.6
"diagnostics polish" theme, in dependency order. Each item is independently
shippable, but later items become substantially easier once the `Doc`
combinator layer (item 1) lands.

The largest visible win — narrative type-mismatch messages driven by a
`MismatchContext` carried through type inference — is **explicitly deferred
to 0.0.7** as a follow-up proposal, because it is a multi-PR initiative and
much easier on top of the `Doc` layer.

## Motivation

### Where Flux's diagnostics are today

`src/diagnostics/` (~5,100 lines) provides:

- A structured `Diagnostic` carrying span, severity, hints, labels, related
  diagnostics, inline suggestions, and stack traces.
- A multi-output rendering layer: ANSI text (`renderer.rs`), plain text
  (NO_COLOR honored in `colors.rs`), and JSON (`json.rs`).
- A multi-error aggregator (`aggregator.rs`, 1,234 lines) with deduplication,
  per-file grouping, sort-by-position, and a configurable
  `max_errors` cap.
- An error-code registry (`compiler_errors.rs` — 108+ codes,
  `runtime_errors.rs`, `registry.rs`) so every diagnostic has a stable
  identifier (`E419`, `W013`, …) usable for opt-out, cross-referencing, and
  documentation.
- Quality-scored type-mismatch helpers (`quality.rs`): `TypeMismatchNotes`,
  `TypeOriginNote`, `EffectConstraintOrigin`, `type_pair_hint`.
- A typo-suggestion helper (`text_similarity.rs`) using plain Levenshtein
  distance.

Flux is **explicitly multi-error by design**: the aggregator collects every
diagnostic from a compilation phase, deduplicates, sorts, groups by file, and
emits them all (capped by `max_errors`). This is the right call given Flux
has stable codes, VM/native parity tests, snapshot tests, and a Language
Server (proposal 0163) on the roadmap — all of which benefit from seeing
*every* problem the compiler knows about, not the first one. **This proposal
does not change the multi-error philosophy.**

### What's missing

A motivated user reading a Flux error today gets *correct* information but
not always *well-presented* information. Concretely:

1. **No reflow.** Long messages are written as pre-broken strings with hard
   newlines. Width-aware wrapping at 80 columns is not available.
2. **Plain Levenshtein.** A typo of `lenght` for `length` is distance 2
   (delete + insert), which ranks it lower than less-likely candidates that
   happen to share more characters. Adjacent-character transpositions are the
   most common typo class and they aren't favored.
3. **Generic parser hints.** When the parser fails at column N, the hint is
   fixed per error code. The compiler does not peek at the actual character
   class at column N+1 to give context-sensitive guidance like "I see a
   keyword `then` here — did you forget the condition?"
4. **One-region snippets.** Source rendering shows one region per snippet.
   Diagnostics that fundamentally compare *two* locations (a redundant
   pattern vs. its earlier match; a branch type vs. previous branches) emit
   two separate stacked snippets that aren't visually connected.
5. **No documentation links.** Hints are text-only. There is no convention
   for embedding a versioned link (`https://flux-lang.org/<compiler-version>/<topic>`)
   into a hint, so users cannot click through to deeper docs from the
   terminal.
6. **Import cycles render flat.** Cycles are reported as "Foo → Bar → Baz →
   Foo" text. The recent 04-26 import-cycle work made the *content* better;
   the *presentation* is still a single line.
7. **String-building rendering.** `renderer.rs` (~580 lines) builds output
   imperatively with `push_str` and inline ANSI escapes. Every new rendering
   convention requires editing the renderer; there is no composable
   intermediate value an author can build and inspect.

These gaps are individually small. Collectively they're the difference
between "the compiler is correct" and "the compiler is *helpful*."

## Guide-level explanation

After this proposal, authoring a diagnostic looks like this:

```rust
use crate::diagnostics::{Diagnostic, DiagnosticBuilder, doc};

Diagnostic::error("type mismatch")
    .with_code(E_TYPE_MISMATCH)
    .with_span(span)
    .with_doc_message(
        doc::reflow("This expression is being used in an unexpected way.")
            .stack(doc::pair(
                doc::label("It is:", doc::yellow(&actual_ty)),
                doc::label("But you are trying to use it as:", doc::yellow(&expected_ty)),
            ))
            .stack(doc::doc_link("Hint", "Read", "type-mismatch", "for examples.")),
    )
```

The author builds a tree of `Doc` values; the renderer chooses the output
format (ANSI / plain / JSON). Reflow at 80 columns is automatic.

For typo suggestions, an unknown name `lenght` ranks `length` at edit
distance 1 (a single transposition), so it appears as the top suggestion
where it would have been ranked lower under plain Levenshtein.

For parser errors, when the parser fails just before a keyword, the hint is
context-sensitive:

```text
error[E2014]: unexpected end of expression
  src/main.flx:5:12
  |
5 | let x = if then 1 else 2
  |            ^^^^

Hint: I see the `then` keyword here. Did you forget to write
the condition between `if` and `then`?
```

For import cycles, the diagnostic includes a small visual:

```text
error[E1041]: import cycle
  src/Foo.flx:1:1

  ┌─────┐
  │    Foo
  │     ↓
  │    Bar
  │     ↓
  │    Baz
  └─────┘
```

Hints can carry versioned doc links:

```text
Hint: Read <https://flux-lang.org/v0.0.6/imports> to see how
import declarations work.
```

Two-region snippets render related locations together:

```text
error[E0205]: redundant pattern
  src/main.flx:5:5

3 |     0 -> "zero",
  |     ^   earlier match here
4 |     n -> "number",
5 |     0 -> "also zero"
  |     ^   redundant
```

None of this changes *what* errors fire, only *how* they read.

## Reference-level explanation

The work breaks into eight focused items, in the order they should ship.
Each section names the new module/struct, the surface area touched, and the
estimated effort.

### Item 1 — `Doc` combinator layer

**New module:** `src/diagnostics/doc.rs`.

**Surface:** introduce a `Doc` enum (or adopt the
[`pretty`](https://crates.io/crates/pretty) crate, Wadler/Leijen-style and
mature, MIT/Apache) with these primitives:

```rust
pub enum Doc {
    Empty,
    Text(Cow<'static, str>),
    Concat(Vec<Doc>),
    Stack(Vec<Doc>),       // vertical, one blank line between
    FillSep(Vec<Doc>),     // wrap at width boundaries
    Indent(usize, Box<Doc>),
    Color(Color, Box<Doc>),
    Underline(Box<Doc>),
}

pub fn reflow(s: &str) -> Doc;        // word-wrap a paragraph
pub fn stack(docs: Vec<Doc>) -> Doc;
pub fn fill_sep(docs: Vec<Doc>) -> Doc;
pub fn comma_sep(items: Vec<Doc>) -> Doc;
pub fn fancy_hint(chunks: Vec<Doc>) -> Doc;   // "Hint:" + chunks
pub fn fancy_note(chunks: Vec<Doc>) -> Doc;
pub fn ordinal(n: usize) -> String;           // "1st", "2nd", "3rd", "4th"…
pub fn args(n: usize) -> String;              // "1 argument" / "N arguments"
```

Three renderers consume a `Doc`:

- `render_ansi(doc, width=80) -> String`
- `render_plain(doc, width=80) -> String`
- `encode_json(doc) -> serde_json::Value`

`renderer.rs` is rewritten to build a `Doc` tree and call `render_ansi`. The
existing `with_message(&str)` API is preserved as a thin shim that wraps the
string in `Doc::Text`. New diagnostics use `with_doc_message(Doc)`.

**Effort:** 2–4 days. Single focused PR. Touches every callsite that builds
rendered output.

**Why first:** every later item is easier on top of `Doc` (especially items
4, 6, and the deferred 0.0.7 narrative type errors).

### Item 2 — Damerau-Levenshtein distance

**File:** `src/diagnostics/text_similarity.rs`.

Add:

```rust
pub fn damerau_levenshtein_distance(a: &str, b: &str) -> usize;
```

Restricted Damerau-Levenshtein: counts adjacent transpositions as a single
edit (`teh`/`the` = 1, `lenght`/`length` = 1). Plain Levenshtein computes
both as 2.

The existing `levenshtein_distance` is marked `#[deprecated(since = "0.0.6",
note = "use damerau_levenshtein_distance")]` and removed in 0.0.7.

Add tests for the canonical transposition cases (`teh`/`the`,
`lenght`/`length`, `recieve`/`receive`).

**Effort:** half a day. ~30 lines plus tests.

### Item 3 — Ranked, lowercase-normalized typo suggestions

**File:** `src/diagnostics/text_similarity.rs`.

Add:

```rust
pub fn rank<'a, T>(target: &str, items: &'a [T], to_str: impl Fn(&T) -> &str)
    -> Vec<(usize, &'a T)>;

pub fn sort<'a, T>(target: &str, items: &'a [T], to_str: impl Fn(&T) -> &str)
    -> Vec<&'a T>;
```

Both lowercase both sides before computing distance, so `Length` and `length`
are equivalent for ranking purposes.

Audit existing callsites in `compiler_errors.rs` and `quality.rs` for any
that compare case-sensitively, and migrate them.

**Effort:** half a day. Mostly mechanical migration.

### Item 4 — `whatIsNext` peek-ahead for parser hints

**New module:** `src/diagnostics/lookahead.rs`.

Surface:

```rust
pub enum NextToken {
    Keyword(&'static str),     // a reserved word
    Operator(String),          // operator characters
    CloseDelim(char),          // ) ] }
    Upper(String),             // identifier starting with uppercase
    Lower(String),             // identifier starting with lowercase
    Other(Option<char>),       // anything else, or EOF
}

pub fn what_is_next(source: &str, line: usize, col: usize) -> NextToken;
pub fn next_line_starts_with(keyword: &str, source: &str, line: usize)
    -> Option<(usize, usize)>;
pub fn next_line_starts_with_close_curly(source: &str, line: usize)
    -> Option<(usize, usize)>;
```

Parser error reporters in `src/syntax/parse_*` use `what_is_next` to choose
between context-sensitive hint variants. Roll this out to the most-painful
parser errors first (the E2xx range that users hit most often per snapshot
test traffic), with fixtures, rather than retrofitting every parser error in
one go.

**Effort:** 1–2 days for the helper + rollout to top 5 parser errors.

### Item 5 — Versioned documentation link helper

**File:** `src/diagnostics/doc.rs` (alongside item 1).

Add:

```rust
pub fn doc_link(label: &str, before: &str, slug: &str, after: &str) -> Doc;
pub fn make_link(slug: &str) -> String;  // → "<https://flux-lang.org/v0.0.6/SLUG>"
```

The version segment comes from `env!("CARGO_PKG_VERSION")` so links are
always pinned to the compiler version that emitted them.

Until `flux-lang.org` (or whatever the docs domain becomes) exists, the
links are still useful as stable slugs for grep / future redirects. The
helper is forward-compat plumbing.

Start using `doc_link` in high-traffic hints (effect-system errors, type
mismatches, import errors). Do not retrofit every hint.

**Effort:** half a day.

### Item 6 — Two-region source snippets

**File:** `src/diagnostics/rendering/source.rs`.

Add a renderer variant:

```rust
pub fn render_two_region_snippet(
    out: &mut String,
    source: &str,
    region1: Span,
    region2: Span,
    bridge: &str,    // text shown between the two snippets
    use_color: bool,
);
```

Behavior:

- If both regions are on the **same line**, render the line once with two
  caret underlines side-by-side.
- Otherwise, render two stacked snippets with the `bridge` text shown between
  them.

Use sites: redundant pattern diagnostics (E0205 / pattern-exhaustiveness),
"this branch differs from these earlier branches" (eventually used by 0.0.7
narrative type errors), and any future "shadowed binding" diagnostic.

**Effort:** 1 day.

### Item 7 — Import-cycle visualization

**File:** `src/diagnostics/doc.rs` (alongside item 1) — provide a `cycle`
combinator.

```rust
pub fn cycle(indent: usize, names: &[&str]) -> Doc;
```

Renders a box-drawing visual for an import cycle:

```text
  ┌─────┐
  │    Foo
  │     ↓
  │    Bar
  │     ↓
  │    Baz
  └─────┘
```

ASCII fallback (`+`, `|`, `-`, `v`) for terminals that don't render
box-drawing characters cleanly (Windows `cmd.exe` without UTF-8, plain ASCII
NO_COLOR).

Wire into the import-cycle diagnostic from the 04-26 import-cycle commit.

**Effort:** half a day. Pure presentation polish.

### Item 8 — Cascade suppression broadening

**File:** `src/diagnostics/aggregator.rs` and the type-inference / Core-pass
boundary.

The recent `import-cycle-diagnostics` change established a precedent:
"stopped module-resolution failures from cascading into later type and
backend diagnostics." This proposal generalizes the pattern.

When a phase fails on a name `x`:

- record `x` as **poisoned** in a `PoisonedNames` set carried through
  subsequent phases,
- suppress *cascade* diagnostics whose root cause is a poisoned name (mark
  them with `Severity::Suppressed` rather than dropping, so the LSP can
  still surface them on demand),
- keep the original diagnostic visible.

Concrete rollout:

- Type inference: when a binding fails to infer, poison its name and
  suppress downstream "unknown type variable" / "unification failure"
  diagnostics that mention the poisoned name as a root cause.
- Core lowering: when AST→Core fails on a definition, poison and suppress
  downstream "unresolved name" diagnostics for that definition's call sites.
- Backend: when a function fails to compile, poison and suppress
  downstream-only diagnostics (linker / codegen errors that depend on it).

The aggregator gains a `Severity::Suppressed` variant that is rendered only
when `--show-suppressed` is passed (and surfaced via the LSP capability).

This is **not** a single-error-at-a-time mode. Multi-error remains the
default. Cascades are the only thing pruned, and they remain queryable.

**Effort:** 2–3 days. The poisoning plumbing is straightforward; finding the
right suppression points without losing real errors is where care is needed.

### Item 9 — Foreign-operator hint table

**File:** `src/diagnostics/compiler_errors.rs` (or a new
`compiler_errors/foreign_ops.rs` if we split that module — see Future
Possibilities).

When name resolution fails on an operator that isn't Flux's but *is* a
common operator from another language, emit a targeted hint instead of the
generic "unknown operator" message. The most-common foreign operators new
users try:

| Foreign operator | Coming from | Flux equivalent | Hint |
|---|---|---|---|
| `!=` | C, Java, Rust, JS | `/=` (proposed) or `not (a == b)` | "Flux uses `/=` for inequality." |
| `===`, `!==` | JS | `==`, `/=` | "Flux uses `==` for equality (no strict-equality variant)." |
| `%` | C, Python, Rust | `mod` (or future `safe_mod`) | "Flux uses `mod` for modulo. See proposal 0135 for `safe_mod`." |
| `&&` / `\|\|` | C, Java, Rust | confirm Flux spelling | (table-driven) |
| `++` (numeric) | C, Java | `n + 1` | "Flux has no increment operator; use `n + 1`." |
| `<>` | Haskell | `++` (string concat) | "For string concatenation Flux uses `++`." |
| `**` | Python, JS | `pow` | "Flux uses `pow` for exponentiation." |

The actual Flux spellings need to be confirmed against current syntax before
implementation; treat the table above as illustrative.

Implementation is a small lookup table consulted by the unresolved-operator
diagnostic before falling through to the generic "did you mean…?" path. No
runtime cost when the operator *is* valid Flux. ~50 lines plus tests.

**Effort:** half a day.

## Drawbacks

- **Item 1 is invasive.** Every callsite in `renderer.rs` changes. We could
  punt by keeping the `String` API and adding `Doc` only for new
  diagnostics, but that defeats the point — the value of a `Doc` layer is
  consistent reflow / styling everywhere.
- **Item 8 risks hiding real errors.** A cascade-suppression heuristic that
  is too aggressive can mask a genuine downstream bug. Mitigation:
  `Severity::Suppressed` (not `Dropped`); LSP can opt-in. We accept some
  initial conservativeness and tighten over time.
- **Adopting `pretty` adds a dependency.** Alternative: a hand-rolled `Doc`
  enum (~200 lines) with the subset we need. Given Flux's overall philosophy
  of keeping the dependency graph small, the hand-rolled option is probably
  preferred. Decision deferred to implementation time.

## Rationale and alternatives

- **Why not skip the `Doc` layer and keep `String`?** Because narrative type
  errors (deferred to 0.0.7) are nearly impossible to author readably as
  hard-broken strings, and width-aware reflow is the single most-noticed
  presentation improvement after color.
- **Why not write all nine items as one PR?** Items 2, 3, 5, 6, 7, 9 are
  small and orthogonal. Item 1 is invasive. Item 4 touches parser code.
  Item 8 touches type-inference plumbing. Splitting matches review surface
  and lets us ship items 2, 3, and 9 immediately (week 1) while item 1 is
  in review.

## Prior art

- The Flux `text_similarity.rs` already provides a Levenshtein helper —
  this proposal upgrades the algorithm rather than introducing one.
- The `quality.rs` module already has provenance helpers (`TypeOriginNote`,
  `EffectConstraintOrigin`) that prefigure the deferred 0.0.7
  `MismatchContext` work.
- The 04-26 `import-cycle-diagnostics` commit established the cascade-
  suppression pattern that item 8 generalizes.

## Unresolved questions

- Hand-rolled `Doc` enum vs. the `pretty` crate. Decide at implementation
  time based on whether `pretty`'s API surface matches what we actually use.
- Whether `make_link`'s base URL should be hard-coded to `flux-lang.org` or
  read from a build-time environment variable. Hard-coded is simpler and
  changeable later; environment-variable is more flexible but adds build
  setup. Default to hard-coded.
- Exact threshold for `damerau_levenshtein_distance` to *show* a suggestion
  ("did you mean…?"). Current implicit threshold for the typo path is
  unclear — settle on `≤ 2 OR ≤ len/3` and document.

## Future possibilities

- **0.0.7 follow-up: narrative type errors.** Plumb a `MismatchContext` enum
  (`ListEntry`, `IfBranch`, `CaseBranch`, `OpLeft`, `OpRight`, `CallArg`,
  `RecordUpdateValue`, …) through `ast/type_infer/`. The reporter then
  writes narrative messages keyed on context: "The 2nd branch of this `if`
  does not match all the previous branches." This is the single biggest
  user-visible improvement remaining, but it is a multi-PR initiative and
  is much easier on top of the `Doc` layer (item 1). It depends on this
  proposal landing first; it is *not* in scope for 0.0.6.

- **0.0.7+ follow-up: parser `Context` stack.** Today, parser errors carry
  a span but no record of *what construct they were nested in*. A `Context`
  enum (`InNode { node: Node, start: Span, parent: Box<Context> }`,
  `InDef { name, start }`, `InDestruct { start }` with `Node` covering
  `Record`, `Parens`, `List`, `Func`, `Cond`, `Then`, `Else`, `Case`,
  `Branch`) would let parser hints know "you're in the `then` branch of an
  `if` inside a record field." This is a meaningful refactor of
  `src/syntax/parse_*` and is its own proposal. **Item 4's `what_is_next`
  helper is the cheap version of this idea**; the full `Context` stack is
  the deeper version.

- **0.0.7+ follow-up: paired-region snippets used throughout the parser.**
  Item 6 introduces `render_two_region_snippet` and rolls it out to
  redundant-pattern diagnostics. A natural next step is to use it for
  *every* "unfinished construct" parser error: render `(start_of_construct,
  position_of_failure)` as a pair so the user sees both "where parsing of
  this `if` began" and "where I got stuck." This depends on the parser
  carrying the construct-start position through to the error site, which
  pairs naturally with the parser `Context` stack above.

- **0.0.7+ follow-up: split `compiler_errors.rs` by phase.** The file is
  1,859 lines today (was 687 when MEMORY.md noted "revisit when >1000
  lines"). Phase-locality would help: `compiler_errors/{syntax,
  canonicalize, type_infer, pattern, import, module, const_eval,
  ice}.rs`. Each file becomes a `data Error = …`-shaped Rust enum with a
  `to_diagnostic` function. Stable error codes survive the split (still
  registered in `registry.rs`).

- **"Missing" vs. "malformed" parser-error split.** Today our parser tends
  to emit one error per failure. Splitting into "I didn't see `then` next"
  vs. "I saw `then` but the surrounding shape is wrong" — the split that
  drives indentation-aware messaging in whitespace-sensitive languages —
  is also useful for brace-delimited Flux when the user mistypes a
  delimiter. Concretely: `IfMissingThen` vs. `IfMalformedThen`. Couples
  with item 4 (`what_is_next`) to choose the variant.

- **First-person voice convention.** Across the surveyed surface in this
  proposal, well-regarded compilers consistently write diagnostics in
  first person ("I was expecting…", "I got stuck here…"). Flux's existing
  messages are mixed (some imperative, some passive, some first-person).
  Add a one-paragraph note to `CLAUDE.md` / contributor docs establishing
  first-person as the convention for new diagnostic messages, and drift
  existing ones during touch-up. Style guide, not code; zero compiler
  effort.

- **Curated long-form hints directory.** A `docs/hints/<slug>.md` directory
  of editorial explainers for the most-confusing errors (mirrors of what
  every mature compiler eventually accumulates: bad-recursion,
  comparing-custom-types, import-cycles, infinite-type, missing-patterns,
  recursive-alias, shadowing, type-annotations). Item 5's `doc_link`
  helper is the *plumbing*; this is the *content*. A long-term editorial
  initiative, not compiler work.

- **LSP integration.** Once items 1 and 8 are in, the Language Server
  (proposal 0163) can surface the `Doc`-rendered output in hovers and
  expose `Severity::Suppressed` as an editor toggle.

- **Diagnostic explanations.** Long-form per-code explanations (the kind
  Rust reaches via `--explain E0432`) become natural once the `Doc` layer
  exists and `doc_link` slugs are stable.

- **Validate the runtime-error story against 0135.** Mature pure-FP
  compilers have *no* runtime-error reporter — type inference, exhaustive
  pattern matching, and total functions eliminate the *need* for one.
  Flux's `runtime_errors.rs` exists today (E1008 division-by-zero, E1201
  multi-shot continuation, etc.) because we still have runtime panics.
  Proposal **0135 (Total Functions and Safe Arithmetic)** is the path to
  closing that gap: Phase 1 (`safe_div`/`safe_mod` returning
  `Option<Int>`) shipped in 0.0.5; Phase 2 (`NonZero` type) and Phase 3
  (operator edition change) are open. The long-term goal is for
  `runtime_errors.rs` to *shrink* toward zero as compile-time guarantees
  expand. This proposal does not advance 0135 directly, but the framing
  in this list confirms 0135's direction.

## Out of scope

- **Single-error-at-a-time display.** Some compilers display only the
  first error from a phase by design (philosophy: subsequent errors are
  often cascade artifacts and users learn faster from one focused error
  than ten noisy ones). Flux takes the opposite stance and that stance is
  correct here: stable error codes, VM/native parity tests, snapshot
  tests, and the planned LSP all benefit from seeing every diagnostic the
  compiler knows about, not the first one. Confirmed against
  `aggregator.rs` (gathers / dedupes / sorts by file→line→col→severity /
  groups by file with `max_errors: Option<usize>` as a *cap* defaulting
  to `usize::MAX`) and `driver/shared.rs::emit_diagnostics_or_exit`
  (collects all phase diagnostics before exiting). No `exit_on_first` /
  `abort_on_error` / `halt_on_error` flags exist anywhere in `src/`.
  Item 8's cascade suppression refines the multi-error model — it does
  not change it.

- **Runtime-error elimination.** This proposal polishes how diagnostics
  *render*. Reducing the *number* of runtime errors (by making more
  operations total) is proposal 0135's responsibility and is tracked
  there. See the runtime-error note under Future Possibilities.

- **Authoring style guide.** A first-person voice convention for new
  diagnostic messages is worth adopting (see Future Possibilities) but
  is contributor documentation, not compiler work, and is filed as
  follow-up rather than scoped here.
