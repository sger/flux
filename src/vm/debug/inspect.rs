//! Reading a paused VM: its stack, each frame's variables, and breakpoints.

use std::sync::Arc;

use crate::bytecode::debug_info::FunctionDebugInfo;
use crate::runtime::compiled_function::CompiledFunction;
use crate::runtime::value::Value;

use super::super::{VM, slot};
use super::values::ValueView;
use super::{DebugControl, LineBreakpoint};

/// One frame of the call stack, newest first.
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// The VM frame index, for [`Inspector::locals`].
    pub index: usize,
    pub name: String,
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
    /// Compiler-made (dictionary methods, wrappers); hidden unless the
    /// debugger shows internals.
    pub artificial: bool,
    /// Calls this frame replaced by tail calls, and the latest of them.
    pub elided: Option<(u64, String)>,
}

/// A named value in a frame.
#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub value: ValueView,
    pub artificial: bool,
}

/// A paused VM, as the observer sees it.
pub struct Inspector<'a> {
    vm: &'a VM,
    control: &'a mut DebugControl,
}

/// A compiler-made function: dictionaries, wrappers and other `__` names.
/// An instance method (`__tc_…`) is code the person wrote, so it is not.
pub(super) fn is_artificial(name: Option<&str>) -> bool {
    name.is_some_and(|name| {
        // `Flow.Primops.__primop_print`: the name, not its module path. A
        // description after the name (`__default_handler Console.print`) is
        // not part of it.
        let name = name.split(' ').next().unwrap_or(name);
        let name = name.rsplit('.').next().unwrap_or(name);
        name.starts_with("__") && !name.starts_with(INSTANCE_METHOD)
    })
}

const INSTANCE_METHOD: &str = "__tc_";

/// How a function is named on the stack: an instance method as
/// `Class<Type>.method`, everything else as compiled.
pub(super) fn display_name(name: &str) -> String {
    name.strip_prefix(INSTANCE_METHOD)
        .and_then(demangle_instance_method)
        .unwrap_or_else(|| name.to_string())
}

/// `Class_Type_method`, where a module-qualified class or type is written
/// `m<len>_<module as hex>_<Name>` (see `types::class_env`).
fn demangle_instance_method(mangled: &str) -> Option<String> {
    let (class, rest) = mangled_symbol(mangled)?;
    let (ty, method) = mangled_type(rest)?;
    (!method.is_empty()).then(|| format!("{class}<{ty}>.{method}"))
}

/// One mangled name and the text after its separating `_`.
fn mangled_symbol(text: &str) -> Option<(String, &str)> {
    if let Some(qualified) = module_qualified(text) {
        return Some(qualified);
    }
    let (name, rest) = text.split_once('_')?;
    Some((name.to_string(), rest))
}

/// A type key: a name with any `<…>` arguments, up to its separating `_`.
fn mangled_type(text: &str) -> Option<(String, &str)> {
    if let Some(qualified) = module_qualified(text) {
        return Some(qualified);
    }
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            '_' if depth == 0 => return Some((text[..index].to_string(), &text[index + 1..])),
            _ => {}
        }
    }
    None
}

/// `m<len>_<hex>_<Name>_rest` as `Name` and `rest`: the module is left
/// out, as a stack line has no room for it.
fn module_qualified(text: &str) -> Option<(String, &str)> {
    let digits = text.strip_prefix('m')?;
    let (len, rest) = digits.split_once('_')?;
    let len: usize = len.parse().ok()?;
    let hex = rest.get(..len * 2)?;
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rest = rest.get(len * 2..)?.strip_prefix('_')?;
    let (name, rest) = rest.split_once('_')?;
    Some((name.to_string(), rest))
}

/// The name and location of frame `index`, for the tail-call record.
pub(super) fn frame_summary(vm: &VM, index: usize) -> super::FrameSummary {
    let function = &vm.frames[index].closure.function;
    let name = function_name(function.debug_info.as_ref(), index);
    let location = location_at(function, vm.frame_ip(index));
    (name, location)
}

fn function_name(info: Option<&FunctionDebugInfo>, index: usize) -> String {
    // The program's entry code, named as runtime stack traces name it.
    if index == 0 {
        return "<entry>".to_string();
    }
    match info.and_then(|info| info.name.as_deref()) {
        Some(name) => display_name(name),
        None => "<anonymous>".to_string(),
    }
}

fn location_at(function: &CompiledFunction, ip: usize) -> Option<(String, usize, usize)> {
    let info = function.debug_info.as_ref()?;
    let location = info.location_at(ip)?;
    let file = info.file_for(location.file_id)?.to_string();
    Some((
        file,
        location.span.start.line,
        location.span.start.column + 1,
    ))
}

/// Whether `file` (as the bytecode names it) is the file the user wrote
/// (`app.flx`, `src/app.flx` or an absolute path).
fn same_file(file: &str, wanted: &str) -> bool {
    FileMatcher::new(wanted).matches(file)
}

/// Matches the files the bytecode names against one the user wrote. Two
/// absolute paths can name one file through a symlink (`/var` is
/// `/private/var` on macOS), so those are compared resolved.
struct FileMatcher<'a> {
    wanted: &'a std::path::Path,
    resolved: Option<std::path::PathBuf>,
    seen: std::collections::HashMap<String, bool>,
}

impl<'a> FileMatcher<'a> {
    fn new(wanted: &'a str) -> Self {
        let wanted = std::path::Path::new(wanted);
        Self {
            wanted,
            resolved: std::fs::canonicalize(wanted).ok(),
            seen: std::collections::HashMap::new(),
        }
    }

    fn matches(&mut self, file: &str) -> bool {
        if let Some(known) = self.seen.get(file) {
            return *known;
        }
        let path = std::path::Path::new(file);
        let matched = path == self.wanted
            || path.ends_with(self.wanted)
            || self.wanted.ends_with(path)
            || self.resolved.as_ref().is_some_and(|resolved| {
                std::fs::canonicalize(path).ok().as_ref() == Some(resolved)
            });
        self.seen.insert(file.to_string(), matched);
        matched
    }
}

impl<'a> Inspector<'a> {
    pub(super) fn new(vm: &'a VM, control: &'a mut DebugControl) -> Self {
        Self { vm, control }
    }

    /// The call stack, newest first. Compiler-made frames are left out
    /// unless internals are shown.
    pub fn stack(&self) -> Vec<StackFrame> {
        (0..=self.vm.frame_index)
            .rev()
            .filter_map(|index| {
                let function = &self.vm.frames[index].closure.function;
                let info = function.debug_info.as_ref();
                let name = function_name(info, index);
                let artificial = is_artificial(info.and_then(|info| info.name.as_deref()));
                if artificial && !self.control.show_internals {
                    return None;
                }
                let (file, line, column) = location_at(function, self.vm.frame_ip(index))
                    .map(|(file, line, column)| (Some(file), line, column))
                    .unwrap_or((None, 0, 0));
                let elided = self
                    .control
                    .tail_calls
                    .get(index)
                    .filter(|calls| calls.count > 0)
                    .map(|calls| {
                        let last = calls
                            .recent
                            .last()
                            .map(|(name, location)| match location {
                                Some((file, line, column)) => {
                                    format!("{name} at {}:{line}:{column}", short_file(file))
                                }
                                None => name.clone(),
                            })
                            .unwrap_or_default();
                        (calls.count, last)
                    });
                Some(StackFrame {
                    index,
                    name,
                    file,
                    line,
                    column,
                    artificial,
                    elided,
                })
            })
            .collect()
    }

    /// The local variables of frame `index` that hold a value at its current
    /// instruction. A name bound twice shows its innermost binding.
    pub fn locals(&self, index: usize) -> Vec<Variable> {
        let Some(frame) = self.vm.frames.get(index) else {
            return Vec::new();
        };
        let Some(info) = frame.closure.function.debug_info.as_ref() else {
            return Vec::new();
        };
        let ip = self.vm.frame_ip(index);
        let mut live: Vec<&crate::bytecode::debug_info::LocalVar> = info
            .locals
            .iter()
            .filter(|local| local.is_live_at(ip))
            .filter(|local| self.control.show_internals || !local.artificial)
            .collect();
        // Innermost binding of each name: the one bound last.
        live.sort_by_key(|local| std::cmp::Reverse(local.start_ip));
        let mut seen = std::collections::HashSet::new();
        live.retain(|local| seen.insert(local.name.clone()));
        live.sort_by_key(|local| local.slot);
        live.into_iter()
            .filter_map(|local| {
                let value = self.slot_value(index, local.slot as usize)?;
                Some(Variable {
                    name: local.name.clone(),
                    value: ValueView::new(value),
                    artificial: local.artificial,
                })
            })
            .collect()
    }

    /// The values frame `index`'s closure captured.
    pub fn captured(&self, index: usize) -> Vec<Variable> {
        let Some(frame) = self.vm.frames.get(index) else {
            return Vec::new();
        };
        let Some(info) = frame.closure.function.debug_info.as_ref() else {
            return Vec::new();
        };
        info.free_names
            .iter()
            .zip(frame.closure.free.iter())
            .filter(|(name, _)| self.control.show_internals || !name.starts_with("__"))
            .map(|(name, value)| Variable {
                name: name.clone(),
                value: ValueView::new(value.clone()),
                artificial: name.starts_with("__"),
            })
            .collect()
    }

    /// The value of `name` in frame `index`: a local, else a captured value.
    pub fn lookup(&mut self, index: usize, name: &str) -> Option<ValueView> {
        let show = self.control.show_internals;
        self.control.show_internals = true;
        let found = self
            .locals(index)
            .into_iter()
            .chain(self.captured(index))
            .find(|variable| variable.name == name)
            .map(|variable| variable.value);
        self.control.show_internals = show;
        found
    }

    fn slot_value(&self, index: usize, slot_index: usize) -> Option<Value> {
        let frame = &self.vm.frames[index];
        let slot = self.vm.stack.get(frame.base_pointer + slot_index)?;
        let value = slot::from_slot_ref(slot);
        (!matches!(value, Value::Uninit)).then_some(value)
    }

    /// Sets a breakpoint at the first line at or after `line` in `file` that
    /// has code. Returns the line it was set on.
    pub fn set_breakpoint(&mut self, file: &str, line: usize) -> Result<usize, String> {
        let mut best: Option<(String, usize)> = None;
        let mut matcher = FileMatcher::new(file);
        for function in self.vm.all_functions() {
            let Some(info) = function.debug_info.as_ref() else {
                continue;
            };
            for site in &info.sites {
                let Some(location) = &site.location else {
                    continue;
                };
                let Some(site_file) = info.file_for(location.file_id) else {
                    continue;
                };
                let site_line = location.span.start.line;
                if site_line < line || !matcher.matches(site_file) {
                    continue;
                }
                if best
                    .as_ref()
                    .is_none_or(|(_, best_line)| site_line < *best_line)
                {
                    best = Some((site_file.to_string(), site_line));
                }
            }
        }
        let Some((file, line)) = best else {
            return Err(format!("no code at or after line {line} of {file}"));
        };
        self.control
            .breakpoints
            .insert(LineBreakpoint { file, line });
        Ok(line)
    }

    /// Clears the breakpoint on `line` of `file`. Returns whether there was
    /// one.
    pub fn clear_breakpoint(&mut self, file: &str, line: usize) -> bool {
        let before = self.control.breakpoints.len();
        self.control
            .breakpoints
            .retain(|bp| !(bp.line == line && same_file(&bp.file, file)));
        self.control.breakpoints.len() != before
    }

    /// Sites the program has run so far: where it is in its run.
    pub fn position(&self) -> u64 {
        self.control.position
    }

    /// Why the debugger cannot go back now, if it cannot.
    pub fn reverse_unavailable(&self) -> Option<String> {
        super::reverse_unavailable()
    }

    /// How many checkpoints the session holds.
    pub fn checkpoint_count(&self) -> usize {
        self.control.checkpoints.len()
    }

    /// Shows or hides compiler-made frames and variables.
    pub fn set_show_internals(&mut self, show: bool) {
        self.control.show_internals = show;
    }

    pub fn show_internals(&self) -> bool {
        self.control.show_internals
    }
}

/// The file name for a stack line: the last component.
pub(super) fn short_file(file: &str) -> String {
    std::path::Path::new(file)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string())
}

impl VM {
    /// Every compiled function the program can run: the entry code and each
    /// function in the constant pool.
    pub(super) fn all_functions(&self) -> Vec<Arc<CompiledFunction>> {
        let mut functions = vec![self.frames[0].closure.function.clone()];
        for constant in &self.constants {
            if let Value::Function(function) = slot::from_slot_ref(constant) {
                functions.push(function);
            }
        }
        functions
    }

    /// The instruction frame `index` is at: the current one for the newest
    /// frame, and the call that is running for the frames below it.
    pub(crate) fn frame_ip(&self, index: usize) -> usize {
        if index == self.frame_index {
            return self.frames[index].ip;
        }
        self.call_ips
            .get(index + 1)
            .copied()
            .unwrap_or_else(|| self.frames[index].ip.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::display_name;

    #[test]
    fn instance_methods_read_as_class_type_method() {
        assert_eq!(display_name("__tc_Show_Int_show"), "Show<Int>.show");
        assert_eq!(display_name("__tc_Eq_List<a>_neq"), "Eq<List<a>>.neq");
        assert_eq!(
            display_name("__tc_Show_Int_to_string"),
            "Show<Int>.to_string"
        );
        assert_eq!(
            display_name("__tc_m8_466C6F772E4E756D_Num_Float_sub"),
            "Num<Float>.sub"
        );
        assert_eq!(display_name("__dict_Show_Int"), "__dict_Show_Int");
        assert_eq!(display_name("main"), "main");
    }

    #[test]
    fn compiler_made_names_are_artificial() {
        use super::is_artificial;
        assert!(is_artificial(Some("__dict_Show_Int")));
        assert!(is_artificial(Some("Flow.Primops.__primop_print")));
        assert!(is_artificial(Some("__default_handler Console.print")));
        assert!(!is_artificial(Some("__tc_Show_Int_show")));
        assert!(!is_artificial(Some("<handler Console.print>")));
        assert!(!is_artificial(Some("main")));
    }

    #[test]
    fn breakpoint_files_match_by_name_or_through_a_symlink() {
        use super::same_file;
        assert!(same_file("/work/app/src/main.flx", "main.flx"));
        assert!(same_file("/work/app/src/main.flx", "src/main.flx"));
        assert!(same_file("src/main.flx", "/work/app/src/main.flx"));
        assert!(!same_file("/work/app/src/main.flx", "other.flx"));

        // An editor and the compiler can name one file by two absolute
        // paths, as `/var` and `/private/var` on macOS.
        #[cfg(unix)]
        {
            let dir = std::env::temp_dir().join(format!("flux-same-file-{}", std::process::id()));
            let real = dir.join("real");
            std::fs::create_dir_all(&real).unwrap();
            std::fs::write(real.join("app.flx"), "fn main() {}\n").unwrap();
            let link = dir.join("link");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let compiled = real.join("app.flx").display().to_string();
            let edited = link.join("app.flx").display().to_string();
            assert!(same_file(&compiled, &edited));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
