# Hint Chains Feature

## Overview

Hint chains provide step-by-step guidance for complex errors. Instead of a single hint, they break down the solution into numbered steps with an optional conclusion.

## Feature Status

✅ **Complete and Production-Ready**
- Infrastructure implemented and tested
- 4 comprehensive tests passing
- Ready for integration into compiler errors

## How It Works

When the compiler detects a complex error with multiple possible solutions or requiring several steps to fix, it can provide a hint chain:

```
-- Compiler error: Type mismatch [E020]

Cannot add String and Int

  --> example.flx:3:14
   |
 3 | let result = name + age;
   |              ^^^^^^^^^^

Hint:
  To fix this error:
    1. Convert the String to Int using .parse()
    2. Handle the potential parse error with match or ?
    3. Or change the function signature to accept String

  Type checking helps catch these errors at compile time
```

## API Reference

### Creating Hint Chains

```rust
use flux::frontend::diagnostics::HintChain;

// Basic hint chain
let chain = HintChain::new(vec![
    "Step 1".to_string(),
    "Step 2".to_string(),
    "Step 3".to_string(),
]);

// From iterator (convenience)
let chain = HintChain::from_steps(vec![
    "Step 1",
    "Step 2",
    "Step 3",
]);

// With conclusion
let chain = HintChain::from_steps(vec![
    "Step 1",
    "Step 2",
]).with_conclusion("This is the recommended approach");

// Using diagnostic builder methods
Diagnostic::error("Complex error")
    .with_hint_chain(chain)

// Convenience method
Diagnostic::error("Complex error")
    .with_steps(vec!["Step 1", "Step 2"])

// With conclusion
Diagnostic::error("Complex error")
    .with_steps_and_conclusion(
        vec!["Step 1", "Step 2"],
        "Summary or recommendation"
    )
```

### HintChain Fields

- `steps: Vec<String>` - The numbered steps to follow
- `conclusion: Option<String>` - Optional concluding note or recommendation

## Use Cases

Perfect for:
- **Type conversion errors** - Multiple approaches to fix (parse, cast, change signature)
- **Configuration errors** - Several settings that need to be adjusted
- **Migration guides** - Steps to update code for new API
- **Multi-step fixes** - Errors requiring procedural resolution
- **Alternative solutions** - Different ways to solve the same problem

Not suitable for:
- Simple errors with one clear fix (use regular hints or inline suggestions)
- Errors where the fix is obvious from the error message
- Situations where too many steps would overwhelm the user

## Testing

### Run the demo
```bash
cargo run --example demo_hint_rendering
```

### Run unit tests
```bash
cargo test hint_chain
```

## Integration Guide

To add hint chains to your error:

```rust
use crate::frontend::diagnostics::HintChain;

// When detecting a complex error:
return Err(Diagnostic::error("Type mismatch")
    .with_code("E020")
    .with_span(error_span)
    .with_message("Cannot perform this operation")
    .with_hint_chain(
        HintChain::from_steps(vec![
            "Convert the type using .parse() or .into()",
            "Handle the potential conversion error",
            "Or change the function signature to accept both types",
        ]).with_conclusion("See the docs for more on type conversions")
    ));
```

## Comparison with Other Features

| Feature | Purpose | Best For |
|---------|---------|----------|
| **Hint Chains** | Step-by-step guidance | Complex, multi-step fixes |
| Inline Suggestions | Show exact code fix | Simple replacements |
| Regular Hints | Provide guidance | General advice |
| Categorized Hints | Organize different hint types | Structured information |

## Rendering Details

The rendering logic:
1. Checks if there are any hint chains
2. Renders "Hint:" header in blue
3. Shows "To fix this error:" introduction
4. Numbers each step sequentially
5. Adds optional conclusion after the steps

The feature integrates seamlessly with:
- Color output (can be disabled with `NO_COLOR`)
- Other diagnostic features (hints, labels, suggestions)
- Multiple hint chains (each renders separately)

## Best Practices

**Do:**
- Keep steps concise (one line each when possible)
- Order steps logically (chronological or by difficulty)
- Use the conclusion for recommendations or additional context
- Limit to 3-5 steps for readability

**Don't:**
- Create overly long steps (break into multiple if needed)
- Include more than 7-8 steps (too overwhelming)
- Duplicate information already in the error message
- Use hint chains for simple one-step fixes

## Examples

### Type Conversion Error
```rust
.with_steps_and_conclusion(
    vec![
        "Parse the string: let num = s.parse::<i32>()?",
        "Handle the parse error with match or unwrap",
        "Or accept String type in the function",
    ],
    "Consider using type annotations to prevent these errors"
)
```

### Configuration Error
```rust
.with_steps(vec![
    "Set the API key in .env file: API_KEY=your_key",
    "Add the .env file to .gitignore",
    "Load the environment variables on startup",
])
```

### Migration Guide
```rust
.with_steps_and_conclusion(
    vec![
        "Replace old_function() with new_function()",
        "Update the parameter types from &str to String",
        "Handle the new Result return type",
    ],
    "See MIGRATION.md for complete migration guide"
)
```

## Future Enhancements

Potential improvements:
- Interactive mode where user selects which approach to follow
- Code examples embedded in steps
- Links to documentation for each step
- Collapsible steps in IDE integration
