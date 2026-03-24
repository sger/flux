use std::fmt::{self, Display, Formatter};

use super::{
    CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmCmpOp, LlvmConst, LlvmDecl, LlvmFunction,
    LlvmGlobal, LlvmInstr, LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator, LlvmType,
    LlvmTypeDef, LlvmValueKind,
};

pub fn render_module(module: &LlvmModule) -> String {
    module.to_string()
}

impl Display for LlvmType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LlvmType::Void => write!(f, "void"),
            LlvmType::Integer(bits) => write!(f, "i{bits}"),
            LlvmType::Float => write!(f, "float"),
            LlvmType::Double => write!(f, "double"),
            LlvmType::Ptr => write!(f, "ptr"),
            LlvmType::Array { len, element } => write!(f, "[{len} x {element}]"),
            LlvmType::Struct { packed, fields } => {
                let body = comma_join(fields);
                if *packed {
                    write!(f, "<{{{body}}}>")
                } else {
                    write!(f, "{{{body}}}")
                }
            }
            LlvmType::Function {
                ret,
                params,
                varargs,
            } => {
                write!(f, "{ret} (")?;
                fmt_param_types(f, params, *varargs)?;
                write!(f, ")")
            }
            LlvmType::Named(name) => write!(f, "%{name}"),
        }
    }
}

impl Display for LlvmModule {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut need_blank = false;
        if let Some(source_filename) = &self.source_filename {
            writeln!(
                f,
                "source_filename = \"{}\"",
                escape_string(source_filename)
            )?;
            need_blank = true;
        }
        if let Some(target_triple) = &self.target_triple {
            writeln!(f, "target triple = \"{}\"", escape_string(target_triple))?;
            need_blank = true;
        }
        if let Some(data_layout) = &self.data_layout {
            writeln!(f, "target datalayout = \"{}\"", escape_string(data_layout))?;
            need_blank = true;
        }
        if need_blank
            && (!self.type_defs.is_empty()
                || !self.globals.is_empty()
                || !self.declarations.is_empty()
                || !self.functions.is_empty())
        {
            writeln!(f)?;
        }

        write_section(f, &self.type_defs)?;
        write_section(f, &self.globals)?;
        write_section(f, &self.declarations)?;
        write_section(f, &self.functions)
    }
}

impl Display for LlvmTypeDef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "%{} = type {}", self.name, self.ty)
    }
}

impl Display for LlvmGlobal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "@{} = {} {} {} {}",
            self.name,
            self.linkage,
            if self.is_constant {
                "constant"
            } else {
                "global"
            },
            self.ty,
            self.value
        )?;
        for attr in &self.attrs {
            write!(f, " {attr}")?;
        }
        Ok(())
    }
}

impl Display for LlvmDecl {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "declare ")?;
        fmt_linkage(f, self.linkage)?;
        write!(f, "{} {} @{}(", self.sig.call_conv, self.sig.ret, self.name)?;
        fmt_param_types(f, &self.sig.params, self.sig.varargs)?;
        write!(f, ")")?;
        for attr in &self.attrs {
            write!(f, " {attr}")?;
        }
        Ok(())
    }
}

impl Display for LlvmFunction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "define ")?;
        fmt_linkage(f, self.linkage)?;
        write!(f, "{} {} @{}(", self.sig.call_conv, self.sig.ret, self.name)?;
        for (idx, (ty, param)) in self.sig.params.iter().zip(&self.params).enumerate() {
            if idx > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{ty} {param}")?;
        }
        if self.sig.varargs {
            if !self.sig.params.is_empty() {
                write!(f, ", ")?;
            }
            write!(f, "...")?;
        }
        write!(f, ")")?;
        for attr in &self.attrs {
            write!(f, " {attr}")?;
        }
        writeln!(f, " {{")?;
        for block in &self.blocks {
            writeln!(f, "{block}")?;
        }
        write!(f, "}}")
    }
}

impl Display for LlvmBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}:", self.label)?;
        for instr in &self.instrs {
            writeln!(f, "  {instr}")?;
        }
        write!(f, "  {}", self.term)
    }
}

impl Display for LlvmInstr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LlvmInstr::Alloca {
                dst,
                ty,
                count,
                align,
            } => {
                write!(f, "{dst} = alloca {ty}")?;
                if let Some((count_ty, count_operand)) = count {
                    write!(f, ", {count_ty} {count_operand}")?;
                }
                if let Some(align) = align {
                    write!(f, ", align {align}")?;
                }
                Ok(())
            }
            LlvmInstr::Load {
                dst,
                ty,
                ptr,
                align,
            } => {
                write!(f, "{dst} = load {ty}, ptr {ptr}")?;
                if let Some(align) = align {
                    write!(f, ", align {align}")?;
                }
                Ok(())
            }
            LlvmInstr::Store {
                ty,
                value,
                ptr,
                align,
            } => {
                write!(f, "store {ty} {value}, ptr {ptr}")?;
                if let Some(align) = align {
                    write!(f, ", align {align}")?;
                }
                Ok(())
            }
            LlvmInstr::Binary {
                dst,
                op,
                ty,
                lhs,
                rhs,
            } => write!(f, "{dst} = {} {ty} {lhs}, {rhs}", value_kind_name(*op)),
            LlvmInstr::Cast {
                dst,
                op,
                from_ty,
                operand,
                to_ty,
            } => write!(
                f,
                "{dst} = {} {from_ty} {operand} to {to_ty}",
                value_kind_name(*op)
            ),
            LlvmInstr::Icmp {
                dst,
                op,
                ty,
                lhs,
                rhs,
            } => write!(f, "{dst} = icmp {} {ty} {lhs}, {rhs}", cmp_name(*op)),
            LlvmInstr::Fcmp {
                dst,
                op,
                ty,
                lhs,
                rhs,
            } => write!(f, "{dst} = fcmp {} {ty} {lhs}, {rhs}", cmp_name(*op)),
            LlvmInstr::Phi { dst, ty, incoming } => {
                write!(f, "{dst} = phi {ty} ")?;
                for (idx, (value, label)) in incoming.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "[ {value}, %{label} ]")?;
                }
                Ok(())
            }
            LlvmInstr::Select {
                dst,
                cond_ty,
                cond,
                value_ty,
                then_value,
                else_value,
            } => write!(
                f,
                "{dst} = select {cond_ty} {cond}, {value_ty} {then_value}, {value_ty} {else_value}"
            ),
            LlvmInstr::Call {
                dst,
                tail,
                call_conv,
                ret_ty,
                callee,
                args,
                attrs,
            } => {
                if let Some(dst) = dst {
                    write!(f, "{dst} = ")?;
                }
                if *tail {
                    write!(f, "tail ")?;
                }
                write!(f, "call ")?;
                if let Some(call_conv) = call_conv {
                    write!(f, "{call_conv} ")?;
                }
                write!(f, "{ret_ty} {callee}(")?;
                for (idx, (arg_ty, arg)) in args.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg_ty} {arg}")?;
                }
                write!(f, ")")?;
                for attr in attrs {
                    write!(f, " {attr}")?;
                }
                Ok(())
            }
            LlvmInstr::GetElementPtr {
                dst,
                inbounds,
                element_ty,
                base,
                indices,
            } => {
                write!(
                    f,
                    "{dst} = getelementptr {}{}, ptr {base}",
                    if *inbounds { "inbounds " } else { "" },
                    element_ty
                )?;
                for (idx_ty, idx_val) in indices {
                    write!(f, ", {idx_ty} {idx_val}")?;
                }
                Ok(())
            }
            LlvmInstr::ExtractValue {
                dst,
                aggregate_ty,
                aggregate,
                indices,
            } => {
                write!(f, "{dst} = extractvalue {aggregate_ty} {aggregate}")?;
                for idx in indices {
                    write!(f, ", {idx}")?;
                }
                Ok(())
            }
            LlvmInstr::InsertValue {
                dst,
                aggregate_ty,
                aggregate,
                element_ty,
                element,
                indices,
            } => {
                write!(
                    f,
                    "{dst} = insertvalue {aggregate_ty} {aggregate}, {element_ty} {element}"
                )?;
                for idx in indices {
                    write!(f, ", {idx}")?;
                }
                Ok(())
            }
        }
    }
}

impl Display for LlvmTerminator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LlvmTerminator::RetVoid => write!(f, "ret void"),
            LlvmTerminator::Ret { ty, value } => write!(f, "ret {ty} {value}"),
            LlvmTerminator::Br { target } => write!(f, "br label %{target}"),
            LlvmTerminator::CondBr {
                cond_ty,
                cond,
                then_label,
                else_label,
            } => write!(
                f,
                "br {cond_ty} {cond}, label %{then_label}, label %{else_label}"
            ),
            LlvmTerminator::Switch {
                ty,
                scrutinee,
                default,
                cases,
            } => {
                write!(f, "switch {ty} {scrutinee}, label %{default} [")?;
                if !cases.is_empty() {
                    writeln!(f)?;
                    for (value, target) in cases {
                        writeln!(f, "    {ty} {value}, label %{target}")?;
                    }
                    write!(f, "  ]")
                } else {
                    write!(f, " ]")
                }
            }
            LlvmTerminator::Unreachable => write!(f, "unreachable"),
        }
    }
}

impl Display for LlvmConst {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LlvmConst::Int { value, .. } => write!(f, "{value}"),
            LlvmConst::Float(value) => {
                if value.is_nan() || value.is_infinite() {
                    write!(f, "0x{:016X}", value.to_bits())
                } else if value.fract() == 0.0 {
                    write!(f, "{:.1}", value)
                } else {
                    write!(f, "0x{:016X}", value.to_bits())
                }
            }
            LlvmConst::Null => write!(f, "null"),
            LlvmConst::Undef => write!(f, "undef"),
            LlvmConst::Array {
                element_ty,
                elements,
            } => {
                write!(f, "[")?;
                for (idx, element) in elements.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{element_ty} {element}")?;
                }
                write!(f, "]")
            }
            LlvmConst::Struct { packed, fields } => {
                if *packed {
                    write!(f, "<{{")?;
                } else {
                    write!(f, "{{")?;
                }
                for (idx, (ty, value)) in fields.iter().enumerate() {
                    if idx > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{ty} {value}")?;
                }
                if *packed {
                    write!(f, "}}>")?;
                } else {
                    write!(f, "}}")?;
                }
                Ok(())
            }
            LlvmConst::GlobalRef(id) => write!(f, "@{id}"),
            LlvmConst::ZeroInit => write!(f, "zeroinitializer"),
        }
    }
}

impl Display for LlvmOperand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LlvmOperand::Local(local) => write!(f, "{local}"),
            LlvmOperand::Global(global) => write!(f, "@{global}"),
            LlvmOperand::Const(value) => write!(f, "{value}"),
        }
    }
}

impl Display for Linkage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Linkage::External => write!(f, "external"),
            Linkage::Private => write!(f, "private"),
            Linkage::Internal => write!(f, "internal"),
        }
    }
}

impl Display for CallConv {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            CallConv::Ccc => write!(f, "ccc"),
            CallConv::Fastcc => write!(f, "fastcc"),
        }
    }
}

impl Display for LlvmLocal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

impl Display for GlobalId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Display for LabelId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn write_section<T: Display>(f: &mut Formatter<'_>, items: &[T]) -> fmt::Result {
    if items.is_empty() {
        return Ok(());
    }
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            writeln!(f)?;
        }
        writeln!(f, "{item}")?;
    }
    if !items.is_empty() {
        writeln!(f)?;
    }
    Ok(())
}

fn fmt_linkage(f: &mut Formatter<'_>, linkage: Linkage) -> fmt::Result {
    match linkage {
        Linkage::External => Ok(()),
        _ => write!(f, "{linkage} "),
    }
}

fn fmt_param_types(f: &mut Formatter<'_>, params: &[LlvmType], varargs: bool) -> fmt::Result {
    for (idx, ty) in params.iter().enumerate() {
        if idx > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{ty}")?;
    }
    if varargs {
        if !params.is_empty() {
            write!(f, ", ")?;
        }
        write!(f, "...")?;
    }
    Ok(())
}

fn comma_join(items: &[LlvmType]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn cmp_name(op: LlvmCmpOp) -> &'static str {
    match op {
        LlvmCmpOp::Eq => "eq",
        LlvmCmpOp::Ne => "ne",
        LlvmCmpOp::Sgt => "sgt",
        LlvmCmpOp::Sge => "sge",
        LlvmCmpOp::Slt => "slt",
        LlvmCmpOp::Sle => "sle",
        LlvmCmpOp::Oeq => "oeq",
        LlvmCmpOp::One => "one",
        LlvmCmpOp::Ogt => "ogt",
        LlvmCmpOp::Oge => "oge",
        LlvmCmpOp::Olt => "olt",
        LlvmCmpOp::Ole => "ole",
    }
}

fn value_kind_name(op: LlvmValueKind) -> &'static str {
    match op {
        LlvmValueKind::Add => "add",
        LlvmValueKind::Sub => "sub",
        LlvmValueKind::Mul => "mul",
        LlvmValueKind::SDiv => "sdiv",
        LlvmValueKind::UDiv => "udiv",
        LlvmValueKind::SRem => "srem",
        LlvmValueKind::FAdd => "fadd",
        LlvmValueKind::FSub => "fsub",
        LlvmValueKind::FMul => "fmul",
        LlvmValueKind::FDiv => "fdiv",
        LlvmValueKind::FRem => "frem",
        LlvmValueKind::And => "and",
        LlvmValueKind::Or => "or",
        LlvmValueKind::Xor => "xor",
        LlvmValueKind::Shl => "shl",
        LlvmValueKind::LShr => "lshr",
        LlvmValueKind::AShr => "ashr",
        LlvmValueKind::Alloca => "alloca",
        LlvmValueKind::Load => "load",
        LlvmValueKind::GetElementPtr => "getelementptr",
        LlvmValueKind::ExtractValue => "extractvalue",
        LlvmValueKind::InsertValue => "insertvalue",
        LlvmValueKind::IntToPtr => "inttoptr",
        LlvmValueKind::PtrToInt => "ptrtoint",
        LlvmValueKind::Bitcast => "bitcast",
        LlvmValueKind::ZExt => "zext",
        LlvmValueKind::SExt => "sext",
        LlvmValueKind::Trunc => "trunc",
        LlvmValueKind::FpToSi => "fptosi",
        LlvmValueKind::SiToFp => "sitofp",
        LlvmValueKind::Icmp(_) => "icmp",
        LlvmValueKind::Fcmp(_) => "fcmp",
        LlvmValueKind::Phi => "phi",
        LlvmValueKind::Select => "select",
        LlvmValueKind::Call => "call",
    }
}

fn escape_string(input: &str) -> String {
    input.escape_default().to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use crate::core_to_llvm::LlvmFunctionSig;

    #[test]
    fn renders_basic_types() {
        assert_eq!(LlvmType::Void.to_string(), "void");
        assert_eq!(LlvmType::i64().to_string(), "i64");
        assert_eq!(LlvmType::ptr().to_string(), "ptr");
        assert_eq!(
            LlvmType::Array {
                len: 4,
                element: Box::new(LlvmType::i8())
            }
            .to_string(),
            "[4 x i8]"
        );
        assert_eq!(
            LlvmType::Struct {
                packed: false,
                fields: vec![LlvmType::i32(), LlvmType::ptr()]
            }
            .to_string(),
            "{i32, ptr}"
        );
    }

    #[test]
    fn renders_declarations_and_functions() {
        let decl = LlvmDecl {
            linkage: Linkage::External,
            name: GlobalId("puts".into()),
            sig: LlvmFunctionSig {
                ret: LlvmType::i32(),
                params: vec![LlvmType::ptr()],
                varargs: false,
                call_conv: CallConv::Ccc,
            },
            attrs: vec!["nounwind".into()],
        };
        assert_eq!(decl.to_string(), "declare ccc i32 @puts(ptr) nounwind");

        let func = LlvmFunction {
            linkage: Linkage::Internal,
            name: GlobalId("main".into()),
            sig: LlvmFunctionSig {
                ret: LlvmType::i64(),
                params: vec![LlvmType::i64()],
                varargs: false,
                call_conv: CallConv::Fastcc,
            },
            params: vec![LlvmLocal("n".into())],
            attrs: vec!["alwaysinline".into()],
            blocks: vec![LlvmBlock {
                label: LabelId("entry".into()),
                instrs: vec![],
                term: LlvmTerminator::Ret {
                    ty: LlvmType::i64(),
                    value: LlvmOperand::Local(LlvmLocal("n".into())),
                },
            }],
        };
        assert!(
            func.to_string()
                .contains("define internal fastcc i64 @main(i64 %n) alwaysinline")
        );
    }

    #[test]
    fn renders_phi_gep_call_and_switch() {
        let func = demo_function();
        let rendered = func.to_string();
        assert!(
            rendered.contains("%elt = getelementptr inbounds %FluxString, ptr %base, i32 0, i32 1")
        );
        assert!(rendered.contains("%result = phi i64 [ 1, %then ], [ 0, %else ]"));
        assert!(rendered.contains("%called = tail call fastcc i64 @helper(i64 %result) nounwind"));
        assert!(rendered.contains("switch i32 %tag, label %default ["));
    }

    #[test]
    fn renders_deterministic_module_output() {
        let expected = build_demo_module().to_string();
        assert_eq!(render_module(&build_demo_module()), expected);
    }

    #[test]
    fn optional_opt_verify_accepts_rendered_module() {
        if Command::new("opt").arg("--version").output().is_err() {
            return;
        }

        let module = build_demo_module();
        let ll = render_module(&module);
        let file = temp_ll_path("core_to_llvm_verify");
        fs::write(&file, ll).expect("write ll file");

        let output = Command::new("opt")
            .arg("--disable-output")
            .arg("-passes=verify")
            .arg(&file)
            .output()
            .expect("run opt");

        let _ = fs::remove_file(&file);
        assert!(
            output.status.success(),
            "opt verification failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn build_demo_module() -> LlvmModule {
        LlvmModule {
            source_filename: Some("demo.flx".into()),
            target_triple: Some("x86_64-unknown-linux-gnu".into()),
            data_layout: Some("e-m:e-p:64:64-i64:64-n8:16:32:64-S128".into()),
            type_defs: vec![LlvmTypeDef {
                name: "FluxString".into(),
                ty: LlvmType::Struct {
                    packed: false,
                    fields: vec![
                        LlvmType::i32(),
                        LlvmType::Array {
                            len: 0,
                            element: Box::new(LlvmType::i8()),
                        },
                    ],
                },
            }],
            globals: vec![LlvmGlobal {
                linkage: Linkage::Private,
                name: GlobalId("flux.tag.int".into()),
                ty: LlvmType::i64(),
                is_constant: true,
                value: LlvmConst::Int {
                    bits: 64,
                    value: 4607182418800017408,
                },
                attrs: vec![],
            }],
            declarations: vec![
                LlvmDecl {
                    linkage: Linkage::External,
                    name: GlobalId("puts".into()),
                    sig: LlvmFunctionSig {
                        ret: LlvmType::i32(),
                        params: vec![LlvmType::ptr()],
                        varargs: false,
                        call_conv: CallConv::Ccc,
                    },
                    attrs: vec!["nounwind".into()],
                },
                LlvmDecl {
                    linkage: Linkage::External,
                    name: GlobalId("helper".into()),
                    sig: LlvmFunctionSig {
                        ret: LlvmType::i64(),
                        params: vec![LlvmType::i64()],
                        varargs: false,
                        call_conv: CallConv::Fastcc,
                    },
                    attrs: vec![],
                },
            ],
            functions: vec![demo_function()],
        }
    }

    fn demo_function() -> LlvmFunction {
        LlvmFunction {
            linkage: Linkage::Internal,
            name: GlobalId("flux.demo".into()),
            sig: LlvmFunctionSig {
                ret: LlvmType::i64(),
                params: vec![LlvmType::ptr(), LlvmType::i32()],
                varargs: false,
                call_conv: CallConv::Fastcc,
            },
            params: vec![LlvmLocal("base".into()), LlvmLocal("tag".into())],
            attrs: vec!["alwaysinline".into()],
            blocks: vec![
                LlvmBlock {
                    label: LabelId("entry".into()),
                    instrs: vec![LlvmInstr::GetElementPtr {
                        dst: LlvmLocal("elt".into()),
                        inbounds: true,
                        element_ty: LlvmType::Named("FluxString".into()),
                        base: LlvmOperand::Local(LlvmLocal("base".into())),
                        indices: vec![
                            (
                                LlvmType::i32(),
                                LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 0 }),
                            ),
                            (
                                LlvmType::i32(),
                                LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 1 }),
                            ),
                        ],
                    }],
                    term: LlvmTerminator::Switch {
                        ty: LlvmType::i32(),
                        scrutinee: LlvmOperand::Local(LlvmLocal("tag".into())),
                        default: LabelId("default".into()),
                        cases: vec![
                            (
                                LlvmConst::Int { bits: 32, value: 0 },
                                LabelId("then".into()),
                            ),
                            (
                                LlvmConst::Int { bits: 32, value: 1 },
                                LabelId("else".into()),
                            ),
                        ],
                    },
                },
                LlvmBlock {
                    label: LabelId("then".into()),
                    instrs: vec![],
                    term: LlvmTerminator::Br {
                        target: LabelId("join".into()),
                    },
                },
                LlvmBlock {
                    label: LabelId("else".into()),
                    instrs: vec![],
                    term: LlvmTerminator::Br {
                        target: LabelId("join".into()),
                    },
                },
                LlvmBlock {
                    label: LabelId("default".into()),
                    instrs: vec![],
                    term: LlvmTerminator::Ret {
                        ty: LlvmType::i64(),
                        value: LlvmOperand::Const(LlvmConst::Int {
                            bits: 64,
                            value: -1,
                        }),
                    },
                },
                LlvmBlock {
                    label: LabelId("join".into()),
                    instrs: vec![
                        LlvmInstr::Phi {
                            dst: LlvmLocal("result".into()),
                            ty: LlvmType::i64(),
                            incoming: vec![
                                (
                                    LlvmOperand::Const(LlvmConst::Int { bits: 64, value: 1 }),
                                    LabelId("then".into()),
                                ),
                                (
                                    LlvmOperand::Const(LlvmConst::Int { bits: 64, value: 0 }),
                                    LabelId("else".into()),
                                ),
                            ],
                        },
                        LlvmInstr::Call {
                            dst: Some(LlvmLocal("called".into())),
                            tail: true,
                            call_conv: Some(CallConv::Fastcc),
                            ret_ty: LlvmType::i64(),
                            callee: LlvmOperand::Global(GlobalId("helper".into())),
                            args: vec![(
                                LlvmType::i64(),
                                LlvmOperand::Local(LlvmLocal("result".into())),
                            )],
                            attrs: vec!["nounwind".into()],
                        },
                    ],
                    term: LlvmTerminator::Ret {
                        ty: LlvmType::i64(),
                        value: LlvmOperand::Local(LlvmLocal("called".into())),
                    },
                },
            ],
        }
    }

    fn temp_ll_path(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}_{nonce}.ll"))
    }
}
