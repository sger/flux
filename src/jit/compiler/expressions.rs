use super::*;

pub(super) fn compile_simple_backend_ir_expr(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    env: &HashMap<BackendIrVar, JitValue>,
    module_env: &HashMap<BackendIrVar, Identifier>,
    scope: &Scope,
    backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
    _backend_function_defs: &HashMap<BackendFunctionId, &BackendIrFunction>,
    interner: &Interner,
    expr: &BackendIrExpr,
) -> Result<JitValue, String> {
    match expr {
        BackendIrExpr::Const(IrConst::Int(value)) => {
            Ok(JitValue::int(builder.ins().iconst(types::I64, *value)))
        }
        BackendIrExpr::Const(IrConst::Float(value)) => Ok(JitValue::float(
            builder.ins().iconst(types::I64, value.to_bits() as i64),
        )),
        BackendIrExpr::Const(IrConst::Bool(value)) => Ok(JitValue::bool(
            builder.ins().iconst(types::I64, *value as i64),
        )),
        BackendIrExpr::Const(IrConst::String(value)) => {
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let bytes = value.as_bytes();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Const(IrConst::Unit) | BackendIrExpr::None => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[1]))
        }
        BackendIrExpr::InterpolatedString(parts) => {
            let rt_to_string = get_helper_func_ref(module, helpers, builder, "rt_to_string");
            let rt_string_concat =
                get_helper_func_ref(module, helpers, builder, "rt_string_concat");
            let mut acc: Option<CraneliftValue> = None;
            for part in parts {
                let part_val = match part {
                    crate::cfg::IrStringPart::Literal(s) => {
                        let bytes = s.as_bytes();
                        let data = module
                            .declare_anonymous_data(false, false)
                            .map_err(|e| e.to_string())?;
                        let mut desc = DataDescription::new();
                        desc.define(bytes.to_vec().into_boxed_slice());
                        module.define_data(data, &desc).map_err(|e| e.to_string())?;
                        let gv = module.declare_data_in_func(data, builder.func);
                        let ptr = builder.ins().global_value(PTR_TYPE, gv);
                        let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
                        let make_string =
                            get_helper_func_ref(module, helpers, builder, "rt_make_string");
                        let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
                        builder.inst_results(call)[0]
                    }
                    crate::cfg::IrStringPart::Interpolation(var) => {
                        let val = env.get(var).copied().ok_or_else(|| {
                            format!("missing backend IR interpolation var {:?}", var)
                        })?;
                        let val = box_jit_value(module, helpers, builder, ctx_val, val);
                        let call = builder.ins().call(rt_to_string, &[ctx_val, val]);
                        builder.inst_results(call)[0]
                    }
                };
                acc = Some(match acc {
                    None => part_val,
                    Some(prev) => {
                        let call = builder
                            .ins()
                            .call(rt_string_concat, &[ctx_val, prev, part_val]);
                        builder.inst_results(call)[0]
                    }
                });
            }
            match acc {
                Some(val) => Ok(JitValue::boxed(val)),
                None => {
                    let make_string =
                        get_helper_func_ref(module, helpers, builder, "rt_make_string");
                    let null = builder.ins().iconst(PTR_TYPE, 0);
                    let zero = builder.ins().iconst(PTR_TYPE, 0);
                    let call = builder.ins().call(make_string, &[ctx_val, null, zero]);
                    Ok(JitValue::boxed(builder.inst_results(call)[0]))
                }
            }
        }
        BackendIrExpr::Prefix { operator, right } => {
            let operand = env
                .get(right)
                .copied()
                .ok_or_else(|| format!("missing backend IR prefix var {:?}", right))?;
            let (tag, payload) = jit_value_to_tag_payload(builder, operand);
            let helper = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => return Err(format!("unsupported backend prefix operator: {}", operator)),
            };
            let func_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(func_ref, &[ctx_val, tag, payload]);
            let result = boxed_value_from_tagged_parts(
                module,
                helpers,
                builder,
                ctx_val,
                builder.inst_results(call)[0],
                builder.inst_results(call)[1],
            );
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::Var(var) => env
            .get(var)
            .copied()
            .ok_or_else(|| format!("missing backend IR var {:?}", var)),
        BackendIrExpr::MakeTuple(vars) => {
            let vals = vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR tuple var {:?}", var))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let elems_ptr = emit_tagged_stack_array(builder, &vals).1;
            let len_val = builder.ins().iconst(PTR_TYPE, vals.len() as i64);
            let make_tuple = get_helper_func_ref(module, helpers, builder, "rt_make_tuple");
            let call = builder
                .ins()
                .call(make_tuple, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeArray(vars) => {
            let vals = vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR array var {:?}", var))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let elems_ptr = emit_tagged_stack_array(builder, &vals).1;
            let len_val = builder.ins().iconst(PTR_TYPE, vals.len() as i64);
            let make_array = get_helper_func_ref(module, helpers, builder, "rt_make_array");
            let call = builder
                .ins()
                .call(make_array, &[ctx_val, elems_ptr, len_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeHash(pairs) => {
            let mut pair_vals = Vec::with_capacity(pairs.len() * 2);
            for (k, v) in pairs {
                pair_vals.push(
                    env.get(k)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR hash key var {:?}", k))?,
                );
                pair_vals.push(
                    env.get(v)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR hash value var {:?}", v))?,
                );
            }
            let pairs_ptr = emit_tagged_stack_array(builder, &pair_vals).1;
            let npairs_val = builder.ins().iconst(PTR_TYPE, pairs.len() as i64);
            let make_hash = get_helper_func_ref(module, helpers, builder, "rt_make_hash");
            let call = builder
                .ins()
                .call(make_hash, &[ctx_val, pairs_ptr, npairs_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeList(vars) => {
            let make_empty = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let empty_call = builder.ins().call(make_empty, &[ctx_val]);
            let mut acc = builder.inst_results(empty_call)[0];
            for var in vars.iter().rev() {
                let val = env
                    .get(var)
                    .copied()
                    .ok_or_else(|| format!("missing backend IR list var {:?}", var))?;
                let val = box_jit_value(module, helpers, builder, ctx_val, val);
                let cons_call = builder.ins().call(make_cons, &[ctx_val, val, acc]);
                acc = builder.inst_results(cons_call)[0];
            }
            Ok(JitValue::boxed(acc))
        }
        BackendIrExpr::MakeAdt(constructor, vars) => {
            let name_str = interner.resolve(*constructor);
            let bytes = name_str.as_bytes().to_vec();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let global_value = module.declare_data_in_func(data, builder.func);
            let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
            let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            let boxed_vals = vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR adt var {:?}", var))
                        .map(|v| box_jit_value(module, helpers, builder, ctx_val, v))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let call = match boxed_vals.len() {
                1 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt1");
                    builder
                        .ins()
                        .call(helper, &[ctx_val, name_ptr, name_len, boxed_vals[0]])
                }
                2 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt2");
                    builder.ins().call(
                        helper,
                        &[ctx_val, name_ptr, name_len, boxed_vals[0], boxed_vals[1]],
                    )
                }
                3 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt3");
                    builder.ins().call(
                        helper,
                        &[
                            ctx_val,
                            name_ptr,
                            name_len,
                            boxed_vals[0],
                            boxed_vals[1],
                            boxed_vals[2],
                        ],
                    )
                }
                4 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt4");
                    builder.ins().call(
                        helper,
                        &[
                            ctx_val,
                            name_ptr,
                            name_len,
                            boxed_vals[0],
                            boxed_vals[1],
                            boxed_vals[2],
                            boxed_vals[3],
                        ],
                    )
                }
                5 => {
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt5");
                    builder.ins().call(
                        helper,
                        &[
                            ctx_val,
                            name_ptr,
                            name_len,
                            boxed_vals[0],
                            boxed_vals[1],
                            boxed_vals[2],
                            boxed_vals[3],
                            boxed_vals[4],
                        ],
                    )
                }
                _ => {
                    let vals = vars
                        .iter()
                        .map(|var| {
                            env.get(var)
                                .copied()
                                .ok_or_else(|| format!("missing backend IR adt var {:?}", var))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let fields_ptr = emit_tagged_stack_array(builder, &vals).1;
                    let arity_val = builder.ins().iconst(PTR_TYPE, vars.len() as i64);
                    let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
                    builder.ins().call(
                        helper,
                        &[ctx_val, name_ptr, name_len, fields_ptr, arity_val],
                    )
                }
            };
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MakeClosure(function_id, capture_vars) => {
            let meta = backend_function_metas
                .get(function_id)
                .copied()
                .ok_or_else(|| format!("missing backend closure metadata for {:?}", function_id))?;
            let capture_vals = capture_vars
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR capture var {:?}", var))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (_slot, captures_ptr) = emit_tagged_stack_array(builder, &capture_vals);
            let ncaptures = builder.ins().iconst(PTR_TYPE, capture_vals.len() as i64);
            let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
            let make_jit_closure =
                get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
            let call = builder.ins().call(
                make_jit_closure,
                &[ctx_val, fn_idx, captures_ptr, ncaptures],
            );
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Binary(op, lhs, rhs) => {
            let lhs = env
                .get(lhs)
                .copied()
                .ok_or_else(|| format!("missing backend IR lhs var {:?}", lhs))?;
            let rhs = env
                .get(rhs)
                .copied()
                .ok_or_else(|| format!("missing backend IR rhs var {:?}", rhs))?;
            compile_simple_backend_ir_binary(module, helpers, builder, ctx_val, *op, lhs, rhs)
        }
        BackendIrExpr::LoadName(name) => {
            if let Some(meta) = scope.functions.get(name).copied() {
                let make_jit_closure =
                    get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let call = builder
                    .ins()
                    .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&global_idx) = scope.globals.get(name) {
                let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, global_idx as i64);
                let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if let Some(&base_idx) = scope.base_functions.get(name) {
                let make_base =
                    get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else if scope.adt_constructors.get(name).copied() == Some(0) {
                compile_backend_named_adt_constructor_call(
                    module,
                    helpers,
                    builder,
                    ctx_val,
                    *name,
                    &[],
                    interner,
                )
            } else if resolve_module_name(scope, interner, *name).is_some() {
                let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                let call = builder.ins().call(make_none, &[ctx_val]);
                Ok(JitValue::boxed(builder.inst_results(call)[1]))
            } else {
                Err("backend JIT path does not yet support non-function LoadName".to_string())
            }
        }
        BackendIrExpr::EmptyList => {
            let make_empty = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
            let call = builder.ins().call(make_empty, &[ctx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Index { left, index } => {
            let left_val = env
                .get(left)
                .copied()
                .ok_or_else(|| format!("missing backend IR index left var {:?}", left))?;
            let index_val = env
                .get(index)
                .copied()
                .ok_or_else(|| format!("missing backend IR index right var {:?}", index))?;
            let rt_index = get_helper_func_ref(module, helpers, builder, "rt_index");
            let left_val = box_jit_value(module, helpers, builder, ctx_val, left_val);
            let index_val = box_jit_value(module, helpers, builder, ctx_val, index_val);
            let call = builder
                .ins()
                .call(rt_index, &[ctx_val, left_val, index_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::MemberAccess { object, member, .. } => {
            if let Some(module_name) = module_env.get(object).copied() {
                if let Some(meta) = scope.module_functions.get(&(module_name, *member)).copied() {
                    let make_jit_closure =
                        get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                    let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                    let zero = builder.ins().iconst(PTR_TYPE, 0);
                    let call = builder
                        .ins()
                        .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                    return Ok(JitValue::boxed(builder.inst_results(call)[0]));
                }
                if interner.resolve(module_name) == "Base"
                    && let Some(base_idx) =
                        crate::runtime::base::get_base_function_index(interner.resolve(*member))
                {
                    let make_base =
                        get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                    let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                    let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
                    return Ok(JitValue::boxed(builder.inst_results(call)[0]));
                }
                return Err(format!(
                    "unknown module member: {}.{}",
                    interner.resolve(module_name),
                    interner.resolve(*member)
                ));
            }
            let object_val = env
                .get(object)
                .copied()
                .ok_or_else(|| format!("missing backend IR member object var {:?}", object))?;
            let member_name = interner.resolve(*member);
            let bytes = member_name.as_bytes();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let member_call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            let member_val = builder.inst_results(member_call)[0];
            let rt_index = get_helper_func_ref(module, helpers, builder, "rt_index");
            let object_val = box_jit_value(module, helpers, builder, ctx_val, object_val);
            let index_call = builder
                .ins()
                .call(rt_index, &[ctx_val, object_val, member_val]);
            let indexed = builder.inst_results(index_call)[0];
            emit_return_on_null_value(builder, indexed);
            let unwrap_some = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let unwrap_call = builder.ins().call(unwrap_some, &[ctx_val, indexed]);
            let result = builder.inst_results(unwrap_call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::TupleFieldAccess { object, index } => {
            let tuple_val = env
                .get(object)
                .copied()
                .ok_or_else(|| format!("missing backend IR tuple object var {:?}", object))?;
            let index_val = builder.ins().iconst(PTR_TYPE, *index as i64);
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            let tuple_val = box_jit_value(module, helpers, builder, ctx_val, tuple_val);
            let call = builder
                .ins()
                .call(tuple_get, &[ctx_val, tuple_val, index_val]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::TupleArityTest { value, arity } => {
            let tuple_val = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR tuple-arity var {:?}", value))?;
            let tuple_val = box_jit_value(module, helpers, builder, ctx_val, tuple_val);
            let arity_val = builder.ins().iconst(PTR_TYPE, *arity as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_tuple_len_eq");
            let call = builder.ins().call(helper, &[ctx_val, tuple_val, arity_val]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::TagTest { value, tag } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR tag-test var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = match tag {
                crate::cfg::IrTagTest::None => "rt_is_none",
                crate::cfg::IrTagTest::Some => "rt_is_some",
                crate::cfg::IrTagTest::Left => "rt_is_left",
                crate::cfg::IrTagTest::Right => "rt_is_right",
            };
            let helper_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(helper_ref, &[ctx_val, boxed]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::TagPayload { value, tag } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR tag-payload var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = match tag {
                crate::cfg::IrTagTest::None => {
                    return Err("backend JIT path cannot extract payload from None".to_string());
                }
                crate::cfg::IrTagTest::Some => "rt_unwrap_some",
                crate::cfg::IrTagTest::Left => "rt_unwrap_left",
                crate::cfg::IrTagTest::Right => "rt_unwrap_right",
            };
            let helper_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(helper_ref, &[ctx_val, boxed]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::ListTest { value, tag } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR list-test var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = match tag {
                crate::cfg::IrListTest::Empty => "rt_is_empty_list",
                crate::cfg::IrListTest::Cons => "rt_is_cons",
            };
            let helper_ref = get_helper_func_ref(module, helpers, builder, helper);
            let call = builder.ins().call(helper_ref, &[ctx_val, boxed]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::ListHead { value } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR list-head var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let call = builder.ins().call(helper, &[ctx_val, boxed]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::ListTail { value } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR list-tail var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let call = builder.ins().call(helper, &[ctx_val, boxed]);
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::AdtTagTest { value, constructor } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR adt-test var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let name_str = interner.resolve(*constructor);
            let bytes = name_str.as_bytes().to_vec();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_is_adt_constructor");
            let call = builder.ins().call(helper, &[ctx_val, boxed, ptr, len]);
            Ok(JitValue::bool(builder.inst_results(call)[0]))
        }
        BackendIrExpr::AdtField { value, index } => {
            let value = env
                .get(value)
                .copied()
                .ok_or_else(|| format!("missing backend IR adt-field var {:?}", value))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, value);
            let idx_val = builder.ins().iconst(PTR_TYPE, *index as i64);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_adt_field_or_none");
            let call = builder.ins().call(helper, &[ctx_val, boxed, idx_val]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Some(var) => {
            let inner = env
                .get(var)
                .copied()
                .ok_or_else(|| format!("missing backend IR some var {:?}", var))?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_some");
            let call = builder.ins().call(helper, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Left(var) => {
            let inner = env
                .get(var)
                .copied()
                .ok_or_else(|| format!("missing backend IR left var {:?}", var))?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_left");
            let call = builder.ins().call(helper, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Right(var) => {
            let inner = env
                .get(var)
                .copied()
                .ok_or_else(|| format!("missing backend IR right var {:?}", var))?;
            let inner = box_jit_value(module, helpers, builder, ctx_val, inner);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_right");
            let call = builder.ins().call(helper, &[ctx_val, inner]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Cons { head, tail } => {
            let head = env
                .get(head)
                .copied()
                .ok_or_else(|| format!("missing backend IR cons head var {:?}", head))?;
            let tail = env
                .get(tail)
                .copied()
                .ok_or_else(|| format!("missing backend IR cons tail var {:?}", tail))?;
            let head = box_jit_value(module, helpers, builder, ctx_val, head);
            let tail = box_jit_value(module, helpers, builder, ctx_val, tail);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let call = builder.ins().call(helper, &[ctx_val, head, tail]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::Perform {
            effect,
            operation,
            args,
        } => {
            let arg_vals = args
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR perform arg {:?}", var))
                        .map(|v| box_and_guard_jit_value(module, helpers, builder, ctx_val, v))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                (arg_vals.len().max(1) as u32) * 8,
                3,
            ));
            for (i, val) in arg_vals.iter().enumerate() {
                builder.ins().stack_store(*val, slot, (i * 8) as i32);
            }
            let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
            let nargs_val = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
            let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
            let op_val = builder.ins().iconst(PTR_TYPE, operation.as_u32() as i64);
            let effect_name = interner.resolve(*effect);
            let op_name = interner.resolve(*operation);
            let effect_data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut effect_desc = DataDescription::new();
            effect_desc.define(effect_name.as_bytes().to_vec().into_boxed_slice());
            module
                .define_data(effect_data, &effect_desc)
                .map_err(|e| e.to_string())?;
            let op_data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut op_desc = DataDescription::new();
            op_desc.define(op_name.as_bytes().to_vec().into_boxed_slice());
            module
                .define_data(op_data, &op_desc)
                .map_err(|e| e.to_string())?;
            let effect_gv = module.declare_data_in_func(effect_data, builder.func);
            let effect_ptr = builder.ins().global_value(PTR_TYPE, effect_gv);
            let effect_len = builder.ins().iconst(PTR_TYPE, effect_name.len() as i64);
            let op_gv = module.declare_data_in_func(op_data, builder.func);
            let op_ptr = builder.ins().global_value(PTR_TYPE, op_gv);
            let op_len = builder.ins().iconst(PTR_TYPE, op_name.len() as i64);
            let zero = builder.ins().iconst(PTR_TYPE, 0);
            let rt_perform = get_helper_func_ref(module, helpers, builder, "rt_perform");
            let call = builder.ins().call(
                rt_perform,
                &[
                    ctx_val, effect_val, op_val, args_ptr, nargs_val, effect_ptr, effect_len,
                    op_ptr, op_len, zero, zero,
                ],
            );
            let result = builder.inst_results(call)[0];
            emit_return_on_null_value(builder, result);
            Ok(JitValue::boxed(result))
        }
        BackendIrExpr::DropReuse(var) => {
            if let Some(&val) = env.get(var) {
                let val = box_jit_value(module, helpers, builder, ctx_val, val);
                let helper = get_helper_func_ref(module, helpers, builder, "rt_drop_reuse");
                let call = builder.ins().call(helper, &[ctx_val, val]);
                Ok(JitValue::boxed(builder.inst_results(call)[0]))
            } else {
                // Token not in scope (e.g., scrutinee in a Case branch block).
                // Return null — reuse constructors will fall back to fresh allocation.
                Ok(JitValue::boxed(builder.ins().iconst(types::I64, 0)))
            }
        }
        BackendIrExpr::ReuseCons {
            token,
            head,
            tail,
            field_mask,
        } => {
            let token_boxed = if let Some(&token_val) = env.get(token) {
                box_jit_value(module, helpers, builder, ctx_val, token_val)
            } else {
                // Token not in scope — null token makes rt_reuse_cons allocate fresh
                builder.ins().iconst(types::I64, 0)
            };
            let head_val = env
                .get(head)
                .copied()
                .ok_or_else(|| format!("missing backend IR reuse_cons head {:?}", head))?;
            let tail_val = env
                .get(tail)
                .copied()
                .ok_or_else(|| format!("missing backend IR reuse_cons tail {:?}", tail))?;
            let head_boxed = box_jit_value(module, helpers, builder, ctx_val, head_val);
            let tail_boxed = box_jit_value(module, helpers, builder, ctx_val, tail_val);
            let call = if let Some(mask) = field_mask {
                let helper = get_helper_func_ref(module, helpers, builder, "rt_reuse_cons_masked");
                let mask_val = builder.ins().iconst(types::I64, *mask as i64);
                builder.ins().call(
                    helper,
                    &[ctx_val, token_boxed, head_boxed, tail_boxed, mask_val],
                )
            } else {
                let helper = get_helper_func_ref(module, helpers, builder, "rt_reuse_cons");
                builder
                    .ins()
                    .call(helper, &[ctx_val, token_boxed, head_boxed, tail_boxed])
            };
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::ReuseSome { token, inner } => {
            let token_boxed = if let Some(&token_val) = env.get(token) {
                box_jit_value(module, helpers, builder, ctx_val, token_val)
            } else {
                // Token not in scope — null token makes rt_reuse_some allocate fresh
                builder.ins().iconst(types::I64, 0)
            };
            let inner_val = env
                .get(inner)
                .copied()
                .ok_or_else(|| format!("missing backend IR reuse_some inner {:?}", inner))?;
            let inner_boxed = box_jit_value(module, helpers, builder, ctx_val, inner_val);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_reuse_some");
            let call = builder
                .ins()
                .call(helper, &[ctx_val, token_boxed, inner_boxed]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::ReuseLeft { token, inner } => {
            let token_boxed = if let Some(&token_val) = env.get(token) {
                box_jit_value(module, helpers, builder, ctx_val, token_val)
            } else {
                // Token not in scope — null token makes rt_reuse_left allocate fresh
                builder.ins().iconst(types::I64, 0)
            };
            let inner_val = env
                .get(inner)
                .copied()
                .ok_or_else(|| format!("missing backend IR reuse_left inner {:?}", inner))?;
            let inner_boxed = box_jit_value(module, helpers, builder, ctx_val, inner_val);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_reuse_left");
            let call = builder
                .ins()
                .call(helper, &[ctx_val, token_boxed, inner_boxed]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::ReuseRight { token, inner } => {
            let token_boxed = if let Some(&token_val) = env.get(token) {
                box_jit_value(module, helpers, builder, ctx_val, token_val)
            } else {
                // Token not in scope — null token makes rt_reuse_right allocate fresh
                builder.ins().iconst(types::I64, 0)
            };
            let inner_val = env
                .get(inner)
                .copied()
                .ok_or_else(|| format!("missing backend IR reuse_right inner {:?}", inner))?;
            let inner_boxed = box_jit_value(module, helpers, builder, ctx_val, inner_val);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_reuse_right");
            let call = builder
                .ins()
                .call(helper, &[ctx_val, token_boxed, inner_boxed]);
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::ReuseAdt {
            token,
            constructor,
            fields,
            field_mask,
        } => {
            let token_boxed = if let Some(&token_val) = env.get(token) {
                box_jit_value(module, helpers, builder, ctx_val, token_val)
            } else {
                // Token not in scope — null token makes rt_reuse_adt allocate fresh
                builder.ins().iconst(types::I64, 0)
            };
            let name_str = interner.resolve(*constructor);
            let bytes = name_str.as_bytes().to_vec();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let name_ptr = builder.ins().global_value(PTR_TYPE, gv);
            let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            // Build tagged fields array
            let field_vals = fields
                .iter()
                .map(|var| {
                    env.get(var)
                        .copied()
                        .ok_or_else(|| format!("missing backend IR reuse_adt field {:?}", var))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (_slot, fields_ptr) = emit_tagged_stack_array(builder, &field_vals);
            let nfields = builder.ins().iconst(PTR_TYPE, fields.len() as i64);
            let call = if let Some(mask) = field_mask {
                let helper = get_helper_func_ref(module, helpers, builder, "rt_reuse_adt_masked");
                let mask_val = builder.ins().iconst(types::I64, *mask as i64);
                builder.ins().call(
                    helper,
                    &[
                        ctx_val,
                        token_boxed,
                        name_ptr,
                        name_len,
                        fields_ptr,
                        nfields,
                        mask_val,
                    ],
                )
            } else {
                let helper = get_helper_func_ref(module, helpers, builder, "rt_reuse_adt");
                builder.ins().call(
                    helper,
                    &[
                        ctx_val,
                        token_boxed,
                        name_ptr,
                        name_len,
                        fields_ptr,
                        nfields,
                    ],
                )
            };
            Ok(JitValue::boxed(builder.inst_results(call)[0]))
        }
        BackendIrExpr::IsUnique(var) => {
            let val = env
                .get(var)
                .ok_or_else(|| format!("missing JIT binding for IsUnique var {:?}", var))?;
            let boxed = box_jit_value(module, helpers, builder, ctx_val, *val);
            let helper = get_helper_func_ref(module, helpers, builder, "rt_is_unique");
            let call = builder.ins().call(helper, &[ctx_val, boxed]);
            let result = builder.inst_results(call)[0];
            Ok(JitValue::int(result))
        }
        _ => Err("unsupported backend IR expression in direct JIT path".to_string()),
    }
}

pub(super) fn compile_backend_named_adt_constructor_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    constructor_name: Identifier,
    arg_vals: &[JitValue],
    interner: &Interner,
) -> Result<JitValue, String> {
    let name_str = interner.resolve(constructor_name);
    let bytes = name_str.as_bytes().to_vec();

    let data = module
        .declare_anonymous_data(false, false)
        .map_err(|e| e.to_string())?;
    let mut desc = DataDescription::new();
    desc.define(bytes.into_boxed_slice());
    module.define_data(data, &desc).map_err(|e| e.to_string())?;

    let global_value = module.declare_data_in_func(data, builder.func);
    let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
    let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);

    let boxed_arg_vals: Vec<_> = arg_vals
        .iter()
        .map(|value| box_jit_value(module, helpers, builder, ctx_val, *value))
        .collect();
    emit_push_gc_roots(module, helpers, builder, ctx_val, &boxed_arg_vals);

    let call = match arg_vals.len() {
        0 => {
            let fields_ptr = builder.ins().iconst(PTR_TYPE, 0);
            let arity_value = builder.ins().iconst(PTR_TYPE, 0);
            let make_adt = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
            builder.ins().call(
                make_adt,
                &[ctx_val, name_ptr, name_len, fields_ptr, arity_value],
            )
        }
        1 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt1");
            builder
                .ins()
                .call(helper, &[ctx_val, name_ptr, name_len, boxed_arg_vals[0]])
        }
        2 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt2");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                ],
            )
        }
        3 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt3");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                ],
            )
        }
        4 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt4");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    boxed_arg_vals[3],
                ],
            )
        }
        5 => {
            let helper = get_helper_func_ref(module, helpers, builder, "rt_make_adt5");
            builder.ins().call(
                helper,
                &[
                    ctx_val,
                    name_ptr,
                    name_len,
                    boxed_arg_vals[0],
                    boxed_arg_vals[1],
                    boxed_arg_vals[2],
                    boxed_arg_vals[3],
                    boxed_arg_vals[4],
                ],
            )
        }
        _ => {
            let function_compiler = FunctionCompiler::new(builder, 0, arg_vals.len());
            let fields_ptr = function_compiler.emit_tagged_array(builder, arg_vals);
            let arity_value = builder.ins().iconst(PTR_TYPE, arg_vals.len() as i64);
            let make_adt = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
            builder.ins().call(
                make_adt,
                &[ctx_val, name_ptr, name_len, fields_ptr, arity_value],
            )
        }
    };

    emit_pop_gc_roots(module, helpers, builder, ctx_val);
    Ok(JitValue::boxed(builder.inst_results(call)[0]))
}

pub(super) fn compile_simple_backend_ir_binary(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    op: IrBinaryOp,
    lhs: JitValue,
    rhs: JitValue,
) -> Result<JitValue, String> {
    if lhs.kind == JitValueKind::Int && rhs.kind == JitValueKind::Int {
        match op {
            IrBinaryOp::Add | IrBinaryOp::IAdd => {
                return Ok(JitValue::int(builder.ins().iadd(lhs.value, rhs.value)));
            }
            IrBinaryOp::Sub | IrBinaryOp::ISub => {
                return Ok(JitValue::int(builder.ins().isub(lhs.value, rhs.value)));
            }
            IrBinaryOp::Mul | IrBinaryOp::IMul => {
                return Ok(JitValue::int(builder.ins().imul(lhs.value, rhs.value)));
            }
            _ => {}
        }
    }

    let (lhs_tag, lhs_payload) = jit_value_to_tag_payload(builder, lhs);
    let (rhs_tag, rhs_payload) = jit_value_to_tag_payload(builder, rhs);
    let helper_name = match op {
        IrBinaryOp::Add | IrBinaryOp::IAdd => "rt_add",
        IrBinaryOp::Sub | IrBinaryOp::ISub => "rt_sub",
        IrBinaryOp::Mul | IrBinaryOp::IMul => "rt_mul",
        IrBinaryOp::Div | IrBinaryOp::IDiv => "rt_div",
        IrBinaryOp::Mod | IrBinaryOp::IMod => "rt_mod",
        IrBinaryOp::FAdd => "rt_add",
        IrBinaryOp::FSub => "rt_sub",
        IrBinaryOp::FMul => "rt_mul",
        IrBinaryOp::FDiv => "rt_div",
        IrBinaryOp::Eq => "rt_equal",
        IrBinaryOp::NotEq => "rt_not_equal",
        IrBinaryOp::Gt => "rt_greater_than",
        IrBinaryOp::Ge => "rt_greater_than_or_equal",
        IrBinaryOp::Le => "rt_less_than_or_equal",
        IrBinaryOp::Lt => {
            let ge_ref = get_helper_func_ref(module, helpers, builder, "rt_greater_than_or_equal");
            let ge_call = builder.ins().call(
                ge_ref,
                &[ctx_val, lhs_tag, lhs_payload, rhs_tag, rhs_payload],
            );
            let ge_tag = builder.inst_results(ge_call)[0];
            let ge_payload = builder.inst_results(ge_call)[1];
            let not_ref = get_helper_func_ref(module, helpers, builder, "rt_not");
            let not_call = builder.ins().call(not_ref, &[ctx_val, ge_tag, ge_payload]);
            let result = boxed_value_from_tagged_parts(
                module,
                helpers,
                builder,
                ctx_val,
                builder.inst_results(not_call)[0],
                builder.inst_results(not_call)[1],
            );
            return Ok(JitValue::boxed(result));
        }
        IrBinaryOp::And | IrBinaryOp::Or => {
            let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
            let lhs_truthy_call = builder
                .ins()
                .call(is_truthy, &[ctx_val, lhs_tag, lhs_payload]);
            let lhs_truthy = builder.inst_results(lhs_truthy_call)[0];
            let lhs_is_truthy = builder.ins().icmp_imm(IntCC::NotEqual, lhs_truthy, 0);
            let lhs_boxed = box_and_guard_jit_value(module, helpers, builder, ctx_val, lhs);
            let rhs_boxed = box_and_guard_jit_value(module, helpers, builder, ctx_val, rhs);
            let lhs_block = builder.create_block();
            let rhs_block = builder.create_block();
            let done_block = builder.create_block();
            builder.append_block_param(done_block, PTR_TYPE);

            match op {
                IrBinaryOp::And => {
                    builder
                        .ins()
                        .brif(lhs_is_truthy, rhs_block, &[], lhs_block, &[]);
                }
                IrBinaryOp::Or => {
                    builder
                        .ins()
                        .brif(lhs_is_truthy, lhs_block, &[], rhs_block, &[]);
                }
                _ => unreachable!(),
            }

            builder.switch_to_block(lhs_block);
            builder
                .ins()
                .jump(done_block, &[BlockArg::Value(lhs_boxed)]);
            builder.seal_block(lhs_block);

            builder.switch_to_block(rhs_block);
            builder
                .ins()
                .jump(done_block, &[BlockArg::Value(rhs_boxed)]);
            builder.seal_block(rhs_block);

            builder.switch_to_block(done_block);
            let result = builder.block_params(done_block)[0];
            builder.seal_block(done_block);
            return Ok(JitValue::boxed(result));
        }
    };

    let func_ref = get_helper_func_ref(module, helpers, builder, helper_name);
    let call = builder.ins().call(
        func_ref,
        &[ctx_val, lhs_tag, lhs_payload, rhs_tag, rhs_payload],
    );
    let result = boxed_value_from_tagged_parts(
        module,
        helpers,
        builder,
        ctx_val,
        builder.inst_results(call)[0],
        builder.inst_results(call)[1],
    );
    Ok(JitValue::boxed(result))
}

pub(super) fn compile_simple_backend_ir_truthiness_condition(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value: JitValue,
) -> CraneliftValue {
    let truthy_i64 = match value.kind {
        JitValueKind::Bool => value.value,
        _ => {
            let boxed = box_and_guard_jit_value(module, helpers, builder, ctx_val, value);
            let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
            let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
            let call = builder.ins().call(is_truthy, &[ctx_val, tag, boxed]);
            builder.inst_results(call)[0]
        }
    };

    builder.ins().icmp_imm(IntCC::NotEqual, truthy_i64, 0)
}

/// After a fallible runtime operation (e.g. rt_add), check if ctx.error is set.
/// If so, render the error with span info and return a null-tagged value to
/// propagate the error upward. Builder is left at the continue block.
#[allow(dead_code)]
fn emit_error_check_and_return(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    span: Span,
) {
    let has_error = get_helper_func_ref(module, helpers, builder, "rt_has_error");
    let call = builder.ins().call(has_error, &[ctx_val]);
    let err_flag = builder.inst_results(call)[0];
    let is_err = builder.ins().icmp_imm(IntCC::NotEqual, err_flag, 0);

    let err_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_err, err_block, &[], continue_block, &[]);

    builder.switch_to_block(err_block);
    emit_render_error_with_span(module, helpers, builder, ctx_val, span);
    emit_return_null_tagged(builder);
    builder.seal_block(err_block);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
}

/// After a runtime helper that may set `ctx.error`, emit a call to
/// `rt_render_error_with_span` so the raw error is rendered as a structured
/// diagnostic with source location.  This produces VM-parity error output.
#[allow(dead_code)]
fn emit_render_error_with_span(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    span: Span,
) {
    let render = get_helper_func_ref(module, helpers, builder, "rt_render_error_with_span");
    let start_line = builder.ins().iconst(PTR_TYPE, span.start.line as i64);
    let start_col = builder
        .ins()
        .iconst(PTR_TYPE, (span.start.column + 1) as i64);
    let end_line = builder.ins().iconst(PTR_TYPE, span.end.line as i64);
    let end_col = builder.ins().iconst(PTR_TYPE, (span.end.column + 1) as i64);
    builder
        .ins()
        .call(render, &[ctx_val, start_line, start_col, end_line, end_col]);
}
fn emit_return_on_null_value(builder: &mut FunctionBuilder, value_ptr: CraneliftValue) {
    let is_null = builder.ins().icmp_imm(IntCC::Equal, value_ptr, 0);
    let null_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_null, null_block, &[], continue_block, &[]);

    builder.switch_to_block(null_block);
    emit_return_null_tagged(builder);
    builder.seal_block(null_block);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
}

/// Like `emit_return_on_null_value` but also renders the raw error in
/// `ctx.error` as a structured diagnostic with source span before returning.
/// This is the primary mechanism for VM/JIT error parity.
#[allow(dead_code)]
fn emit_return_on_null_with_render(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    value_ptr: CraneliftValue,
    span: Span,
) {
    let is_null = builder.ins().icmp_imm(IntCC::Equal, value_ptr, 0);
    let null_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_null, null_block, &[], continue_block, &[]);

    builder.switch_to_block(null_block);
    emit_render_error_with_span(module, helpers, builder, ctx_val, span);
    emit_return_null_tagged(builder);
    builder.seal_block(null_block);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
}
