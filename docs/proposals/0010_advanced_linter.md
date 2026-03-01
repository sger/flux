- Feature Name: Advanced Linter (Post v0.1.0)
- Start Date: 2026-02-02
- Proposal PR: 
- Flux Issue: 

# Proposal 0010: Advanced Linter (Post v0.1.0)

## Summary
[summary]: #summary

This proposal outlines a comprehensive linter enhancement plan for Flux, introducing advanced code quality checks, configurability, and integration with development workflows.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 1. Result Usage (W100-W109)

**W101: Unused Function Result**
```flux
calculate_total(items);  // ← W101: Result not used
// Should be: let total = calculate_total(items);
```

**W102: Unused Error Result**
```flux
match try_open_file(path) {
    Left(err) -> ();  // ← W102: Error ignored without handling
    Right(file) -> process(file);
}
```

**Implementation:**
- Track function calls in expression statements
- Check if return value is `None` type
- Flag non-unit returns that are discarded

### Usage

```toml
# .flux-lint.toml
[custom-rules]
no-print-in-prod = { level = "warn", path = ".flux-lint/rules/no_print_in_prod.flx" }
```

### 1. Result Usage (W100-W109)

### Usage

```toml

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Existing Warnings:** | Code | Warning | Status | |------|---------|--------| | W001 | Unused variable | ✅ Working | | W002 | Unused parameter | ✅ Working | | W003 | Unused i...
- **Existing Warnings:** | Code | Warning | Status | |------|---------|--------| | W001 | Unused variable | ✅ Working | | W002 | Unused parameter | ✅ Working | | W003 | Unused import | ✅ Working | | W00...
- **Primary Objectives:** 1. **Configurability** - Let users customize lint rules 2. **Advanced Checks** - Add sophisticated code quality analysis 3. **IDE Integration** - LSP support for real-time linti...
- **2. Code Smell Detection (W110-W119):** **W111: Magic Number** ```flux fn calculate_tax(amount) { amount * 0.19; // ← W111: Magic number 0.19 }
- **3. Complexity Metrics (W120-W129):** **W121: Cyclomatic Complexity** ```flux fn process(x) { // ← W121: Cyclomatic complexity: 15 (max: 10) if x > 0 { if x < 10 { // ... many branches } } } ```
- **4. Style & Consistency (W130-W139):** **W131: Inconsistent Naming** ```flux let user_name = "Alice"; let userName = "Bob"; // ← W131: Inconsistent naming style ```

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Limitations

- ❌ No configuration support
- ❌ No unused function result detection
- ❌ No complexity metrics
- ❌ No magic number detection
- ❌ No documentation coverage
- ❌ No custom rule support

### Limitations

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- **Rust clippy:** https://github.com/rust-lang/rust-clippy
- **ESLint:** https://eslint.org/
- **RuboCop:** https://rubocop.org/
- **SonarQube:** https://www.sonarqube.org/

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### Out of Scope (Future)

**Post v0.2.0:**
- Auto-fix suggestions
- Machine learning-based suggestions
- Team/org-wide lint profiles
- Git pre-commit hook integration
- CI/CD integration helpers
- Code coverage integration
- Security vulnerability detection

### Out of Scope (Future)
