use std::fmt;

use crate::runtime::{BaseFn, base::BaseHmSignatureId};

#[derive(Clone)]
pub struct BaseFunction {
    pub name: &'static str,
    pub func: BaseFn,
    pub hm_signature: BaseHmSignatureId,
}

impl fmt::Debug for BaseFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BaseFunction({})", self.name)
    }
}

impl PartialEq for BaseFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
