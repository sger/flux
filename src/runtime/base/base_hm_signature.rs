use std::collections::HashMap;

use crate::{
    runtime::base::helpers::{lower_effect_row, lower_type},
    syntax::interner::Interner,
    types::{TypeVarId, infer_type::InferType, scheme::Scheme},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseHmSignature {
    pub type_params: Vec<&'static str>,
    pub row_params: Vec<&'static str>,
    pub params: Vec<BaseHmType>,
    pub ret: BaseHmType,
    pub effects: BaseHmEffectRow,
}

impl BaseHmSignature {
    pub fn to_scheme(&self, interner: &mut Interner) -> Result<Scheme, String> {
        let mut next_var: TypeVarId = 0;
        let mut type_params: HashMap<&'static str, TypeVarId> = HashMap::new();
        let mut row_params: HashMap<&'static str, TypeVarId> = HashMap::new();

        for &name in &self.type_params {
            type_params.insert(name, next_var);
            next_var = next_var.saturating_add(1);
        }
        for &name in &self.row_params {
            row_params.insert(name, next_var);
            next_var = next_var.saturating_add(1);
        }

        let params = self
            .params
            .iter()
            .map(|param| {
                lower_type(
                    param,
                    &mut type_params,
                    &mut row_params,
                    &mut next_var,
                    interner,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let ret = lower_type(
            &self.ret,
            &mut type_params,
            &mut row_params,
            &mut next_var,
            interner,
        )?;

        let effects = lower_effect_row(&self.effects, &mut row_params, &mut next_var, interner)?;
        let infer_type = InferType::Fun(params, Box::new(ret), effects);
        let mut forall: Vec<TypeVarId> = infer_type.free_vars().into_iter().collect();
        forall.sort_unstable();
        forall.dedup();

        Ok(Scheme { forall, infer_type })
    }
}
