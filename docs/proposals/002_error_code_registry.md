# Diagnostic Error Catalog (TOML)

## Goal
Centralize diagnostic titles/messages/hints in a data file so compiler/linter/runtime code no longer hardcodes strings. The code only references error codes and variable values; the catalog owns the human-facing text.

## Motivation
- Avoid duplicated strings and inconsistent wording.
- Make it easy to review/edit diagnostics in one place.
- Keep error code definitions stable while improving UX.
- Enable localization or future tooling without touching compiler logic.

## Proposed Format
Use TOML for readability and comments.

```
[errors.E007]
title = "UNDEFINED VARIABLE"
message = "I can't find a value named `{name}`."
hint = "Define it first: let {name} = ...;"

[errors.E021]
title = "PRIVATE MEMBER"
message = "Cannot access private member `{member}`."
hint = "Private members can only be accessed within the same module."
```

- `errors.<CODE>` is the key.
- `title` is the diagnostic header.
- `message` is the primary error text.
- `hint` is optional (omit if not needed).
- Placeholders use `{name}`, `{member}`, etc.

## Resolution Model
At runtime (or startup), load the catalog and render messages by substituting placeholders.

Pseudo API:
```
// Example usage inside compiler/linter
let spec = catalog::spec("E007")?;
Diagnostic::error(spec.title)
  .with_code("E007")
  .with_message(catalog::format(spec.message, [ ("name", var) ]))
  .with_hint(spec.hint.map(|h| catalog::format(h, [ ("name", var) ])))
```

## Recommended Integration
1) Keep the existing `error_codes.rs` for code/title canonical list.
2) Add a new `error_catalog.toml` file under `resources/` (or `docs/` if you prefer).
3) Add a small loader + formatter module (e.g. `frontend/error_catalog.rs`).
4) Migrate a few errors first (E007, E021) and expand gradually.

## Implementation Details (proposed)

### Files
- `resources/error_catalog.toml`: catalog data.
- `src/frontend/error_catalog.rs`: loader + formatter.
- `src/frontend/mod.rs`: export `error_catalog`.
- `tests/error_catalog_tests.rs`: ensure catalog is complete and valid.

### Loader Module
```
use std::{collections::HashMap, sync::OnceLock};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CatalogFile {
    errors: HashMap<String, ErrorEntry>,
}

#[derive(Debug, Deserialize, Clone)]
struct ErrorEntry {
    title: String,
    message: String,
    hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ErrorSpec {
    pub title: String,
    pub message: String,
    pub hint: Option<String>,
}

static CATALOG: OnceLock<CatalogFile> = OnceLock::new();

fn load_catalog() -> &'static CatalogFile {
    CATALOG.get_or_init(|| {
        let raw = include_str!("../../resources/error_catalog.toml");
        toml::from_str(raw).expect("invalid error_catalog.toml")
    })
}

pub fn spec(code: &str) -> Option<ErrorSpec> {
    let catalog = load_catalog();
    catalog.errors.get(code).map(|entry| ErrorSpec {
        title: entry.title.clone(),
        message: entry.message.clone(),
        hint: entry.hint.clone(),
    })
}

pub fn format(template: &str, vars: &[(&str, &str)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in vars {
        let needle = format!("{{{}}}", key);
        rendered = rendered.replace(&needle, value);
    }
    rendered
}
```

Dependencies:
```
serde = { version = "1.0", features = ["derive"] }
toml = "0.8"
```

### Usage Example (compiler)
```
let spec = error_catalog::spec("E007").expect("missing E007");
Diagnostic::error(spec.title)
  .with_code("E007")
  .with_file(self.file_path.clone())
  .with_position(position)
  .with_message(error_catalog::format(&spec.message, &[("name", name)]))
  .with_hint(spec.hint.map(|h| error_catalog::format(&h, &[("name", name)])));
```

### Alternative: Enum-Based Catalog (no external file)
```
// src/frontend/diagnostics/catalog.rs
use crate::frontend::diagnostic::Diagnostic;
use crate::frontend::position::Position;

#[derive(Debug, Clone, Copy)]
pub enum ErrorCode {
    E007, // undefined variable
    // ...
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::E007 => "E007",
        }
    }
}

pub struct ErrorSpec {
    pub title: &'static str,
    pub message: &'static str,
    pub hint: Option<&'static str>,
}

pub fn spec(code: ErrorCode) -> ErrorSpec {
    match code {
        ErrorCode::E007 => ErrorSpec {
            title: "UNDEFINED VARIABLE",
            message: "I can't find a value named `{}`.",
            hint: Some("Define it first: let {} = ...;"),
        },
    }
}

// Example helper (typed, easy to call)
pub fn undefined_variable(file: &str, position: Position, name: &str) -> Diagnostic {
    let spec = spec(ErrorCode::E007);
    Diagnostic::error(spec.title)
        .with_code(ErrorCode::E007.as_str())
        .with_file(file.to_string())
        .with_position(position)
        .with_message(format!(spec.message, name))
        .with_hint(spec.hint.map(|h| format!(h, name)))
}
```

### Tests
- `error_catalog_loads`: ensure TOML parses.
- `error_catalog_has_all_codes`: ensure `error_codes::ERROR_CODES` all exist in TOML.
- `error_catalog_templates`: spot-check placeholder rendering.

## Pros / Cons
Pros:
- No hardcoded strings in compiler/linter/runtime.
- Consistent phrasing and easy edits.
- Future localization possible without refactoring.

Cons:
- Requires a loader and runtime access.
- Missing keys become runtime errors (mitigate with tests).

## Tests to Add
- Catalog loads successfully.
- Each code in `error_codes.rs` exists in the catalog.
- Optional: snapshot test for a few diagnostics.

## Open Questions
- Should catalog live in `resources/` or `docs/`?
- Should `error_codes.rs` be generated from the catalog to prevent drift?
- Should placeholders be enforced (e.g., compile-time tests)?
