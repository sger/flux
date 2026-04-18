use std::collections::HashMap;

use super::*;
use crate::{syntax::Identifier, types::scheme::Scheme};

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
        InferType::HktApp(head, args) => {
            let head_str = display_infer_type(head, interner);
            let args_str: Vec<String> = args
                .iter()
                .map(|a| display_infer_type(a, interner))
                .collect();
            format!("{}<{}>", head_str, args_str.join(", "))
        }
    }
}

/// Return the canonical display name for the `index`th quantified type variable.
fn alpha_name(index: usize) -> String {
    let letter = ((index % 26) as u8 + b'a') as char;
    let suffix = index / 26;
    if suffix == 0 {
        letter.to_string()
    } else {
        format!("{letter}{suffix}")
    }
}

struct CanonicalSchemeFormatter<'a> {
    interner: &'a Interner,
    names: HashMap<u32, String>,
    next: usize,
}

impl<'a> CanonicalSchemeFormatter<'a> {
    /// Create a formatter that canonicalizes type-variable names through `interner`.
    fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            names: HashMap::new(),
            next: 0,
        }
    }

    /// Intern a stable display name for a type or effect-row tail variable id.
    fn intern_var_name(&mut self, id: u32) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let name = alpha_name(self.next);
        self.next += 1;
        self.names.insert(id, name.clone());
        name
    }

    /// Resolve an identifier into a readable class or ADT name for display.
    fn identifier_name(&self, sym: Identifier) -> String {
        self.interner
            .try_resolve(sym)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{sym}"))
    }

    /// Render a constructor name, resolving ADT symbols through the interner.
    fn format_constructor(&self, constructor: &TypeConstructor) -> String {
        match constructor {
            TypeConstructor::Adt(sym) => self.identifier_name(*sym),
            other => other.to_string(),
        }
    }

    /// Assign canonical variable names in first-use order while walking a type.
    fn note_type_order(&mut self, infer_type: &InferType) {
        match infer_type {
            InferType::Var(id) => {
                self.intern_var_name(*id);
            }
            InferType::Con(_) => {}
            InferType::App(_, args) => {
                for arg in args {
                    self.note_type_order(arg);
                }
            }
            InferType::Fun(params, ret, effects) => {
                for param in params {
                    self.note_type_order(param);
                }
                self.note_type_order(ret);
                if let Some(tail) = effects.tail() {
                    self.intern_var_name(tail);
                }
            }
            InferType::Tuple(elements) => {
                for element in elements {
                    self.note_type_order(element);
                }
            }
            InferType::HktApp(head, args) => {
                self.note_type_order(head);
                for arg in args {
                    self.note_type_order(arg);
                }
            }
        }
    }

    /// Assign canonical variable names for constraint variables in sorted class order.
    fn note_constraint_order(&mut self, scheme: &Scheme) {
        let mut constraints = scheme.constraints.iter().collect::<Vec<_>>();
        constraints.sort_by_key(|constraint| self.identifier_name(constraint.class_name));
        for constraint in constraints {
            for var in &constraint.type_vars {
                self.intern_var_name(*var);
            }
        }
    }

    /// Render an effect row using the formatter's canonical variable names.
    fn format_effects(&mut self, effects: &InferEffectRow) -> String {
        let mut concrete: Vec<_> = effects
            .concrete()
            .iter()
            .map(|effect| self.identifier_name(*effect))
            .collect();
        concrete.sort();
        match effects.tail() {
            Some(tail) if concrete.is_empty() => format!("|{}", self.intern_var_name(tail)),
            Some(tail) => format!("{}, |{}", concrete.join(", "), self.intern_var_name(tail)),
            None => concrete.join(", "),
        }
    }

    /// Render an inferred type using canonicalized variable names.
    fn format_type(&mut self, infer_type: &InferType) -> String {
        match infer_type {
            InferType::Var(id) => self.intern_var_name(*id),
            InferType::Con(constructor) => self.format_constructor(constructor),
            InferType::App(constructor, args) => format!(
                "{}<{}>",
                self.format_constructor(constructor),
                args.iter()
                    .map(|arg| self.format_type(arg))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            InferType::Fun(params, ret, effects) => {
                let params = params
                    .iter()
                    .map(|param| self.format_type(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut rendered = format!("({params}) -> {}", self.format_type(ret));
                if !effects.concrete().is_empty() || effects.tail().is_some() {
                    rendered.push_str(" with ");
                    rendered.push_str(&self.format_effects(effects));
                }
                rendered
            }
            InferType::Tuple(elements) => format!(
                "({})",
                elements
                    .iter()
                    .map(|element| self.format_type(element))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            InferType::HktApp(head, args) => format!(
                "{}<{}>",
                self.format_type(head),
                args.iter()
                    .map(|arg| self.format_type(arg))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Render scheme constraints in canonical sorted order.
    fn format_constraints(&mut self, scheme: &Scheme) -> String {
        if scheme.constraints.is_empty() {
            return String::new();
        }
        let mut rendered = scheme
            .constraints
            .iter()
            .map(|constraint| {
                let class_name = self.identifier_name(constraint.class_name);
                let args = constraint
                    .type_vars
                    .iter()
                    .map(|var| self.intern_var_name(*var))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{class_name}<{args}>")
            })
            .collect::<Vec<_>>();
        rendered.sort();
        format!("{} => ", rendered.join(", "))
    }
}

/// Format a `Scheme` with deterministic quantifier names and constraint order.
pub fn render_scheme_canonical(interner: &Interner, scheme: &Scheme) -> String {
    let mut formatter = CanonicalSchemeFormatter::new(interner);
    formatter.note_type_order(&scheme.infer_type);
    formatter.note_constraint_order(scheme);

    let constraints = formatter.format_constraints(scheme);
    let ty = formatter.format_type(&scheme.infer_type);
    let mut forall = scheme.forall.clone();
    forall.extend(scheme.infer_type.free_vars());
    for constraint in &scheme.constraints {
        forall.extend(constraint.type_vars.iter().copied());
    }
    forall.sort_unstable();
    forall.dedup();

    if forall.is_empty() {
        format!("{constraints}{ty}")
    } else {
        let mut vars = forall
            .iter()
            .map(|var| formatter.intern_var_name(*var))
            .collect::<Vec<_>>();
        vars.sort();
        let vars = vars.join(", ");
        format!("forall {vars}. {constraints}{ty}")
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

#[cfg(test)]
mod tests {
    use crate::{
        ast::type_infer::{constraint::SchemeConstraint, render_scheme_canonical},
        syntax::interner::Interner,
        types::{
            infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme,
            type_constructor::TypeConstructor,
        },
    };

    #[test]
    fn render_scheme_canonical_normalizes_quantified_ids_across_runs() {
        let interner = Interner::new();
        let first = Scheme {
            forall: vec![3, 7],
            constraints: Vec::new(),
            infer_type: InferType::Fun(
                vec![InferType::Var(3), InferType::Var(7)],
                Box::new(InferType::Var(3)),
                InferEffectRow::closed_empty(),
            ),
        };
        let second = Scheme {
            forall: vec![41, 2],
            constraints: Vec::new(),
            infer_type: InferType::Fun(
                vec![InferType::Var(41), InferType::Var(2)],
                Box::new(InferType::Var(41)),
                InferEffectRow::closed_empty(),
            ),
        };

        assert_eq!(
            render_scheme_canonical(&interner, &first),
            "forall a, b. (a, b) -> a"
        );
        assert_eq!(
            render_scheme_canonical(&interner, &first),
            render_scheme_canonical(&interner, &second)
        );
    }

    #[test]
    fn render_scheme_canonical_sorts_constraints_and_shares_names() {
        let mut interner = Interner::new();
        let eq = interner.intern("Eq");
        let num = interner.intern("Num");
        let scheme = Scheme {
            forall: vec![9],
            constraints: vec![
                SchemeConstraint {
                    class_name: num,
                    type_vars: vec![9],
                },
                SchemeConstraint {
                    class_name: eq,
                    type_vars: vec![9],
                },
            ],
            infer_type: InferType::Fun(
                vec![InferType::Var(9)],
                Box::new(InferType::Con(TypeConstructor::Bool)),
                InferEffectRow::closed_empty(),
            ),
        };

        assert_eq!(
            render_scheme_canonical(&interner, &scheme),
            "forall a. Eq<a>, Num<a> => (a) -> Bool"
        );
    }

    #[test]
    fn render_scheme_canonical_orders_forall_by_assigned_names_not_raw_ids() {
        let interner = Interner::new();
        let scheme = Scheme {
            forall: vec![30, 10, 20],
            constraints: Vec::new(),
            infer_type: InferType::Fun(
                vec![InferType::Var(30)],
                Box::new(InferType::Fun(
                    vec![InferType::Var(20)],
                    Box::new(InferType::Var(10)),
                    InferEffectRow::open_from_symbols(std::iter::empty(), 20),
                )),
                InferEffectRow::closed_empty(),
            ),
        };

        assert_eq!(
            render_scheme_canonical(&interner, &scheme),
            "forall a, b, c. (a) -> (b) -> c with |b"
        );
    }
}
