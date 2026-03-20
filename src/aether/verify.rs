//! Aether verification.
//!
//! This module has two responsibilities:
//! - `verify_contract`: fatal semantic-contract checks for emitted Aether Core
//! - `verify_diagnostics`: optional non-fatal optimization diagnostics

use crate::core::{CoreExpr, CorePat, CoreTag};

use super::analysis::use_counts;
use super::{constructor_shape_for_tag, is_heap_tag};

/// Fatal Aether verification error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AetherError {
    pub kind: AetherErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AetherErrorKind {
    UnresolvedAetherVar,
    UnsafeDrop,
    InvalidReuseTag,
    ReuseTokenEscapesIntoFields,
    InvalidFieldMask,
    InvalidDropSpecializedScrutinee,
    InvalidDropSpecializedUse,
}

/// Non-fatal Aether diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AetherDiagnostic {
    pub kind: AetherDiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AetherDiagnosticKind {
    /// Case arm destructures value and reconstructs same shape — could reuse.
    MissedReuse,
}

/// Verify the fatal Aether contract for an expression.
pub fn verify_contract(expr: &CoreExpr) -> Result<(), Vec<AetherError>> {
    let mut errors = Vec::new();
    check_contract(expr, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Verify optional non-fatal diagnostics for an expression.
pub fn verify_diagnostics(expr: &CoreExpr) -> Vec<AetherDiagnostic> {
    let mut diags = Vec::new();
    check_diagnostics(expr, &mut diags);
    diags
}

fn check_contract(expr: &CoreExpr, errors: &mut Vec<AetherError>) {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => {
            check_contract(body, errors)
        }
        CoreExpr::App { func, args, .. } => {
            check_contract(func, errors);
            for arg in args {
                check_contract(arg, errors);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            check_contract(rhs, errors);
            check_contract(body, errors);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            check_contract(scrutinee, errors);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    check_contract(guard, errors);
                }
                check_contract(&alt.rhs, errors);
            }
        }
        CoreExpr::Con { fields, .. } => {
            for field in fields {
                check_contract(field, errors);
            }
        }
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            for arg in args {
                check_contract(arg, errors);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            check_contract(body, errors);
            for handler in handlers {
                check_contract(&handler.body, errors);
            }
        }
        CoreExpr::Dup { var, body, .. } => {
            if var.binder.is_none() {
                errors.push(AetherError {
                    kind: AetherErrorKind::UnresolvedAetherVar,
                    message: format!("dup uses unresolved variable `{}`", var.name.as_u32()),
                });
            }
            check_contract(body, errors);
        }
        CoreExpr::Drop { var, body, .. } => {
            if let Some(id) = var.binder {
                if let Some(&count) = use_counts(body).get(&id)
                    && count > 0
                {
                    errors.push(AetherError {
                        kind: AetherErrorKind::UnsafeDrop,
                        message: format!(
                            "drop of `{}` is unsafe: variable still has {} use(s) in body",
                            var.name.as_u32(),
                            count
                        ),
                    });
                }
            } else {
                errors.push(AetherError {
                    kind: AetherErrorKind::UnresolvedAetherVar,
                    message: format!("drop uses unresolved variable `{}`", var.name.as_u32()),
                });
            }
            check_contract(body, errors);
        }
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            ..
        } => {
            if token.binder.is_none() {
                errors.push(AetherError {
                    kind: AetherErrorKind::UnresolvedAetherVar,
                    message: format!("reuse uses unresolved token `{}`", token.name.as_u32()),
                });
            }
            if !is_heap_tag(tag) {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidReuseTag,
                    message: format!("reuse uses non-heap constructor tag `{:?}`", tag),
                });
            }
            if let Some(token_id) = token.binder
                && fields
                    .iter()
                    .any(|field| use_counts(field).contains_key(&token_id))
            {
                errors.push(AetherError {
                    kind: AetherErrorKind::ReuseTokenEscapesIntoFields,
                    message: format!(
                        "reuse token `{}` escapes into constructor fields",
                        token.name.as_u32()
                    ),
                });
            }
            if let Some(mask) = field_mask
                && !field_mask_fits(*mask, fields.len())
            {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidFieldMask,
                    message: format!(
                        "reuse field_mask=0b{:b} exceeds constructor arity {}",
                        mask,
                        fields.len()
                    ),
                });
            }
            for field in fields {
                check_contract(field, errors);
            }
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            let Some(scrutinee_id) = scrutinee.binder else {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidDropSpecializedScrutinee,
                    message: format!(
                        "drop_spec uses unresolved scrutinee `{}`",
                        scrutinee.name.as_u32()
                    ),
                });
                check_contract(unique_body, errors);
                check_contract(shared_body, errors);
                return;
            };

            let unique_count = invalid_drop_specialized_uses(unique_body, scrutinee_id);
            if unique_count > 0 {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidDropSpecializedUse,
                    message: format!(
                        "drop_spec unique branch still uses scrutinee `{}` {} time(s)",
                        scrutinee.name.as_u32(),
                        unique_count
                    ),
                });
            }
            let shared_count = invalid_drop_specialized_uses(shared_body, scrutinee_id);
            if shared_count > 0 {
                errors.push(AetherError {
                    kind: AetherErrorKind::InvalidDropSpecializedUse,
                    message: format!(
                        "drop_spec shared branch still uses scrutinee `{}` {} time(s)",
                        scrutinee.name.as_u32(),
                        shared_count
                    ),
                });
            }
            check_contract(unique_body, errors);
            check_contract(shared_body, errors);
        }
    }
}

fn check_diagnostics(expr: &CoreExpr, diags: &mut Vec<AetherDiagnostic>) {
    match expr {
        CoreExpr::Case { scrutinee, alts, .. } => {
            check_diagnostics(scrutinee, diags);
            for alt in alts {
                let destr_tag = pat_constructor_tag(&alt.pat);
                if let Some(destr_tag) = destr_tag
                    && let Some(con_tag) = find_con_in_body(&alt.rhs, Some(&destr_tag))
                    && tags_compatible(&destr_tag, &con_tag)
                    && !has_reuse_for_tag(&alt.rhs, &con_tag)
                {
                    diags.push(AetherDiagnostic {
                        kind: AetherDiagnosticKind::MissedReuse,
                        message: format!(
                            "MISSED REUSE: Case arm destructures {:?} and constructs {:?} — could reuse allocation",
                            destr_tag, con_tag
                        ),
                    });
                }
                check_diagnostics(&alt.rhs, diags);
                if let Some(g) = &alt.guard {
                    check_diagnostics(g, diags);
                }
            }
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => check_diagnostics(body, diags),
        CoreExpr::Reuse { fields, .. } | CoreExpr::Con { fields, .. } => {
            for f in fields {
                check_diagnostics(f, diags);
            }
        }
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => {
            check_diagnostics(body, diags)
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            check_diagnostics(rhs, diags);
            check_diagnostics(body, diags);
        }
        CoreExpr::App { func, args, .. } => {
            check_diagnostics(func, diags);
            for a in args {
                check_diagnostics(a, diags);
            }
        }
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            for a in args {
                check_diagnostics(a, diags);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            check_diagnostics(body, diags);
            for h in handlers {
                check_diagnostics(&h.body, diags);
            }
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            check_diagnostics(unique_body, diags);
            check_diagnostics(shared_body, diags);
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
    }
}

fn field_mask_fits(mask: u64, arity: usize) -> bool {
    if arity >= u64::BITS as usize {
        true
    } else {
        let valid_bits = if arity == 0 { 0 } else { (1u64 << arity) - 1 };
        mask & !valid_bits == 0
    }
}

fn invalid_drop_specialized_uses(expr: &CoreExpr, scrutinee_id: crate::core::CoreBinderId) -> usize {
    match expr {
        CoreExpr::Var { var, .. } => usize::from(var.binder == Some(scrutinee_id)),
        CoreExpr::Lit(_, _) => 0,
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => {
            invalid_drop_specialized_uses(body, scrutinee_id)
        }
        CoreExpr::App { func, args, .. } => {
            invalid_drop_specialized_uses(func, scrutinee_id)
                + args
                    .iter()
                    .map(|arg| invalid_drop_specialized_uses(arg, scrutinee_id))
                    .sum::<usize>()
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            invalid_drop_specialized_uses(rhs, scrutinee_id)
                + invalid_drop_specialized_uses(body, scrutinee_id)
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            invalid_drop_specialized_uses(scrutinee, scrutinee_id)
                + alts
                    .iter()
                    .map(|alt| {
                        invalid_drop_specialized_uses(&alt.rhs, scrutinee_id)
                            + alt
                                .guard
                                .as_ref()
                                .map(|g| invalid_drop_specialized_uses(g, scrutinee_id))
                                .unwrap_or(0)
                    })
                    .sum::<usize>()
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => fields
            .iter()
            .map(|field| invalid_drop_specialized_uses(field, scrutinee_id))
            .sum(),
        CoreExpr::Perform { args, .. } => args
            .iter()
            .map(|arg| invalid_drop_specialized_uses(arg, scrutinee_id))
            .sum(),
        CoreExpr::Handle { body, handlers, .. } => {
            invalid_drop_specialized_uses(body, scrutinee_id)
                + handlers
                    .iter()
                    .map(|h| invalid_drop_specialized_uses(&h.body, scrutinee_id))
                    .sum::<usize>()
        }
        CoreExpr::Dup { var, body, .. } => {
            usize::from(var.binder == Some(scrutinee_id))
                + invalid_drop_specialized_uses(body, scrutinee_id)
        }
        CoreExpr::Drop { var, body, .. } => {
            usize::from(var.binder == Some(scrutinee_id))
                + invalid_drop_specialized_uses(body, scrutinee_id)
        }
        CoreExpr::Reuse { token, fields, .. } => {
            let token_uses = usize::from(token.binder == Some(scrutinee_id)) * 0;
            token_uses
                + fields
                    .iter()
                    .map(|field| invalid_drop_specialized_uses(field, scrutinee_id))
                    .sum::<usize>()
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            usize::from(scrutinee.binder == Some(scrutinee_id))
                + invalid_drop_specialized_uses(unique_body, scrutinee_id)
                + invalid_drop_specialized_uses(shared_body, scrutinee_id)
        }
    }
}

fn pat_constructor_tag(pat: &CorePat) -> Option<CoreTag> {
    match pat {
        CorePat::Con { tag, .. } => Some(tag.clone()),
        _ => None,
    }
}

fn find_con_in_body(expr: &CoreExpr, expected_tag: Option<&CoreTag>) -> Option<CoreTag> {
    match expr {
        CoreExpr::Reuse { tag, .. } => Some(tag.clone()),
        _ if constructor_shape_for_tag(expr, expected_tag).is_some() => {
            constructor_shape_for_tag(expr, expected_tag).map(|(tag, _, _)| tag)
        }
        CoreExpr::Let { body, .. } | CoreExpr::Drop { body, .. } | CoreExpr::Dup { body, .. } => {
            find_con_in_body(body, expected_tag)
        }
        _ => None,
    }
}

fn tags_compatible(a: &CoreTag, b: &CoreTag) -> bool {
    match (a, b) {
        (CoreTag::Cons, CoreTag::Cons) => true,
        (CoreTag::Some, CoreTag::Some) => true,
        (CoreTag::Left, CoreTag::Left) => true,
        (CoreTag::Right, CoreTag::Right) => true,
        (CoreTag::Named(a), CoreTag::Named(b)) => a == b,
        _ => false,
    }
}

fn has_reuse_for_tag(expr: &CoreExpr, tag: &CoreTag) -> bool {
    match expr {
        CoreExpr::Reuse { tag: t, .. } => tags_compatible(t, tag),
        CoreExpr::Let { body, .. } | CoreExpr::Drop { body, .. } | CoreExpr::Dup { body, .. } => {
            has_reuse_for_tag(body, tag)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreLit, CorePat, CoreTag};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::{AetherErrorKind, verify_contract};

    fn binder(raw: u32, name: crate::syntax::Identifier) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), name)
    }

    fn s() -> Span {
        Span::default()
    }

    fn v(binder: CoreBinder) -> CoreExpr {
        CoreExpr::bound_var(binder, s())
    }

    #[test]
    fn contract_rejects_drop_with_remaining_use() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let expr = CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(x),
            body: Box::new(v(x)),
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected unsafe drop");
        assert!(err.iter().any(|e| e.kind == AetherErrorKind::UnsafeDrop));
    }

    #[test]
    fn contract_rejects_reuse_when_token_escapes_into_fields() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let expr = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(x),
            tag: CoreTag::Cons,
            fields: vec![v(x), CoreExpr::Lit(CoreLit::Int(0), s())],
            field_mask: None,
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected token-escape error");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::ReuseTokenEscapesIntoFields)
        );
    }

    #[test]
    fn contract_rejects_reuse_on_non_heap_tag() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let expr = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(x),
            tag: CoreTag::Nil,
            fields: vec![],
            field_mask: None,
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected invalid reuse tag");
        assert!(err.iter().any(|e| e.kind == AetherErrorKind::InvalidReuseTag));
    }

    #[test]
    fn contract_rejects_out_of_range_field_mask() {
        let mut interner = Interner::new();
        let x = binder(1, interner.intern("x"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let expr = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(x),
            tag: CoreTag::Cons,
            fields: vec![v(h), v(t)],
            field_mask: Some(0b100),
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected invalid field mask");
        assert!(err.iter().any(|e| e.kind == AetherErrorKind::InvalidFieldMask));
    }

    #[test]
    fn contract_rejects_drop_specialized_with_unresolved_scrutinee() {
        let mut interner = Interner::new();
        let name = interner.intern("xs");
        let expr = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::unresolved(name),
            unique_body: Box::new(CoreExpr::Lit(CoreLit::Int(1), s())),
            shared_body: Box::new(CoreExpr::Lit(CoreLit::Int(2), s())),
            span: s(),
        };
        let err = verify_contract(&expr).expect_err("expected invalid drop_spec scrutinee");
        assert!(
            err.iter()
                .any(|e| e.kind == AetherErrorKind::InvalidDropSpecializedScrutinee)
        );
    }

    #[test]
    fn contract_accepts_valid_current_shapes() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let color = binder(4, interner.intern("color"));
        let left = binder(5, interner.intern("left"));
        let key = binder(6, interner.intern("key"));
        let right = binder(7, interner.intern("right"));
        let node = CoreTag::Named(interner.intern("Node"));

        let list_reuse = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(xs),
            tag: CoreTag::Cons,
            fields: vec![v(h), v(t)],
            field_mask: Some(0),
            span: s(),
        };
        assert!(verify_contract(&list_reuse).is_ok());

        let named_adt_reuse = CoreExpr::Reuse {
            token: crate::core::CoreVarRef::resolved(xs),
            tag: node.clone(),
            fields: vec![v(color), v(left), v(key), v(right)],
            field_mask: Some(0b1),
            span: s(),
        };
        assert!(verify_contract(&named_adt_reuse).is_ok());

        let list_drop_spec = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(xs),
            unique_body: Box::new(CoreExpr::Reuse {
                token: crate::core::CoreVarRef::resolved(xs),
                tag: CoreTag::Cons,
                fields: vec![v(h), v(t)],
                field_mask: Some(0b10),
                span: s(),
            }),
            shared_body: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(h), v(t)],
                span: s(),
            }),
            span: s(),
        };
        assert!(verify_contract(&list_drop_spec).is_ok());

        let named_adt_drop_spec = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(xs),
            unique_body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(right),
                body: Box::new(CoreExpr::Reuse {
                    token: crate::core::CoreVarRef::resolved(xs),
                    tag: node.clone(),
                    fields: vec![v(color), v(left), v(key), v(left)],
                    field_mask: Some(0b1000),
                    span: s(),
                }),
                span: s(),
            }),
            shared_body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(right),
                body: Box::new(CoreExpr::Con {
                    tag: node.clone(),
                    fields: vec![v(color), v(left), v(key), v(left)],
                    span: s(),
                }),
                span: s(),
            }),
            span: s(),
        };
        assert!(verify_contract(&named_adt_drop_spec).is_ok());

        let keep = binder(8, interner.intern("keep"));
        let branchy_named = CoreExpr::DropSpecialized {
            scrutinee: crate::core::CoreVarRef::resolved(xs),
            unique_body: Box::new(CoreExpr::Case {
                scrutinee: Box::new(v(keep)),
                alts: vec![
                    CoreAlt {
                        pat: CorePat::Lit(CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::Reuse {
                            token: crate::core::CoreVarRef::resolved(xs),
                            tag: node.clone(),
                            fields: vec![v(color), v(left), v(key), v(right)],
                            field_mask: Some(0),
                            span: s(),
                        },
                        span: s(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Reuse {
                            token: crate::core::CoreVarRef::resolved(xs),
                            tag: node,
                            fields: vec![v(color), v(left), v(key), v(left)],
                            field_mask: Some(0b1000),
                            span: s(),
                        },
                        span: s(),
                    },
                ],
                span: s(),
            }),
            shared_body: Box::new(CoreExpr::Case {
                scrutinee: Box::new(v(keep)),
                alts: vec![
                    CoreAlt {
                        pat: CorePat::Lit(CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::Con {
                            tag: CoreTag::Named(interner.intern("Node")),
                            fields: vec![v(color), v(left), v(key), v(right)],
                            span: s(),
                        },
                        span: s(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Con {
                            tag: CoreTag::Named(interner.intern("Node")),
                            fields: vec![v(color), v(left), v(key), v(left)],
                            span: s(),
                        },
                        span: s(),
                    },
                ],
                span: s(),
            }),
            span: s(),
        };
        assert!(verify_contract(&branchy_named).is_ok());
    }
}
