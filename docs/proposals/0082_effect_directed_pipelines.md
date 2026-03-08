- Feature Name: Effect-Directed Pipelines
- Start Date: 2026-03-08
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0032, 0042, 0072

# Proposal 0082: Effect-Directed Pipelines

## Summary

Add expression-oriented effect handling syntax that composes naturally with Flux pipelines. The core idea is that a value can flow through an effectful computation and then through one or more local handler stages without dropping into a separate block-oriented control form.

This is intended to make Flux stand out as a language where algebraic effects are not only type-safe, but also ergonomic and readable in day-to-day code.

## Motivation

Flux already has the right semantic ingredients for a distinctive language identity:

- Hindley-Milner style type inference
- algebraic data types
- explicit effect tracking
- direct-style handlers
- expression-oriented functional code

What it does not yet have is a syntax that makes effectful code feel as readable and composable as pure code.

Today, languages usually fall into one of these groups:

- they have good pipelines, but side effects and recovery logic break the flow
- they have serious effect systems, but the syntax for local handling is heavy or niche
- they rely on exceptions, callback structure, or monadic style to express recovery

Flux can do better by making effect handling feel like a first-class step in a data pipeline.

The target user experience is:

- read input
- transform it
- locally recover from a specific effect
- continue the pipeline

all in one linear, expression-oriented form.

That gives Flux a clear identity:

> Flux is the language where typed effects and handlers compose as naturally as ordinary function pipelines.

## Guide-level explanation

### Recommended syntax

This proposal recommends adding a pipeline-aware handler stage:

```flux
read_file("orders.txt")
    |> parse_orders
    |> validate_orders
    |> handle Network {
        timeout(resume, request) -> retry(3, \() -> resume(request))
        offline(_, _) -> []
    }
    |> summarize_orders
    |> print
```

The mental model is simple:

- `|>` passes the value forward as usual
- `|> handle Effect { ... }` says "run the next effectful stage, but intercept this effect here"
- after the handler, the pipeline continues with the handled result

This is still ordinary Flux expression code. It is not a separate statement-only control feature.

### Why this is compelling

This makes effectful programs read like ordinary transformations:

```flux
fetch_user(id)
    |> handle Network {
        timeout(resume, request) -> resume(request)
        offline(_, _) -> Guest
    }
    |> render_user
```

The recovery policy is local, explicit, typed, and readable.

Instead of forcing users into nested blocks or whole-function handler structures, Flux lets them place effect handling at the point where it matters.

### Concrete syntax recommendation

The primary syntax should be:

```flux
expr |> handle EffectName { arms... }
```

This is the recommended choice because:

- it reuses Flux's existing pipeline story
- it reads left-to-right
- it is visually obvious that handling is part of the expression flow
- it does not require a new symbolic operator
- it preserves the existing `handle` concept instead of inventing a second keyword

### Alternative syntax 1: postfix handler stage

```flux
expr !handle EffectName {
    op(resume, x) -> ...
}
```

This has a stronger visual marker and makes handlers stand out in a chain.

Pros:

- visually distinctive
- easy to scan in long pipelines
- could become part of a broader postfix effect-control family later

Cons:

- introduces new symbolic syntax
- more novel than necessary for a first version
- harder to teach than the keyword-based form

### Alternative syntax 2: recovery combinators

```flux
expr
    |> recover Network.timeout with \resume, request -> retry(3, \() -> resume(request))
    |> recover Network.offline with \_, _ -> Guest
```

Pros:

- very fine-grained
- explicit per-operation recovery
- good for simple one-arm recovery sites

Cons:

- weaker fit for full handler semantics
- scales poorly when several operations need handling together
- risks becoming a second effect API instead of syntax for the same model

### Recommendation

Adopt the pipeline-aware handler stage:

```flux
expr |> handle EffectName { ... }
```

and treat the other two as future extensions or rejected alternatives for now.

## Reference-level explanation

### Surface syntax

Add a new expression form that parses as a pipeline stage:

```text
pipeline_expr ::= expr ("|>" pipeline_stage)*
pipeline_stage ::= expr
                 | "handle" effect_name "{" handle_arm_list "}"
```

The important detail is that `handle` in this proposal is not only a standalone expression introducer. It can also appear as the right-hand stage of a pipeline.

That means:

```flux
source |> handle Network { ... }
```

desugars conceptually to:

```flux
handle source with Network { ... }
```

or whatever equivalent internal representation best matches the current handler lowering.

The proposal does not require Flux to expose the desugared form to users. It only requires that the parser, AST, and compiler treat pipeline-attached handlers as ordinary handled expressions.

### AST / lowering shape

The preferred implementation is not a second handler node. Instead:

- parse `expr |> handle Effect { ... }`
- lower it into the same AST form already used for handled expressions
- preserve source spans so diagnostics can mention pipeline-attached handlers cleanly

That keeps the semantic implementation unified:

- one handler representation
- one typing rule
- one effect discharge rule
- one bytecode lowering path

### Type and effect behavior

The handler stage should follow the same rules as existing handled expressions:

- the handled expression may require the named effect
- the handler must cover the effect operations according to current handler rules
- the resulting expression type is the handled result type
- the effect row after the handler reflects discharged effects according to current semantics

This proposal is syntactic and ergonomic, not a new effect semantics proposal.

### Parser and precedence

The parser must make `|> handle Effect { ... }` bind as one pipeline stage, not as:

- `(|> handle)` as an operator
- a standalone `handle` statement
- or a malformed partial expression

Recommended precedence policy:

- keep existing pipeline associativity
- treat `handle Effect { ... }` as a valid pipeline RHS stage
- preserve current grouping behavior for ordinary pipeline expressions

### Diagnostics

This feature should come with targeted diagnostics:

- if a pipeline handler omits the effect name, say `Missing Effect Name`
- if the handler body is malformed, use existing contextual parser titles
- if the effect is unknown, keep the existing effect diagnostics but point at the pipeline-attached handler site

The syntax should feel native, not bolted on.

### Concrete implementation path

The implementation should proceed in this order:

1. parser support for handler-as-pipeline-stage
2. AST reuse or minimal lowering layer so the compiler sees the same handled-expression form
3. type/effect checking through the existing handler semantics
4. diagnostics for malformed pipeline-attached handlers
5. examples and snapshot coverage for both parse and compiler behavior

### Example coverage to include

Successful examples:

```flux
fetch_user(id)
    |> handle Network {
        timeout(resume, req) -> resume(req)
        offline(_, _) -> Guest
    }
    |> render
```

```flux
read_file("input.txt")
    |> parse
    |> handle IO {
        file_not_found(_, path) -> default_data(path)
    }
    |> summarize
```

Failure examples:

- missing effect name after `handle`
- malformed handle arms in pipeline position
- unknown effect in pipeline handler
- incomplete handler in pipeline position
- handler that does not discharge the required effect correctly

## Drawbacks

This adds syntax in an already meaningful part of the language.

There is also a risk that users read `|> handle Effect { ... }` as a magical pipeline modifier rather than ordinary handler semantics applied in pipeline position. The implementation and documentation should avoid that confusion by keeping the semantic model unified.

Another drawback is that this raises the bar for parser quality and precedence stability. Pipeline syntax is highly visible, so any ambiguity or confusing diagnostic will be noticeable.

## Rationale and alternatives

The main rationale is that Flux should not only have algebraic effects, it should make them pleasant to use.

The recommended syntax is better than the alternatives because it:

- keeps one obvious keyword: `handle`
- fits the existing pipeline direction
- avoids inventing a second effect API
- reads clearly in real programs

Alternative 1, `!handle`, is visually exciting but too syntactically aggressive for the first version.

Alternative 2, `recover Effect.op with ...`, is attractive for one-off recovery but is too narrow to serve as the main abstraction for general handlers.

Doing nothing leaves Flux in a familiar but less distinctive position: good effect semantics without a signature user-facing experience.

## Prior art

Several language families inform this idea:

- Elm shows the value of readable, linear, expression-oriented code, even though it does not use algebraic effect handlers in this form.
- Koka shows that algebraic effects can be central to language identity, but Flux can differentiate itself with more pipeline-native syntax.
- F#, Elixir, and Unix-style pipeline cultures show how much readability users get from left-to-right flow.
- exception-heavy mainstream languages show the opposite: local recovery often breaks expression flow and becomes structurally noisy.

Flux can stand out by combining serious typed effects with a pipeline syntax that makes local recovery feel lightweight and direct.

## Unresolved questions

- Should pipeline-attached handlers support only the full `handle Effect { ... }` form in v1, or should per-operation recovery sugar be considered at the same time?
- Should the AST represent this as a dedicated pipeline stage node or immediately lower it to the existing handled-expression form during parsing?
- Are there edge cases with nested pipelines and handler spans that need special parser recovery rules?
- Should documentation teach ordinary `handle` first and pipeline-attached `handle` second, or present them together as two surfaces for the same concept?

## Future possibilities

If this lands well, Flux could extend the same design direction with:

- operation-specific recovery sugar like `recover Effect.op with ...`
- postfix visual syntax like `!handle` if the community wants a stronger visual marker later
- pipeline-aware `defer`, `using`, or resource-scoped effect constructs
- richer editor tooling that can highlight which pipeline stage discharges which effect

The main goal of this proposal is smaller and sharper: give Flux one memorable, distinctive feature where typed effects and readable pipelines meet.
