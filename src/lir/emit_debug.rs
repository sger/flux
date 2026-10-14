//! DWARF for native code (proposal 0188 S5 and S6).
//!
//! A `-g` build gives each emitted module one compile unit and file, and
//! each Flux function a `DISubprogram`. Each instruction gets a location
//! from the LIR position it was emitted for. Code the compiler made (closure
//! prologues, reference counting, yield checks) is at line 0, which a
//! debugger steps over.
//!
//! Variables live in SSA values, which an unoptimised build keeps in
//! registers a debugger mostly cannot name. So each source variable gets a
//! stack slot of its own that only the debugger reads, described by
//! `llvm.dbg.declare`: the value is stored there when the variable is bound,
//! and a marker when a heap value is dropped, so a debugger never reads freed
//! memory.

use std::collections::HashMap;
use std::path::Path;

use crate::core::FluxRep;
use crate::llvm::ir::{
    BlockDebug, CallConv, FunctionDebug, GlobalId, LlvmConst, LlvmFunction, LlvmInstr, LlvmLocal,
    LlvmOperand, LlvmType, ModuleDebug,
};

use super::{LirFnKind, LirFunction, LirLoc, LirVarKind};

/// What a debug slot holds before its variable is bound. `flux_lldb.py`
/// hides such a variable. The markers are even, so no tagged Int, and far
/// above any user-space address, so no pointer; a worker's raw Int could
/// hold one, but is unlikely to.
pub const UNBOUND_MARKER: i64 = 0x0DEB_0DEB_0000_0004;
/// What a debug slot holds once its value has been dropped.
pub const DROPPED_MARKER: i64 = 0x0DEB_0DEB_0000_0002;

/// What a debug build needs to know about the module being emitted.
#[derive(Debug, Clone)]
pub struct NativeDebug {
    /// The source file the module was compiled from.
    pub file: String,
}

/// Where each LLVM block's instructions came from: marks of (first
/// instruction, LIR location), and the terminator's location.
pub(super) type BlockLocs = (Vec<(usize, LirLoc)>, LirLoc);

pub(super) struct DebugBuilder {
    debug: ModuleDebug,
    file: u32,
    unit: u32,
    subroutine_type: u32,
    file_name: String,
    locations: HashMap<(u32, u32, u32), u32>,
    /// The variable types, by name, made when first used.
    types: HashMap<&'static str, u32>,
    /// Whether a function calls `llvm.dbg.declare`, which the module must
    /// then declare.
    declares: bool,
    /// The symbol of the global holding this module's types, unique to the
    /// source file, and its debug information once described.
    types_symbol: String,
    types_global: Option<u32>,
}

impl DebugBuilder {
    pub(super) fn new(native: &NativeDebug) -> Self {
        let path = Path::new(&native.file);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|dir| dir.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        let file_name = absolute
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| native.file.clone());
        let directory = absolute
            .parent()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();

        let mut debug = ModuleDebug::default();
        let unit = debug.node(String::new());
        let file = debug.node(format!(
            "!DIFile(filename: \"{}\", directory: \"{}\")",
            escape(&file_name),
            escape(&directory)
        ));
        debug.nodes[unit as usize] = format!(
            "distinct !DICompileUnit(language: DW_LANG_C, file: !{file}, producer: \"flux {}\", \
             isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug)",
            env!("CARGO_PKG_VERSION")
        );
        let dwarf_version = debug.node("!{i32 7, !\"Dwarf Version\", i32 4}".to_string());
        let info_version = debug.node("!{i32 2, !\"Debug Info Version\", i32 3}".to_string());
        let no_types = debug.node("!{}".to_string());
        let subroutine_type = debug.node(format!("!DISubroutineType(types: !{no_types})"));
        debug.named.push(("llvm.dbg.cu".to_string(), vec![unit]));
        debug.named.push((
            "llvm.module.flags".to_string(),
            vec![dwarf_version, info_version],
        ));
        Self {
            debug,
            file,
            unit,
            subroutine_type,
            file_name,
            locations: HashMap::new(),
            types: HashMap::new(),
            declares: false,
            types_symbol: format!(
                "flux_debug_types_{:016x}",
                fnv1a(&absolute.display().to_string())
            ),
            types_global: None,
        }
    }

    /// Describes `emitted`, the LLVM function made from `func`, whose blocks'
    /// locations are `blocks`. A function lowering gave no name to stays
    /// without debug information, like a C function built without `-g`.
    pub(super) fn add_function(
        &mut self,
        emitted: &mut LlvmFunction,
        func: &LirFunction,
        blocks: &[BlockLocs],
    ) {
        let Some(debug) = &func.debug else {
            return;
        };
        let name = match &debug.kind {
            LirFnKind::Named(name) => name.clone(),
            LirFnKind::Lambda => format!(
                "<lambda {}:{}:{}>",
                self.file_name, debug.loc.line, debug.loc.col
            ),
            LirFnKind::Resumed { parent, after } => {
                format!("{parent} (resumed after {}:{})", after.line, after.col)
            }
        };
        let scope_line = match &debug.kind {
            LirFnKind::Resumed { after, .. } if !after.is_none() => after.line,
            _ => debug.loc.line,
        };
        let flags = if debug.artificial {
            "DIFlagArtificial | DIFlagPrototyped"
        } else {
            "DIFlagPrototyped"
        };
        // No `linkageName`: with one, lldb names the frame by the symbol
        // (`flux_app_square`) rather than by `name`.
        let subprogram = self.debug.node(format!(
            "distinct !DISubprogram(name: \"{}\", scope: !{file}, \
             file: !{file}, line: {}, type: !{}, scopeLine: {scope_line}, flags: {flags}, \
             spFlags: DISPFlagDefinition, unit: !{})",
            escape(&name),
            debug.loc.line,
            self.subroutine_type,
            self.unit,
            file = self.file,
        ));
        let line_zero = self.location(LirLoc::NONE, subprogram);
        let mut blocks = blocks.to_vec();
        self.add_variables(emitted, func, subprogram, &mut blocks);
        let blocks = blocks
            .iter()
            .map(|(marks, term)| BlockDebug {
                marks: marks
                    .iter()
                    .map(|(from, loc)| (*from, self.location(*loc, subprogram)))
                    .collect(),
                term: Some(self.location(*term, subprogram)),
            })
            .collect();
        self.debug.functions.insert(
            emitted.name.0.clone(),
            FunctionDebug {
                subprogram,
                line_zero,
                blocks,
            },
        );
    }

    /// Gives each source variable of `func` defined in `emitted` a debug
    /// slot, and moves the blocks' marks past the instructions added.
    fn add_variables(
        &mut self,
        emitted: &mut LlvmFunction,
        func: &LirFunction,
        subprogram: u32,
        blocks: &mut [BlockLocs],
    ) {
        let Some(debug) = &func.debug else {
            return;
        };
        if debug.vars.is_empty() || emitted.blocks.is_empty() {
            return;
        }
        // The worker variant keeps Ints and Bools raw.
        let worker = emitted.name.0.ends_with("$w");
        let defined: std::collections::HashSet<&str> = emitted
            .params
            .iter()
            .map(|param| param.0.as_str())
            .chain(
                emitted
                    .blocks
                    .iter()
                    .flat_map(|block| block.instrs.iter().filter_map(stored_result))
                    .map(|local| local.0.as_str()),
            )
            .collect();

        // Each variable defined here, its slot, and the slots of each value.
        let mut slots: HashMap<String, Vec<LlvmLocal>> = HashMap::new();
        let mut prologue = Vec::new();
        let mut declares = Vec::new();
        for (index, var) in debug.vars.iter().enumerate() {
            let value = format!("v{}", var.var.0);
            if !defined.contains(value.as_str()) {
                continue;
            }
            let param = func.params.iter().position(|param| *param == var.var);
            let rep = match (var.kind, param) {
                (LirVarKind::Param, Some(position)) => {
                    func.param_reps.get(position).copied().unwrap_or(var.rep)
                }
                _ => var.rep,
            };
            let ty = if var.kind == LirVarKind::Captured && !worker {
                self.word_type("FluxCaptured")
            } else {
                self.var_type(worker, rep)
            };
            let arg = match (var.kind, param) {
                (LirVarKind::Param, Some(position)) => format!("arg: {}, ", position + 1),
                _ => String::new(),
            };
            let flags = if var.name.starts_with("__") {
                ", flags: DIFlagArtificial"
            } else {
                ""
            };
            let line = if var.loc.is_none() {
                debug.loc.line
            } else {
                var.loc.line
            };
            let variable = self.debug.node(format!(
                "!DILocalVariable(name: \"{}\", {arg}scope: !{subprogram}, file: !{}, \
                 line: {line}, type: !{ty}{flags})",
                escape(&var.name),
                self.file,
            ));
            let slot = LlvmLocal(format!("dbg.slot.{index}"));
            prologue.push(LlvmInstr::Alloca {
                dst: slot.clone(),
                ty: LlvmType::i64(),
                count: None,
                align: Some(8),
            });
            prologue.push(store(marker(UNBOUND_MARKER), &slot));
            declares.push(LlvmInstr::Call {
                dst: None,
                tail: false,
                call_conv: None,
                ret_ty: LlvmType::Void,
                callee: LlvmOperand::Global(GlobalId("llvm.dbg.declare".to_string())),
                args: vec![
                    (
                        LlvmType::Metadata,
                        LlvmOperand::Metadata(format!("ptr %{}", slot.0)),
                    ),
                    (
                        LlvmType::Metadata,
                        LlvmOperand::Metadata(format!("!{variable}")),
                    ),
                    (
                        LlvmType::Metadata,
                        LlvmOperand::Metadata("!DIExpression()".to_string()),
                    ),
                ],
                attrs: Vec::new(),
            });
            slots.entry(value).or_default().push(slot);
        }
        if slots.is_empty() {
            return;
        }
        self.declares = true;
        prologue.append(&mut declares);
        for param in &emitted.params {
            for slot in slots.get(&param.0).into_iter().flatten() {
                prologue.push(store(LlvmOperand::Local(param.clone()), slot));
            }
        }

        let mut dropped_count = 0;
        for (position, block) in emitted.blocks.iter_mut().enumerate() {
            let old = std::mem::take(&mut block.instrs);
            let mut instrs = Vec::with_capacity(old.len());
            // Where each old instruction went.
            let mut moved = Vec::with_capacity(old.len() + 1);
            if position == 0 {
                instrs.extend(prologue.iter().cloned());
            }
            let phis = old
                .iter()
                .take_while(|instr| matches!(instr, LlvmInstr::Phi { .. }))
                .count();
            for (index, instr) in old.into_iter().enumerate() {
                moved.push(instrs.len());
                let mut after = Vec::new();
                if let Some(local) = stored_result(&instr) {
                    for slot in slots.get(&local.0).into_iter().flatten() {
                        after.push(store(LlvmOperand::Local(local.clone()), slot));
                    }
                }
                if let Some(local) = dropped(&instr)
                    && let Some(local_slots) = slots.get(&local.0)
                {
                    let kept = LlvmLocal(format!("dbg.kept.{}", dropped_count));
                    dropped_count += 1;
                    after.extend(kept_after_drop(local, &kept));
                    for slot in local_slots {
                        after.push(store(LlvmOperand::Local(kept.clone()), slot));
                    }
                }
                instrs.push(instr);
                if index < phis {
                    // Phis come first in a block: store after the last.
                    block.instrs.extend(after);
                } else {
                    if index == phis {
                        let pending = std::mem::take(&mut block.instrs);
                        instrs.splice(instrs.len() - 1..instrs.len() - 1, pending);
                        *moved.last_mut().expect("just pushed") = instrs.len() - 1;
                    }
                    instrs.extend(after);
                }
            }
            // A block of phis only.
            instrs.append(&mut block.instrs);
            moved.push(instrs.len());
            block.instrs = instrs;
            if let Some((marks, _)) = blocks.get_mut(position) {
                for (from, _) in marks.iter_mut() {
                    *from = moved.get(*from).copied().unwrap_or(*from);
                }
            }
        }
    }

    /// The DWARF type of a variable represented as `rep`. Every value is a
    /// 64-bit word, and its typedef tells `flux_lldb.py` how to read it.
    fn var_type(&mut self, worker: bool, rep: FluxRep) -> u32 {
        let name = match (worker, rep) {
            (true, FluxRep::IntRep) => "FluxInt",
            (true, FluxRep::BoolRep) => "FluxBool",
            (true, FluxRep::BoxedRep) | (false, _) => "FluxValue",
            (true, _) => "FluxWord",
        };
        self.word_type(name)
    }

    /// The typedef `name` of a 64-bit word.
    fn word_type(&mut self, name: &'static str) -> u32 {
        if let Some(node) = self.types.get(name) {
            return *node;
        }
        let word = match self.types.get("int64_t") {
            Some(node) => *node,
            None => {
                let node = self.debug.node(
                    "!DIBasicType(name: \"int64_t\", size: 64, encoding: DW_ATE_signed)"
                        .to_string(),
                );
                self.types.insert("int64_t", node);
                node
            }
        };
        let node = self.debug.node(format!(
            "!DIDerivedType(tag: DW_TAG_typedef, name: \"{name}\", file: !{}, baseType: !{word})",
            self.file
        ));
        self.types.insert(name, node);
        node
    }

    /// Describes the constructors `program` knows, for `flux_lldb.py`:
    ///
    /// - `FluxCtor`, an enumeration naming each constructor tag;
    /// - for each constructor this module declares with fields, a structure
    ///   `FluxCtor.Name` laid out as its heap object, naming the fields.
    ///
    /// No variable has these types, so the compile unit retains them.
    pub(super) fn add_constructors(&mut self, program: &super::LirProgram) {
        let mut tags: Vec<(i32, &str)> = [(1, "Some"), (2, "Left"), (3, "Right"), (4, "Cons")]
            .into_iter()
            .chain(
                program
                    .constructor_tags
                    .iter()
                    .map(|(name, tag)| (*tag, name.as_str())),
            )
            .collect();
        tags.sort();
        tags.dedup_by_key(|(tag, _)| *tag);
        let enumerators: Vec<String> = tags
            .iter()
            .map(|(tag, name)| {
                let node = self.debug.node(format!(
                    "!DIEnumerator(name: \"{}\", value: {tag})",
                    escape(name)
                ));
                format!("!{node}")
            })
            .collect();
        let elements = self.debug.node(format!("!{{{}}}", enumerators.join(", ")));
        let int32 = self
            .debug
            .node("!DIBasicType(name: \"int32_t\", size: 32, encoding: DW_ATE_signed)".to_string());
        let ctor = self.debug.node(format!(
            "!DICompositeType(tag: DW_TAG_enumeration_type, name: \"FluxCtor\", file: !{}, \
             size: 32, baseType: !{int32}, elements: !{elements})",
            self.file
        ));
        let mut retained = vec![ctor];

        let value = self.var_type(false, FluxRep::TaggedRep);
        let mut declared: Vec<(&String, &Vec<Option<String>>)> =
            program.constructor_fields.iter().collect();
        declared.sort();
        for (name, fields) in declared {
            if fields.is_empty() {
                continue;
            }
            let mut members = vec![
                self.member("ctor_tag", ctor, 32, 0),
                self.member("field_count", int32, 32, 32),
            ];
            for (index, field) in fields.iter().enumerate() {
                let field_name = field.clone().unwrap_or_else(|| format!("_{index}"));
                members.push(self.member(&field_name, value, 64, 64 + 64 * index as u64));
            }
            let elements = self.debug.node(format!(
                "!{{{}}}",
                members
                    .iter()
                    .map(|member| format!("!{member}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            retained.push(self.debug.node(format!(
                "!DICompositeType(tag: DW_TAG_structure_type, name: \"FluxCtor.{}\", \
                 file: !{}, size: {}, elements: !{elements})",
                escape(name),
                self.file,
                64 + 64 * fields.len()
            )));
        }

        // `dsymutil` keeps only what reachable code or data refers to, so a
        // global, `FluxDebugTypes`, holds every type here.
        let mut holder = Vec::new();
        for (index, ty) in retained.iter().enumerate() {
            holder.push(self.member(&format!("_{index}"), *ty, 0, 0));
        }
        let holder_elements = self.list(&holder);
        let holder = self.debug.node(format!(
            "!DICompositeType(tag: DW_TAG_structure_type, name: \"FluxDebugTypes\", \
             file: !{}, size: 8, elements: !{holder_elements})",
            self.file
        ));
        let variable = self.debug.node(format!(
            "distinct !DIGlobalVariable(name: \"{}\", scope: !{}, file: !{}, type: !{holder}, \
             isLocal: false, isDefinition: true)",
            self.types_symbol, self.unit, self.file
        ));
        let expression = self.debug.node(format!(
            "!DIGlobalVariableExpression(var: !{variable}, expr: !DIExpression())"
        ));
        self.types_global = Some(expression);

        let retained = self.list(&retained);
        let globals = self.list(&[expression]);
        let unit = &mut self.debug.nodes[self.unit as usize];
        if let Some(open) = unit.strip_suffix(')') {
            *unit = format!("{open}, retainedTypes: !{retained}, globals: !{globals})");
        }
    }

    /// A node listing `nodes`.
    fn list(&mut self, nodes: &[u32]) -> u32 {
        self.debug.node(format!(
            "!{{{}}}",
            nodes
                .iter()
                .map(|node| format!("!{node}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    /// The global that keeps this module's Flux types in the debug
    /// information, once [`Self::add_constructors`] has described them.
    pub(super) fn types_global(&self) -> Option<crate::llvm::ir::LlvmGlobal> {
        let expression = self.types_global?;
        Some(crate::llvm::ir::LlvmGlobal {
            linkage: crate::llvm::ir::Linkage::External,
            name: GlobalId(self.types_symbol.clone()),
            ty: LlvmType::Integer(8),
            is_constant: true,
            value: Some(LlvmConst::Int { bits: 8, value: 0 }),
            attrs: vec![format!(", !dbg !{expression}")],
        })
    }

    fn member(&mut self, name: &str, ty: u32, size: u64, offset: u64) -> u32 {
        self.debug.node(format!(
            "!DIDerivedType(tag: DW_TAG_member, name: \"{}\", file: !{}, baseType: !{ty}, \
             size: {size}, offset: {offset})",
            escape(name),
            self.file
        ))
    }

    /// Whether the module calls `llvm.dbg.declare`.
    pub(super) fn declares(&self) -> bool {
        self.declares
    }

    fn location(&mut self, loc: LirLoc, scope: u32) -> u32 {
        let key = (loc.line, loc.col, scope);
        if let Some(node) = self.locations.get(&key) {
            return *node;
        }
        let node = self.debug.node(format!(
            "!DILocation(line: {}, column: {}, scope: !{scope})",
            loc.line, loc.col
        ));
        self.locations.insert(key, node);
        node
    }

    pub(super) fn finish(self) -> ModuleDebug {
        self.debug
    }
}

/// The 64-bit value `instr` defines, which a debug slot can hold. A tail
/// call's result is left alone: a store between it and the `ret` would stop
/// the call being a tail call.
fn stored_result(instr: &LlvmInstr) -> Option<&LlvmLocal> {
    let i64_ty = LlvmType::i64();
    match instr {
        LlvmInstr::Load { dst, ty, .. }
        | LlvmInstr::Phi { dst, ty, .. }
        | LlvmInstr::Binary { dst, ty, .. }
            if *ty == i64_ty =>
        {
            Some(dst)
        }
        LlvmInstr::Cast { dst, to_ty, .. } if *to_ty == i64_ty => Some(dst),
        LlvmInstr::Select { dst, value_ty, .. } if *value_ty == i64_ty => Some(dst),
        LlvmInstr::Call {
            dst: Some(dst),
            ret_ty,
            tail: false,
            ..
        } if *ret_ty == i64_ty => Some(dst),
        _ => None,
    }
}

/// The value `instr` drops: after it, the value may be freed or reused.
fn dropped(instr: &LlvmInstr) -> Option<&LlvmLocal> {
    let LlvmInstr::Call { callee, args, .. } = instr else {
        return None;
    };
    let LlvmOperand::Global(GlobalId(name)) = callee else {
        return None;
    };
    if name != "flux_drop" && name != "flux_drop_reuse" {
        return None;
    }
    match args.first() {
        Some((_, LlvmOperand::Local(local))) => Some(local),
        _ => None,
    }
}

/// What a slot holds once `value` is dropped, computed into `kept`: the
/// value itself if it is an Int or a sentinel, which dropping leaves alone,
/// else the dropped marker, since the object may now be freed.
fn kept_after_drop(value: &LlvmLocal, kept: &LlvmLocal) -> Vec<LlvmInstr> {
    let part = |name: &str| LlvmLocal(format!("{}.{name}", kept.0));
    let value = LlvmOperand::Local(value.clone());
    vec![
        LlvmInstr::Binary {
            dst: part("tag"),
            op: crate::llvm::ir::LlvmValueKind::And,
            ty: LlvmType::i64(),
            lhs: value.clone(),
            rhs: marker(1),
        },
        LlvmInstr::Icmp {
            dst: part("int"),
            op: crate::llvm::ir::LlvmCmpOp::Ne,
            ty: LlvmType::i64(),
            lhs: LlvmOperand::Local(part("tag")),
            rhs: marker(0),
        },
        LlvmInstr::Icmp {
            dst: part("sentinel"),
            op: crate::llvm::ir::LlvmCmpOp::Ule,
            ty: LlvmType::i64(),
            lhs: value.clone(),
            rhs: marker(10),
        },
        LlvmInstr::Binary {
            dst: part("unboxed"),
            op: crate::llvm::ir::LlvmValueKind::Or,
            ty: LlvmType::i1(),
            lhs: LlvmOperand::Local(part("int")),
            rhs: LlvmOperand::Local(part("sentinel")),
        },
        LlvmInstr::Select {
            dst: kept.clone(),
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(part("unboxed")),
            value_ty: LlvmType::i64(),
            then_value: value,
            else_value: marker(DROPPED_MARKER),
        },
    ]
}

fn marker(value: i64) -> LlvmOperand {
    LlvmOperand::Const(LlvmConst::Int {
        bits: 64,
        value: value as i128,
    })
}

fn store(value: LlvmOperand, slot: &LlvmLocal) -> LlvmInstr {
    LlvmInstr::Store {
        ty: LlvmType::i64(),
        value,
        ptr: LlvmOperand::Local(slot.clone()),
        align: Some(8),
    }
}

/// `declare void @llvm.dbg.declare(metadata, metadata, metadata)`.
pub(super) fn dbg_declare() -> crate::llvm::ir::LlvmDecl {
    crate::llvm::ir::LlvmDecl {
        linkage: crate::llvm::ir::Linkage::External,
        name: GlobalId("llvm.dbg.declare".to_string()),
        sig: crate::llvm::ir::LlvmFunctionSig {
            ret: LlvmType::Void,
            params: vec![LlvmType::Metadata; 3],
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        attrs: vec!["nounwind".to_string()],
    }
}

/// A stable 64-bit hash, so a module's symbols do not change between builds.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'"' | b'\\' => out.push_str(&format!("\\{byte:02X}")),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:02X}")),
        }
    }
    out
}
