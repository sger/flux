use crate::core::{CoreBinderId, CoreVarRef};
use crate::syntax::Identifier;

use super::borrow_infer::{BorrowCallee, BorrowProvenance};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AetherCalleeKind {
    DirectLocal,
    DirectInferredGlobal,
    BaseRuntime,
    Imported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AetherCalleeClassification {
    pub kind: AetherCalleeKind,
    pub borrow_callee: BorrowCallee,
    pub binder: Option<CoreBinderId>,
    pub name: Option<Identifier>,
    pub provenance: BorrowProvenance,
}

pub fn classify_direct_var_ref<B, N>(
    var: &CoreVarRef,
    binder_known: B,
    name_provenance: N,
) -> AetherCalleeClassification
where
    B: Fn(CoreBinderId) -> bool,
    N: Fn(Identifier) -> Option<BorrowProvenance>,
{
    if let Some(binder) = var.binder
        && binder_known(binder)
    {
        return AetherCalleeClassification {
            kind: AetherCalleeKind::DirectLocal,
            borrow_callee: BorrowCallee::Local(binder),
            binder: Some(binder),
            name: Some(var.name),
            provenance: BorrowProvenance::Inferred,
        };
    }

    match name_provenance(var.name) {
        Some(BorrowProvenance::Inferred) => AetherCalleeClassification {
            kind: AetherCalleeKind::DirectInferredGlobal,
            borrow_callee: BorrowCallee::Global(var.name),
            binder: None,
            name: Some(var.name),
            provenance: BorrowProvenance::Inferred,
        },
        Some(BorrowProvenance::BaseRuntime) => AetherCalleeClassification {
            kind: AetherCalleeKind::BaseRuntime,
            borrow_callee: BorrowCallee::BaseRuntime(var.name),
            binder: None,
            name: Some(var.name),
            provenance: BorrowProvenance::BaseRuntime,
        },
        Some(BorrowProvenance::Imported) => AetherCalleeClassification {
            kind: AetherCalleeKind::Imported,
            borrow_callee: BorrowCallee::Imported(var.name),
            binder: None,
            name: Some(var.name),
            provenance: BorrowProvenance::Imported,
        },
        Some(BorrowProvenance::Unknown) | None => AetherCalleeClassification {
            kind: AetherCalleeKind::Unknown,
            borrow_callee: BorrowCallee::Unknown,
            binder: var.binder,
            name: Some(var.name),
            provenance: BorrowProvenance::Unknown,
        },
    }
}
