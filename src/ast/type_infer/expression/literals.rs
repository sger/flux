// use super::*;

// impl<'a> InferCtx<'a> {
//     /// Infer literal-like expressions and identifiers.
//     ///
//     /// Returns `None` when the expression is not handled by this family.
//     pub(super) fn infer_literal_expression(&mut self, expr: &Expression) -> Option<InferType> {
//         let inferred = match expr {
//             Expression::Integer { .. } => InferType::Con(TypeConstructor::Int),
//             Expression::Float { .. } => InferType::Con(TypeConstructor::Float),
//             Expression::Boolean { .. } => InferType::Con(TypeConstructor::Bool),
//             Expression::String { .. } | Expression::InterpolatedString { .. } => {
//                 InferType::Con(TypeConstructor::String)
//             }
//             Expression::None { .. } => {
//                 InferType::App(TypeConstructor::Option, vec![self.env.alloc_infer_type_var()])
//             }
//             Expression::Some { value, .. } => {
//                 let inner = self.infer_expression(value);
//                 InferType::App(TypeConstructor::Option, vec![inner])
//             }
//             Expression::Left { value, .. } => {
//                 let inner = self.infer_expression(value);
//                 let right = self.env.alloc_infer_type_var();
//                 InferType::App(TypeConstructor::Either, vec![inner, right])
//             }
//             Expression::Right { value, .. } => {
//                 let inner = self.infer_expression(value);
//                 let left = self.env.alloc_infer_type_var();
//                 InferType::App(TypeConstructor::Either, vec![left, inner])
//             }
//             Expression::Identifier { name, .. } => {
//                 if let Some(scheme) = self.env.lookup(*name).cloned() {
//                     let (ty, _) = scheme.instantiate(&mut self.env.counter);
//                     ty
//                 } else {
//                     if self.known_base_names.contains(name) {
//                         self.emit_missing_base_hm_signature(*name, expr.span());
//                     }
//                     InferType::Con(TypeConstructor::Any)
//                 }
//             }
//             _ => return None,
//         };
//         Some(inferred)
//     }
// }
