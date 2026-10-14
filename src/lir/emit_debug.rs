//! DWARF line tables for native code (proposal 0188 S5).
//!
//! A `-g` build gives each emitted module one compile unit and file, and
//! each Flux function a `DISubprogram`. Each instruction gets a location
//! from the LIR position it was emitted for. Code the compiler made (closure
//! prologues, reference counting, yield checks) is at line 0, which a
//! debugger steps over. Variables and types are S6, so the compile unit is
//! line tables only.

use std::collections::HashMap;
use std::path::Path;

use crate::llvm::ir::{BlockDebug, FunctionDebug, LlvmFunction, ModuleDebug};

use super::{LirFnKind, LirFunction, LirLoc};

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
             isOptimized: false, runtimeVersion: 0, emissionKind: LineTablesOnly)",
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
        }
    }

    /// Describes `emitted`, the LLVM function made from `func`, whose blocks'
    /// locations are `blocks`. A function lowering gave no name to stays
    /// without debug information, like a C function built without `-g`.
    pub(super) fn add_function(
        &mut self,
        emitted: &LlvmFunction,
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
        let subprogram = self.debug.node(format!(
            "distinct !DISubprogram(name: \"{}\", linkageName: \"{}\", scope: !{file}, \
             file: !{file}, line: {}, type: !{}, scopeLine: {scope_line}, flags: {flags}, \
             spFlags: DISPFlagDefinition, unit: !{})",
            escape(&name),
            escape(&emitted.name.0),
            debug.loc.line,
            self.subroutine_type,
            self.unit,
            file = self.file,
        ));
        let line_zero = self.location(LirLoc::NONE, subprogram);
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
