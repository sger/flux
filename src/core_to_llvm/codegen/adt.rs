use std::collections::HashMap;

use crate::{
    core::{CoreProgram, CoreTag, CoreTopLevelItem},
    core_to_llvm::{
        CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmConst, LlvmFunction, LlvmFunctionSig,
        LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator, LlvmType, LlvmTypeDef,
        LlvmValueKind,
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
const FLUX_ADT_HEADER_SIZE: i32 = 8;
const FLUX_TUPLE_HEADER_SIZE: i32 = 8;

const SOME_TAG_ID: i32 = 1;
const LEFT_TAG_ID: i32 = 2;
const RIGHT_TAG_ID: i32 = 3;
const CONS_TAG_ID: i32 = 4;
const FIRST_USER_TAG_ID: i32 = 5;

// C runtime obj_tag values (must match flux_rt.h)
const FLUX_OBJ_ADT: i32 = 0xF2;
const FLUX_OBJ_TUPLE: i32 = 0xF3;

pub const FLUX_ADT_TAG_FIELD: i32 = 0;
pub const FLUX_ADT_FIELD_COUNT_FIELD: i32 = 1;
pub const FLUX_ADT_PAYLOAD_FIELD: i32 = 2;

pub const FLUX_TUPLE_OBJ_TAG_FIELD: i32 = 0;
pub const FLUX_TUPLE_ARITY_FIELD: i32 = 4;
pub const FLUX_TUPLE_PAYLOAD_FIELD: i32 = 5;

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
        let mut metadata = Self {
            user_constructor_tags: HashMap::new(),
            constructor_arities: HashMap::new(),
        };
        let mut next_tag = FIRST_USER_TAG_ID;
        collect_items(
            &program.top_level_items,
            &mut metadata,
            &mut next_tag,
            interner,
        )?;
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
    emit_make_adt(module);
    emit_make_cons(module);
    emit_make_tuple(module);
    emit_adt_tag(module);
    emit_adt_field_ptr(module);
    emit_tuple_len(module);
    emit_tuple_field_ptr(module);
}

pub fn adt_type() -> LlvmType {
    LlvmType::Named(FLUX_ADT_TYPE_NAME.into())
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
                    metadata
                        .constructor_arities
                        .insert(variant.name, variant.fields.len());
                    if metadata.user_constructor_tags.contains_key(&variant.name) {
                        continue;
                    }
                    metadata
                        .user_constructor_tags
                        .insert(variant.name, *next_tag);
                    *next_tag += 1;
                }
            }
            CoreTopLevelItem::Module { body, .. } => {
                collect_items(body, metadata, next_tag, interner)?;
            }
            CoreTopLevelItem::Function { .. }
            | CoreTopLevelItem::Import { .. }
            | CoreTopLevelItem::EffectDecl { .. } => {}
        }
    }
    Ok(())
}

fn emit_adt_type(module: &mut LlvmModule) {
    if module
        .type_defs
        .iter()
        .any(|def| def.name == FLUX_ADT_TYPE_NAME)
    {
        return;
    }
    module.type_defs.push(LlvmTypeDef {
        name: FLUX_ADT_TYPE_NAME.into(),
        ty: LlvmType::Struct {
            packed: false,
            fields: vec![
                LlvmType::i32(),
                LlvmType::i32(),
                LlvmType::Array {
                    len: 0,
                    element: Box::new(LlvmType::i64()),
                },
            ],
        },
    });
}

fn emit_tuple_type(module: &mut LlvmModule) {
    if module
        .type_defs
        .iter()
        .any(|def| def.name == FLUX_TUPLE_TYPE_NAME)
    {
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
                LlvmType::Array {
                    len: 0,
                    element: Box::new(LlvmType::i64()),
                },
            ],
        },
    });
}

fn emit_make_adt(module: &mut LlvmModule) {
    let name = "flux_make_adt";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_adt_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::ptr(), LlvmType::i32(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![
            LlvmLocal("fields".into()),
            LlvmLocal("field_count".into()),
            LlvmLocal("ctor_tag".into()),
        ],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                byte_size_instr("payload.bytes", local_operand("field_count")),
                LlvmInstr::Binary {
                    dst: LlvmLocal("alloc.bytes".into()),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i32(),
                    lhs: local_operand("payload.bytes"),
                    rhs: const_i32_operand(FLUX_ADT_HEADER_SIZE),
                },
                LlvmInstr::Call {
                    dst: Some(LlvmLocal("mem".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::ptr(),
                    callee: LlvmOperand::Global(GlobalId("flux_bump_alloc_inline".into())),
                    args: vec![
                        (LlvmType::i32(), local_operand("alloc.bytes")),
                        (LlvmType::i32(), local_operand("field_count")), // scan_fsize
                        (LlvmType::i32(), const_i32_operand(FLUX_OBJ_ADT)),
                    ],
                    attrs: vec![],
                },
                gep_struct_field(
                    "tag.ptr",
                    adt_type(),
                    local_operand("mem"),
                    FLUX_ADT_TAG_FIELD,
                ),
                LlvmInstr::Store {
                    ty: LlvmType::i32(),
                    value: local_operand("ctor_tag"),
                    ptr: local_operand("tag.ptr"),
                    align: Some(4),
                },
                gep_struct_field(
                    "field_count.ptr",
                    adt_type(),
                    local_operand("mem"),
                    FLUX_ADT_FIELD_COUNT_FIELD,
                ),
                LlvmInstr::Store {
                    ty: LlvmType::i32(),
                    value: local_operand("field_count"),
                    ptr: local_operand("field_count.ptr"),
                    align: Some(4),
                },
                gep_payload(
                    "payload.ptr",
                    adt_type(),
                    local_operand("mem"),
                    FLUX_ADT_PAYLOAD_FIELD,
                ),
                LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_copy_i64s")),
                    args: vec![
                        (LlvmType::ptr(), local_operand("payload.ptr")),
                        (LlvmType::ptr(), local_operand("fields")),
                        (LlvmType::i32(), local_operand("field_count")),
                    ],
                    attrs: vec![],
                },
                LlvmInstr::Call {
                    dst: Some(LlvmLocal("boxed".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_tag_boxed_ptr")),
                    args: vec![(LlvmType::ptr(), local_operand("mem"))],
                    attrs: vec![],
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local_operand("boxed"),
            },
        }],
    });
}

fn emit_make_cons(module: &mut LlvmModule) {
    let name = "flux_make_cons";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_adt_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::i64(), LlvmType::i64()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("head".into()), LlvmLocal("tail".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                LlvmInstr::Alloca {
                    dst: LlvmLocal("fields".into()),
                    ty: LlvmType::i64(),
                    count: Some((LlvmType::i32(), const_i32_operand(2))),
                    align: Some(8),
                },
                gep_i64("head.ptr", local_operand("fields"), const_i32_operand(0)),
                LlvmInstr::Store {
                    ty: LlvmType::i64(),
                    value: local_operand("head"),
                    ptr: local_operand("head.ptr"),
                    align: Some(8),
                },
                gep_i64("tail.ptr", local_operand("fields"), const_i32_operand(1)),
                LlvmInstr::Store {
                    ty: LlvmType::i64(),
                    value: local_operand("tail"),
                    ptr: local_operand("tail.ptr"),
                    align: Some(8),
                },
                LlvmInstr::Call {
                    dst: Some(LlvmLocal("boxed".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_adt_symbol("flux_make_adt")),
                    args: vec![
                        (LlvmType::ptr(), local_operand("fields")),
                        (LlvmType::i32(), const_i32_operand(2)),
                        (LlvmType::i32(), const_i32_operand(CONS_TAG_ID)),
                    ],
                    attrs: vec![],
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local_operand("boxed"),
            },
        }],
    });
}

fn emit_make_tuple(module: &mut LlvmModule) {
    let name = "flux_make_tuple";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_adt_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::ptr(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("fields".into()), LlvmLocal("arity".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                byte_size_instr("payload.bytes", local_operand("arity")),
                LlvmInstr::Binary {
                    dst: LlvmLocal("alloc.bytes".into()),
                    op: LlvmValueKind::Add,
                    ty: LlvmType::i32(),
                    lhs: local_operand("payload.bytes"),
                    rhs: const_i32_operand(FLUX_TUPLE_HEADER_SIZE),
                },
                LlvmInstr::Call {
                    dst: Some(LlvmLocal("mem".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::ptr(),
                    callee: LlvmOperand::Global(GlobalId("flux_bump_alloc_inline".into())),
                    args: vec![
                        (LlvmType::i32(), local_operand("alloc.bytes")),
                        (LlvmType::i32(), local_operand("arity")), // scan_fsize
                        (LlvmType::i32(), const_i32_operand(FLUX_OBJ_TUPLE)),
                    ],
                    attrs: vec![],
                },
                // Store obj_tag = FLUX_OBJ_TUPLE (0xF3)
                gep_struct_field(
                    "tag.ptr",
                    tuple_type(),
                    local_operand("mem"),
                    FLUX_TUPLE_OBJ_TAG_FIELD,
                ),
                LlvmInstr::Store {
                    ty: LlvmType::i8(),
                    value: LlvmOperand::Const(LlvmConst::Int {
                        bits: 8,
                        value: 0xF3,
                    }),
                    ptr: local_operand("tag.ptr"),
                    align: Some(1),
                },
                gep_struct_field(
                    "arity.ptr",
                    tuple_type(),
                    local_operand("mem"),
                    FLUX_TUPLE_ARITY_FIELD,
                ),
                LlvmInstr::Store {
                    ty: LlvmType::i32(),
                    value: local_operand("arity"),
                    ptr: local_operand("arity.ptr"),
                    align: Some(4),
                },
                gep_payload(
                    "payload.ptr",
                    tuple_type(),
                    local_operand("mem"),
                    FLUX_TUPLE_PAYLOAD_FIELD,
                ),
                LlvmInstr::Call {
                    dst: None,
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::Void,
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_copy_i64s")),
                    args: vec![
                        (LlvmType::ptr(), local_operand("payload.ptr")),
                        (LlvmType::ptr(), local_operand("fields")),
                        (LlvmType::i32(), local_operand("arity")),
                    ],
                    attrs: vec![],
                },
                LlvmInstr::Call {
                    dst: Some(LlvmLocal("boxed".into())),
                    tail: false,
                    call_conv: Some(CallConv::Fastcc),
                    ret_ty: LlvmType::i64(),
                    callee: LlvmOperand::Global(flux_closure_symbol("flux_tag_boxed_ptr")),
                    args: vec![(LlvmType::ptr(), local_operand("mem"))],
                    attrs: vec![],
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: local_operand("boxed"),
            },
        }],
    });
}

fn emit_adt_tag(module: &mut LlvmModule) {
    let name = "flux_adt_tag";
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
                    "tag.ptr",
                    adt_type(),
                    local_operand("ptr"),
                    FLUX_ADT_TAG_FIELD,
                ),
                LlvmInstr::Load {
                    dst: LlvmLocal("tag".into()),
                    ty: LlvmType::i32(),
                    ptr: local_operand("tag.ptr"),
                    align: Some(4),
                },
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::i32(),
                value: local_operand("tag"),
            },
        }],
    });
}

fn emit_adt_field_ptr(module: &mut LlvmModule) {
    let name = "flux_adt_field_ptr";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_adt_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::ptr(),
            params: vec![LlvmType::i64(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("value".into()), LlvmLocal("index".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                untag_boxed_ptr_call("ptr", local_operand("value")),
                gep_payload(
                    "payload.ptr",
                    adt_type(),
                    local_operand("ptr"),
                    FLUX_ADT_PAYLOAD_FIELD,
                ),
                gep_i64(
                    "field.ptr",
                    local_operand("payload.ptr"),
                    local_operand("index"),
                ),
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::ptr(),
                value: local_operand("field.ptr"),
            },
        }],
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
            term: LlvmTerminator::Ret {
                ty: LlvmType::i32(),
                value: local_operand("arity"),
            },
        }],
    });
}

fn emit_tuple_field_ptr(module: &mut LlvmModule) {
    let name = "flux_tuple_field_ptr";
    if has_function(module, name) {
        return;
    }
    module.functions.push(LlvmFunction {
        linkage: Linkage::Internal,
        name: flux_adt_symbol(name),
        sig: LlvmFunctionSig {
            ret: LlvmType::ptr(),
            params: vec![LlvmType::i64(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![LlvmLocal("value".into()), LlvmLocal("index".into())],
        attrs: helper_attrs(),
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs: vec![
                untag_boxed_ptr_call("ptr", local_operand("value")),
                gep_payload(
                    "payload.ptr",
                    tuple_type(),
                    local_operand("ptr"),
                    FLUX_TUPLE_PAYLOAD_FIELD,
                ),
                gep_i64(
                    "field.ptr",
                    local_operand("payload.ptr"),
                    local_operand("index"),
                ),
            ],
            term: LlvmTerminator::Ret {
                ty: LlvmType::ptr(),
                value: local_operand("field.ptr"),
            },
        }],
    });
}

fn byte_size_instr(dst: &str, count: LlvmOperand) -> LlvmInstr {
    LlvmInstr::Binary {
        dst: LlvmLocal(dst.into()),
        op: LlvmValueKind::Mul,
        ty: LlvmType::i32(),
        lhs: count,
        rhs: const_i32_operand(8),
    }
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

fn gep_payload(dst: &str, ty: LlvmType, base: LlvmOperand, field: i32) -> LlvmInstr {
    LlvmInstr::GetElementPtr {
        dst: LlvmLocal(dst.into()),
        inbounds: true,
        element_ty: ty,
        base,
        indices: vec![
            (LlvmType::i32(), const_i32_operand(0)),
            (LlvmType::i32(), const_i32_operand(field)),
            (LlvmType::i32(), const_i32_operand(0)),
        ],
    }
}

fn gep_i64(dst: &str, base: LlvmOperand, index: LlvmOperand) -> LlvmInstr {
    LlvmInstr::GetElementPtr {
        dst: LlvmLocal(dst.into()),
        inbounds: true,
        element_ty: LlvmType::i64(),
        base,
        indices: vec![(LlvmType::i32(), index)],
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
                            span: Span::default(),
                        },
                        DataVariant {
                            name: some_i,
                            fields: vec![],
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
