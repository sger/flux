use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Display helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format an `InferType` for user-facing diagnostics, resolving ADT symbols
/// to their human-readable names via the interner. Unresolved type variables
/// display as `_` (unknown type).
pub fn display_infer_type(ty: &InferType, interner: &Interner) -> String {
    match ty {
        InferType::Var(_) => "_".to_string(),
        InferType::Con(c) => display_type_constructor(c, interner),
        InferType::App(c, args) => {
            let base = display_type_constructor(c, interner);
            let args_str: Vec<String> = args
                .iter()
                .map(|a| display_infer_type(a, interner))
                .collect();
            format!("{}<{}>", base, args_str.join(", "))
        }
        InferType::Fun(params, ret, effects) => {
            let params_str: Vec<String> = params
                .iter()
                .map(|p| display_infer_type(p, interner))
                .collect();
            let ret_str = display_infer_type(ret, interner);
            if effects.concrete().is_empty() && effects.tail().is_none() {
                format!("({}) -> {}", params_str.join(", "), ret_str)
            } else {
                let mut concrete: Vec<_> = effects.concrete().iter().copied().collect();
                concrete.sort_by_key(|s| s.as_u32());
                let mut eff_str: Vec<String> = concrete
                    .into_iter()
                    .map(|e| interner.resolve(e).to_string())
                    .collect();
                if let Some(tail) = effects.tail() {
                    eff_str.push(format!("|?{tail}"));
                }
                format!(
                    "({}) -> {} with {}",
                    params_str.join(", "),
                    ret_str,
                    eff_str.join(", ")
                )
            }
        }
        InferType::Tuple(elems) => {
            let elems_str: Vec<String> = elems
                .iter()
                .map(|e| display_infer_type(e, interner))
                .collect();
            format!("({})", elems_str.join(", "))
        }
    }
}

/// Render a type constructor, resolving ADT symbols through the interner.
pub(super) fn display_type_constructor(c: &TypeConstructor, interner: &Interner) -> String {
    match c {
        TypeConstructor::Adt(sym) => interner.resolve(*sym).to_string(),
        _ => c.to_string(),
    }
}

/// Built-in type names used for "did you mean?" suggestions.
pub(super) const KNOWN_TYPE_NAMES: &[&str] = &[
    "Int", "Float", "Bool", "String", "Unit", "List", "Map", "Array", "Option", "Either",
];

/// If a type name looks like a typo of a known built-in type, return a
/// suggestion string like `did you mean \`String\`?`.
pub fn suggest_type_name(name: &str) -> Option<String> {
    // Don't suggest for known types or very short names
    if KNOWN_TYPE_NAMES.contains(&name) || name.len() < 2 {
        return None;
    }
    let best = KNOWN_TYPE_NAMES
        .iter()
        .filter_map(|&known| {
            let d = levenshtein_distance(name, known);
            // Allow distance ≤ 2, or prefix match
            if d <= 2 || known.starts_with(name) || name.starts_with(known) {
                Some((d, known))
            } else {
                None
            }
        })
        .min_by_key(|(d, _)| *d);

    best.map(|(_, known)| format!("did you mean `{known}`?"))
}
