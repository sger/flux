use std::collections::HashMap;

use crate::{
    core::{CoreProgram, CoreTag, CoreTopLevelItem},
    llvm::{
        CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmFunction, LlvmFunctionSig, LlvmInstr,
        LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator, LlvmType, LlvmTypeDef,
    },
    syntax::{Identifier, interner::Interner},
};

use super::{
    CoreToLlvmError,
    closure::{const_i32_operand, flux_closure_symbol, local as local_operand},
    display_ident,
    prelude::{has_function, helper_attrs},
};

pub const FLUX_ADT_TYPE_NAME: &str = "FluxAdt";
pub const FLUX_TUPLE_TYPE_NAME: &str = "FluxTuple";
// FluxAdt = {i32 tag, i32 field_count, [0 x i64]} → 8 bytes before payload (no padding needed).
// FluxTuple = {i32 arity, [0 x i64]} → 4 bytes data, but LLVM pads to 8 for i64 alignment.

const SOME_TAG_ID: i32 = 1;
const LEFT_TAG_ID: i32 = 2;
const RIGHT_TAG_ID: i32 = 3;
const CONS_TAG_ID: i32 = 4;
const FIRST_USER_TAG_ID: i32 = 5;

// C runtime obj_tag values (must match flux_rt.h)
pub const FLUX_OBJ_ADT: i32 = 0xF2;
pub const FLUX_OBJ_TUPLE: i32 = 0xF3;

pub const FLUX_TUPLE_ARITY_FIELD: i32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdtMetadata {
    user_constructor_tags: HashMap<Identifier, i32>,
    constructor_arities: HashMap<Identifier, usize>,
}

impl AdtMetadata {
    pub fn collect(
        program: &CoreProgram,
        interner: Option<&Interner>,
    ) -> Result<Self, CoreToLlvmError> {
        let mut metadata =
            Self { user_constructor_tags: HashMap::new(), constructor_arities: HashMap::new() };
        let mut next_tag = FIRST_USER_TAG_ID;
        collect_items(&program.top_level_items, &mut metadata, &mut next_tag, interner)?;
        Ok(metadata)
    }

    pub fn builtin_tag(tag: &CoreTag) -> Option<i32> {
        match tag {
            CoreTag::Some => Some(SOME_TAG_ID),
            CoreTag::Left => Some(LEFT_TAG_ID),
            CoreTag::Right => Some(RIGHT_TAG_ID),
            CoreTag::Cons => Some(CONS_TAG_ID),
            CoreTag::Named(_) | CoreTag::None | CoreTag::Nil => None,
        }
    }

    pub fn tag_for(&self, tag: &CoreTag) -> Option<i32> {
        Self::builtin_tag(tag).or_else(|| match tag {
            CoreTag::Named(name) => self.user_constructor_tags.get(name).copied(),
            _ => None,
        })
    }

    pub fn arity_for(&self, tag: &CoreTag) -> Option<usize> {
        match tag {
            CoreTag::None | CoreTag::Nil => Some(0),
            CoreTag::Some | CoreTag::Left | CoreTag::Right => Some(1),
            CoreTag::Cons => Some(2),
            CoreTag::Named(name) => self.constructor_arities.get(name).copied(),
        }
    }

    pub fn user_tag_for_constructor(&self, ctor: Identifier) -> Option<i32> {
        self.user_constructor_tags.get(&ctor).copied()
    }

    pub fn arity_for_constructor(&self, ctor: Identifier) -> Option<usize> {
        self.constructor_arities.get(&ctor).copied()
    }
}

pub fn flux_adt_symbol(name: &str) -> GlobalId {
    GlobalId(name.to_string())
}

/// Emit ADT/tuple type definitions and helper functions into the module.
/// `_metadata` is reserved for future use (e.g., specialized constructors for known arities).
pub fn emit_adt_support(module: &mut LlvmModule, _metadata: &AdtMetadata) {
    emit_adt_type(module);
    emit_tuple_type(module);
    emit_tuple_len(module);
}

pub fn tuple_type() -> LlvmType {
    LlvmType::Named(FLUX_TUPLE_TYPE_NAME.into())
}

fn collect_items(
    items: &[CoreTopLevelItem],
    metadata: &mut AdtMetadata,
    next_tag: &mut i32,
    interner: Option<&Interner>,
) -> Result<(), CoreToLlvmError> {
    for item in items {
        match item {
            CoreTopLevelItem::Data { variants, .. } => {
                for variant in variants {
                    if let Some(existing) = metadata.constructor_arities.get(&variant.name)
                        && *existing != variant.fields.len()
                    {
                        return Err(CoreToLlvmError::Malformed {
                            message: format!(
                                "constructor `{}` was declared with multiple arities",
                                display_ident(variant.name, interner)
                            ),
                        });
                    }
                    metadata.constructor_arities.insert(variant.name, variant.fields.len());
                    if metadata.user_constructor_tags.contains_key(&variant.name) {
                        continue;
                    }
                    metadata.user_constructor_tags.insert(variant.name, *next_tag);
                    *next_tag += 1;
                }
            }
            CoreTopLevelItem::Module { body, .. } => {
                collect_items(body, metadata, next_tag, interner)?;
            }
            CoreTopLevelItem::Function { .. }
            | CoreTopLevelItem::Let { .. }
            | CoreTopLevelItem::Import { .. }
            | CoreTopLevelItem::EffectDecl { .. }
            | CoreTopLevelItem::Class { .. }
            | CoreTopLevelItem::Instance { .. } => {}
        }
    }
    Ok(())
}

fn emit_adt_type(module: &mut LlvmModule) {
    if module.type_defs.iter().any(|def| def.name == FLUX_ADT_TYPE_NAME) {
        return;
    }
    module.type_defs.push(LlvmTypeDef {
        name: FLUX_ADT_TYPE_NAME.into(),
        ty: LlvmType::Struct {
            packed: false,
            fields: vec![
                LlvmType::i32(),
                LlvmType::i32(),
                LlvmType::Array { len: 0, element: Box::new(LlvmType::i64()) },
            ],
        },
    });
}

fn emit_tuple_type(module: &mut LlvmModule) {
    if module.type_defs.iter().any(|def| def.name == FLUX_TUPLE_TYPE_NAME) {
        return;
    }
    module.type_defs.push(LlvmTypeDef {
        name: FLUX_TUPLE_TYPE_NAME.into(),
        ty: LlvmType::Struct {
            packed: false,
            fields: vec![
                LlvmType::i8(),  // obj_tag (FLUX_OBJ_TUPLE = 0xF3)
                LlvmType::i8(),  // _pad[0]
                LlvmType::i8(),  // _pad[1]
                LlvmType::i8(),  // _pad[2]
                LlvmType::i32(), // arity
                LlvmType::Array { len: 0, element: Box::new(LlvmType::i64()) },
            ],
        },
    });
}

fn emit_tuple_len(module: &mut LlvmModule) {
    let name = "flux_tuple_len";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_adt_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i32(),
            params: vec![LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("value".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                untag_boxed_ptr_call("ptr", local_operand("value")),
                gep_struct_field(
                    "arity.ptr",
                    tuple_type(),
                    local_operand("ptr"),
                    FLUX_TUPLE_ARITY_FIELD,
                ),
                LlvmInstr::Load {
                    dst: LlvmLocal("arity".into()),
                    ty: LlvmType::i32(),
                    ptr: local_operand("arity.ptr"),
                    align: Some(4),
                },
            ],
            term: LlvmTerminator::Ret { ty: LlvmType::i32(), value: local_operand("arity") },
        }],
    });
}

fn gep_struct_field(dst: &str, ty: LlvmType, base: LlvmOperand, field: i32) -> LlvmInstr {
    LlvmInstr::GetElementPtr {
        dst: LlvmLocal(dst.into()),
        inbounds: true,
        element_ty: ty,
        base,
        indices: vec![
            (LlvmType::i32(), const_i32_operand(0)),
            (LlvmType::i32(), const_i32_operand(field)),
        ],
    }
}

fn untag_boxed_ptr_call(dst: &str, value: LlvmOperand) -> LlvmInstr {
    LlvmInstr::Call {
        dst: Some(LlvmLocal(dst.into())),
        tail: false,
        call_conv: Some(CallConv::Fastcc),
        ret_ty: LlvmType::ptr(),
        callee: LlvmOperand::Global(flux_closure_symbol("flux_untag_boxed_ptr")),
        args: vec![(LlvmType::i64(), value)],
        attrs: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::CoreTopLevelItem,
        diagnostics::position::Span,
        syntax::{data_variant::DataVariant, interner::Interner},
    };

    #[test]
    fn builtin_tag_ids_are_stable() {
        assert_eq!(AdtMetadata::builtin_tag(&CoreTag::Some), Some(1));
        assert_eq!(AdtMetadata::builtin_tag(&CoreTag::Left), Some(2));
        assert_eq!(AdtMetadata::builtin_tag(&CoreTag::Right), Some(3));
        assert_eq!(AdtMetadata::builtin_tag(&CoreTag::Cons), Some(4));
        assert_eq!(AdtMetadata::builtin_tag(&CoreTag::None), None);
    }

    #[test]
    fn user_constructor_tags_follow_source_order() {
        let mut interner = Interner::new();
        let option = interner.intern("OptionI");
        let none_i = interner.intern("NoneI");
        let some_i = interner.intern("SomeI");
        let pair = interner.intern("Pair");
        let pair_ctor = interner.intern("PairCtor");
        let program = CoreProgram {
            defs: vec![],
            top_level_items: vec![
                CoreTopLevelItem::Data {
                    name: option,
                    type_params: vec![],
                    variants: vec![
                        DataVariant {
                            name: none_i,
                            fields: vec![],
                            field_names: None,
                            span: Span::default(),
                        },
                        DataVariant {
                            name: some_i,
                            fields: vec![],
                            field_names: None,
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                },
                CoreTopLevelItem::Data {
                    name: pair,
                    type_params: vec![],
                    variants: vec![DataVariant {
                        name: pair_ctor,
                        fields: vec![],
                        field_names: None,
                        span: Span::default(),
                    }],
                    span: Span::default(),
                },
            ],
        };

        let metadata = AdtMetadata::collect(&program, Some(&interner)).expect("collect");
        assert_eq!(metadata.user_tag_for_constructor(none_i), Some(5));
        assert_eq!(metadata.user_tag_for_constructor(some_i), Some(6));
        assert_eq!(metadata.user_tag_for_constructor(pair_ctor), Some(7));
        assert_eq!(metadata.arity_for_constructor(some_i), Some(0));
    }
}
