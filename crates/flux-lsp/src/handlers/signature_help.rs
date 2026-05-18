use flux::ast::type_infer::display_infer_type;
use flux::syntax::expression::Expression;
use flux::types::infer_type::InferType;
use lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
};

use crate::locator::find_enclosing_call;
use crate::snapshot::Snapshot;

pub fn signature_help(snapshot: &Snapshot, position: Position) -> Option<SignatureHelp> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let infer = snapshot.infer.as_ref()?;

    let (call_expr, active_param) =
        find_enclosing_call(&snapshot.program, target)?;

    let Expression::Call { function, .. } = call_expr else {
        return None;
    };

    let fn_ty = infer.expr_types.get(&function.expr_id())?;
    let InferType::Fun(param_types, ret_type, _) = fn_ty else {
        return None;
    };

    let params_str: Vec<String> = param_types
        .iter()
        .map(|ty| display_infer_type(ty, &snapshot.interner))
        .collect();
    let ret_str = display_infer_type(ret_type, &snapshot.interner);
    let label = format!("({}) -> {}", params_str.join(", "), ret_str);

    let parameters: Vec<ParameterInformation> = params_str
        .iter()
        .map(|s| ParameterInformation {
            label: ParameterLabel::Simple(s.clone()),
            documentation: None,
        })
        .collect();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active_param as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param as u32),
    })
}
