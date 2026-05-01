//! Proposal 0174 Phase 2: structure-driven `deriving (Encode, Decode)` synthesis.
//!
//! Walks every `Statement::Data` whose `deriving` list mentions `Encode` or
//! `Decode` and synthesizes a matching `Statement::Instance` block whose body
//! traverses the data type's variants and fields.
//!
//! This pass runs after parsing and before Phase 1b dispatch generation, so the
//! synthesized instances flow through the same pipeline as hand-written ones —
//! class collection, mangled `__tc_*` generation, dictionary elaboration, and
//! qualified-name lookup all see them uniformly.
//!
//! Supported shapes (V1):
//!   - Single-variant named-record `data Foo { Foo { f1: T1, ... } }`. The
//!     synthesized encoder emits a JSON object with field names as keys; the
//!     decoder pulls each field via `require_field` + the matching `to_*`
//!     helper, threading `Either<JsonError, _>` through a nested-match
//!     cascade that short-circuits on the first `Left`.
//!
//! Field types are mapped syntactically to the appropriate primitive helper
//! (`Int → from_int`, `String → from_string`, `Bool → from_bool`); other types
//! recurse through the class itself (`encode(field)`), which dispatches to
//! whichever instance the user has provided.

use crate::syntax::{
    Identifier,
    block::Block,
    data_variant::DataVariant,
    expression::{Expression, ExprIdGen, MatchArm, NamedFieldInit, NamedFieldPattern, Pattern},
    interner::Interner,
    program::Program,
    statement::Statement,
    type_class::InstanceMethod,
    type_expr::TypeExpr,
};
use crate::diagnostics::position::Span;

const ENCODE_CLASS: &str = "Encode";
const DECODE_CLASS: &str = "Decode";

pub fn derive_codec_instances_in_program(program: &mut Program, interner: &mut Interner) {
    // Allocate fresh ExprIds beyond every id the parser produced. Without
    // this, every synthesized expression carries `ExprId::UNSET` (= 0) and
    // the inferrer's `hm_expr_types` table conflates them, which corrupts
    // the type of any user expression that happens to also share id 0.
    let mut id_gen = ExprIdGen::resuming_past_program(program);
    let mut synthesized: Vec<(Option<Identifier>, Statement)> = Vec::new();
    collect_from_statements(&program.statements, None, interner, &mut id_gen, &mut synthesized);
    // Remove `Encode` / `Decode` entries from each `data` declaration's
    // `deriving` list so `class_env::collect_deriving` doesn't register a
    // placeholder InstanceDef that would conflict with our explicit synthesis.
    strip_codec_deriving_in_program(program, interner);
    if synthesized.is_empty() {
        return;
    }
    inject_synthesized_instances(program, synthesized);
}

fn strip_codec_deriving_in_program(program: &mut Program, interner: &mut Interner) {
    for stmt in &mut program.statements {
        strip_codec_deriving_in_stmt(stmt, interner);
    }
}

fn strip_codec_deriving_in_stmt(stmt: &mut Statement, interner: &mut Interner) {
    match stmt {
        Statement::Data { deriving, .. } => {
            deriving.retain(|class_name| {
                let class_str = interner.resolve(*class_name);
                let short = class_str.rsplit('.').next().unwrap_or(class_str);
                short != ENCODE_CLASS && short != DECODE_CLASS
            });
        }
        Statement::Module { body, .. } => {
            for nested in &mut body.statements {
                strip_codec_deriving_in_stmt(nested, interner);
            }
        }
        _ => {}
    }
}

fn collect_from_statements(
    statements: &[Statement],
    enclosing_module: Option<Identifier>,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
    out: &mut Vec<(Option<Identifier>, Statement)>,
) {
    for stmt in statements {
        match stmt {
            Statement::Data {
                name,
                variants,
                deriving,
                span,
                ..
            } if !deriving.is_empty() => {
                for class_name in deriving {
                    let class_str = interner.resolve(*class_name);
                    let class_short = class_str.rsplit('.').next().unwrap_or(class_str);
                    if class_short == ENCODE_CLASS {
                        if let Some(instance) =
                            try_derive_encode(*name, variants, *span, interner, id_gen)
                        {
                            out.push((enclosing_module, instance));
                        }
                    } else if class_short == DECODE_CLASS {
                        if let Some(instance) =
                            try_derive_decode(*name, variants, *span, interner, id_gen)
                        {
                            out.push((enclosing_module, instance));
                        }
                    }
                }
            }
            Statement::Module { name, body, .. } => {
                collect_from_statements(&body.statements, Some(*name), interner, id_gen, out);
            }
            _ => {}
        }
    }
}

fn try_derive_encode(
    adt_name: Identifier,
    variants: &[DataVariant],
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Option<Statement> {
    // V1 supports a single named-record variant only.
    if variants.len() != 1 {
        return None;
    }
    let variant = &variants[0];
    let field_names = variant.field_names.as_ref()?;
    if field_names.len() != variant.fields.len() {
        return None;
    }

    let body = build_encode_body(variant.name, field_names, &variant.fields, span, interner, id_gen);
    let method = InstanceMethod {
        name: interner.intern("encode"),
        params: vec![interner.intern("__derive_x")],
        effects: Vec::new(),
        body,
        span,
    };
    Some(Statement::Instance {
        is_public: true,
        class_name: interner.intern(ENCODE_CLASS),
        type_args: vec![TypeExpr::Named {
            name: adt_name,
            args: Vec::new(),
            span,
        }],
        context: Vec::new(),
        methods: vec![method],
        span,
    })
}

/// Build:
/// ```ignore
/// {
///     match __derive_x {
///         Variant { f1, f2, ... } -> Flow.JsonCodec.from_object({
///             "f1": <encode_field>(f1),
///             "f2": <encode_field>(f2),
///             ...
///         })
///     }
/// }
/// ```
fn build_encode_body(
    variant_name: Identifier,
    field_names: &[Identifier],
    field_types: &[TypeExpr],
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Block {
    let scrutinee = ident_expr(interner.intern("__derive_x"), span, id_gen);

    // Pattern: Variant { f1: __derive_f1, f2: __derive_f2, ... }
    // Use explicit pattern bodies (rather than punning) so the bound
    // identifier doesn't collide with anything in the surrounding scope.
    let mut bound_field_names: Vec<Identifier> = Vec::with_capacity(field_names.len());
    let pattern_fields: Vec<NamedFieldPattern> = field_names
        .iter()
        .map(|name| {
            let bound_str = format!("__derive_f_{}", interner.resolve(*name));
            let bound = interner.intern(&bound_str);
            bound_field_names.push(bound);
            NamedFieldPattern {
                name: *name,
                pattern: Some(Pattern::Identifier { name: bound, span }),
                span,
            }
        })
        .collect();
    let pattern = Pattern::NamedConstructor {
        name: variant_name,
        fields: pattern_fields,
        rest: false,
        span,
    };

    // Hash literal pairs: ("field_name", <encode_field>(__derive_f_<name>))
    let hash_pairs: Vec<(Expression, Expression)> = field_names
        .iter()
        .zip(bound_field_names.iter())
        .zip(field_types.iter())
        .map(|((name, bound), ty)| {
            let key = Expression::String {
                value: interner.resolve(*name).to_string(),
                span,
                id: id_gen.next_id(),
            };
            let field_value = ident_expr(*bound, span, id_gen);
            let encoded = encode_field_call(field_value, ty, span, interner, id_gen);
            (key, encoded)
        })
        .collect();
    let hash = Expression::Hash {
        pairs: hash_pairs,
        span,
        id: id_gen.next_id(),
    };

    // from_object(<hash>) — relies on `Flow.JsonCodec` being exposed via
    // the user's `import Flow.JsonCodec exposing (..)` (or matching auto-
    // expose). The deriving pass leaves resolution of the helpers to the
    // standard module-import machinery rather than emitting a hard-coded
    // `Flow.JsonCodec.` prefix; that way users who alias the codec module
    // (or shadow the helper names) still see the same resolution rules.
    let from_object_call = bare_call("from_object", vec![hash], span, interner, id_gen);

    let arm = MatchArm {
        pattern,
        guard: None,
        body: from_object_call,
        span,
    };
    let match_expr = Expression::Match {
        scrutinee: Box::new(scrutinee),
        arms: vec![arm],
        span,
        id: id_gen.next_id(),
    };

    Block {
        statements: vec![Statement::Expression {
            expression: match_expr,
            has_semicolon: false,
            span,
        }],
        span,
    }
}

fn encode_field_call(
    field_value: Expression,
    field_type: &TypeExpr,
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Expression {
    match field_type {
        TypeExpr::Named { name, args, .. } if args.is_empty() => {
            let resolved = interner.resolve(*name);
            let helper = match resolved {
                "Int" => Some("from_int"),
                "String" => Some("from_string"),
                "Bool" => Some("from_bool"),
                _ => None,
            };
            if let Some(helper) = helper {
                bare_call(helper, vec![field_value], span, interner, id_gen)
            } else {
                bare_call("encode", vec![field_value], span, interner, id_gen)
            }
        }
        _ => bare_call("encode", vec![field_value], span, interner, id_gen),
    }
}

fn bare_call(
    member: &str,
    arguments: Vec<Expression>,
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Expression {
    let function = ident_expr(interner.intern(member), span, id_gen);
    Expression::Call {
        function: Box::new(function),
        arguments,
        span,
        id: id_gen.next_id(),
    }
}

fn ident_expr(name: Identifier, span: Span, id_gen: &mut ExprIdGen) -> Expression {
    Expression::Identifier {
        name,
        span,
        id: id_gen.next_id(),
    }
}

// ── Decode synthesis ────────────────────────────────────────────────
//
// V1 supports a single named-record variant whose fields are primitives or
// recurse through `decode`. The body shape is a nested match cascade over
// `Either<JsonError, _>`: each step is `match <step_call> { Left(e) -> Left(e),
// Right(__derive_v_NAME) -> <next step> }`. The leaf step is
// `Right(Variant { f1: __derive_d_f1, f2: __derive_d_f2, ... })`.

fn try_derive_decode(
    adt_name: Identifier,
    variants: &[DataVariant],
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Option<Statement> {
    if variants.len() != 1 {
        return None;
    }
    let variant = &variants[0];
    let field_names = variant.field_names.as_ref()?;
    if field_names.len() != variant.fields.len() {
        return None;
    }

    let body = build_decode_body(variant.name, field_names, &variant.fields, span, interner, id_gen);
    let method = InstanceMethod {
        name: interner.intern("decode"),
        params: vec![interner.intern("__derive_j")],
        effects: Vec::new(),
        body,
        span,
    };
    Some(Statement::Instance {
        is_public: true,
        class_name: interner.intern(DECODE_CLASS),
        type_args: vec![TypeExpr::Named {
            name: adt_name,
            args: Vec::new(),
            span,
        }],
        context: Vec::new(),
        methods: vec![method],
        span,
    })
}

fn build_decode_body(
    variant_name: Identifier,
    field_names: &[Identifier],
    field_types: &[TypeExpr],
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Block {
    let json_var = interner.intern("__derive_j");
    let obj_var = interner.intern("__derive_obj");

    // Decoded-field bound names: __derive_d_<field>
    let decoded_names: Vec<Identifier> = field_names
        .iter()
        .map(|name| {
            let s = format!("__derive_d_{}", interner.resolve(*name));
            interner.intern(&s)
        })
        .collect();

    // Final step: Right(Variant { f1: __derive_d_f1, ... })
    let final_fields: Vec<NamedFieldInit> = field_names
        .iter()
        .zip(decoded_names.iter())
        .map(|(name, bound)| NamedFieldInit {
            name: *name,
            value: Some(Box::new(ident_expr(*bound, span, id_gen))),
            span,
        })
        .collect();
    let constructed = Expression::NamedConstructor {
        name: variant_name,
        fields: final_fields,
        span,
        id: id_gen.next_id(),
    };
    let mut chain = right_call(constructed, span, id_gen);

    // Build the cascade in reverse so each step wraps the previously built
    // continuation. For each field, two nested matches: require_field then
    // type-specific decoder.
    for ((field_name, decoded_bound), field_type) in field_names
        .iter()
        .zip(decoded_names.iter())
        .zip(field_types.iter())
        .rev()
    {
        let value_bound = {
            let s = format!("__derive_v_{}", interner.resolve(*field_name));
            interner.intern(&s)
        };

        // Step B: match <decode_helper>(__derive_v_<name>) {
        //   Left(e) -> Left(e),
        //   Right(__derive_d_<name>) -> <chain so far>
        // }
        let decode_call = decode_field_call(
            ident_expr(value_bound, span, id_gen),
            field_type,
            span,
            interner,
            id_gen,
        );
        let step_b = either_match(decode_call, *decoded_bound, chain, span, interner, id_gen);

        // Step A: match require_field(__derive_obj, "<name>") {
        //   Left(e) -> Left(e),
        //   Right(__derive_v_<name>) -> step_b
        // }
        let key_lit = Expression::String {
            value: interner.resolve(*field_name).to_string(),
            span,
            id: id_gen.next_id(),
        };
        let require_call = bare_call(
            "require_field",
            vec![ident_expr(obj_var, span, id_gen), key_lit],
            span,
            interner,
            id_gen,
        );
        let step_a = either_match(require_call, value_bound, step_b, span, interner, id_gen);

        chain = step_a;
    }

    // Outermost: match to_object(__derive_j) {
    //   Left(e) -> Left(e),
    //   Right(__derive_obj) -> <field cascade>
    // }
    let to_object_call = bare_call(
        "to_object",
        vec![ident_expr(json_var, span, id_gen)],
        span,
        interner,
        id_gen,
    );
    let outer = either_match(to_object_call, obj_var, chain, span, interner, id_gen);

    Block {
        statements: vec![Statement::Expression {
            expression: outer,
            has_semicolon: false,
            span,
        }],
        span,
    }
}

/// Build:
/// ```ignore
/// match <scrutinee> {
///     Left(__derive_e) -> Left(__derive_e),
///     Right(<bind>) -> <on_right>
/// }
/// ```
fn either_match(
    scrutinee: Expression,
    bind: Identifier,
    on_right: Expression,
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Expression {
    let err_bind = interner.intern("__derive_e");
    let left_pat = Pattern::Left {
        pattern: Box::new(Pattern::Identifier {
            name: err_bind,
            span,
        }),
        span,
    };
    let left_body = Expression::Left {
        value: Box::new(ident_expr(err_bind, span, id_gen)),
        span,
        id: id_gen.next_id(),
    };
    let right_pat = Pattern::Right {
        pattern: Box::new(Pattern::Identifier { name: bind, span }),
        span,
    };
    Expression::Match {
        scrutinee: Box::new(scrutinee),
        arms: vec![
            MatchArm {
                pattern: left_pat,
                guard: None,
                body: left_body,
                span,
            },
            MatchArm {
                pattern: right_pat,
                guard: None,
                body: on_right,
                span,
            },
        ],
        span,
        id: id_gen.next_id(),
    }
}

fn right_call(value: Expression, span: Span, id_gen: &mut ExprIdGen) -> Expression {
    Expression::Right {
        value: Box::new(value),
        span,
        id: id_gen.next_id(),
    }
}

fn decode_field_call(
    field_value: Expression,
    field_type: &TypeExpr,
    span: Span,
    interner: &mut Interner,
    id_gen: &mut ExprIdGen,
) -> Expression {
    match field_type {
        TypeExpr::Named { name, args, .. } if args.is_empty() => {
            let resolved = interner.resolve(*name);
            let helper = match resolved {
                "Int" => Some("to_int"),
                "String" => Some("to_string_value"),
                "Bool" => Some("to_bool"),
                _ => None,
            };
            if let Some(helper) = helper {
                bare_call(helper, vec![field_value], span, interner, id_gen)
            } else {
                bare_call("decode", vec![field_value], span, interner, id_gen)
            }
        }
        _ => bare_call("decode", vec![field_value], span, interner, id_gen),
    }
}

fn inject_synthesized_instances(
    program: &mut Program,
    synthesized: Vec<(Option<Identifier>, Statement)>,
) {
    let mut top_level: Vec<Statement> = Vec::new();
    let mut by_module: std::collections::HashMap<Identifier, Vec<Statement>> =
        std::collections::HashMap::new();
    for (module, stmt) in synthesized {
        match module {
            Some(module_sym) => by_module.entry(module_sym).or_default().push(stmt),
            None => top_level.push(stmt),
        }
    }

    let mut new_statements: Vec<Statement> = Vec::with_capacity(program.statements.len() + 4);
    let mut iter = std::mem::take(&mut program.statements).into_iter().peekable();
    // Preserve any leading `Statement::Import` declarations; insert top-level
    // synthesized instances *after* them so the generated bodies (which call
    // `Flow.JsonCodec.from_*`) can resolve the module reference.
    while let Some(stmt) = iter.peek() {
        if matches!(stmt, Statement::Import { .. }) {
            new_statements.push(iter.next().unwrap());
        } else {
            break;
        }
    }
    new_statements.append(&mut top_level);
    for stmt in iter {
        match stmt {
            Statement::Module { name, body, span } => {
                let mut new_body = body.statements;
                if let Some(extras) = by_module.remove(&name) {
                    new_body.extend(extras);
                }
                new_statements.push(Statement::Module {
                    name,
                    body: Block {
                        statements: new_body,
                        span: body.span,
                    },
                    span,
                });
            }
            other => new_statements.push(other),
        }
    }
    // Any leftover module-keyed instances had no matching `Statement::Module` —
    // dump them at top level for safety.
    for (_, mut extras) in by_module {
        new_statements.append(&mut extras);
    }
    program.statements = new_statements;
}
