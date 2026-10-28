//! Builds a [`ProgramDebugInfo`] from one module's Core program.

use std::collections::HashMap;
use std::path::Path;

use crate::core::{
    CoreAlt, CoreBinder, CoreDef, CoreExpr, CorePat, CoreProgram, CoreTopLevelItem, CoreType,
};
use crate::diagnostics::position::Span;
use crate::syntax::interner::Interner;

use super::model::{
    DebugCtor, DebugEffect, DebugField, DebugFile, DebugFnId, DebugFunction, DebugType,
    DebugTypeId, DebugVar, FileId, FnKind, ProgramDebugInfo, VarKind,
};

/// Builds the debug information for one module.
///
/// `file` is the module's source path. Each module is compiled by its own
/// `Compiler`, so every function in `core` belongs to that file.
pub fn build_program_debug_info(
    core: &CoreProgram,
    file: &str,
    interner: &Interner,
) -> ProgramDebugInfo {
    let mut builder = Builder::new(file, interner);
    builder.collect_declarations(&core.top_level_items);
    for def in &core.defs {
        builder.add_def(def);
    }
    builder.info
}

struct Builder<'a> {
    interner: &'a Interner,
    info: ProgramDebugInfo,
    file: FileId,
    /// The file name used in lambda names (`<lambda app.flx:12:5>`).
    file_label: String,
    type_ids: HashMap<DebugType, DebugTypeId>,
    /// Constructors of the data types this module declares, by type name.
    data_ctors: HashMap<String, Vec<DebugCtor>>,
}

impl<'a> Builder<'a> {
    fn new(file: &str, interner: &'a Interner) -> Self {
        let file_label = Path::new(file)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.to_string());
        let mut info = ProgramDebugInfo::default();
        info.files.push(DebugFile { path: file.to_string() });
        Self {
            interner,
            info,
            file: FileId(0),
            file_label,
            type_ids: HashMap::new(),
            data_ctors: HashMap::new(),
        }
    }

    fn name(&self, sym: crate::syntax::Identifier) -> String {
        // Lowering and the Core passes name their temporaries with symbols
        // that were never interned (`Symbol::new(N_000_000 + fresh)` in
        // `core/lower_ast` and `core/passes`); name them as the compiler-made
        // values they are.
        match self.interner.try_resolve(sym) {
            Some(name) => name.to_string(),
            None => format!("__tmp_{}", sym.as_u32()),
        }
    }

    /// Records the module's data types and effects, including those declared
    /// inside `module` blocks.
    fn collect_declarations(&mut self, items: &[CoreTopLevelItem]) {
        for item in items {
            match item {
                CoreTopLevelItem::Data { name, variants, .. } => {
                    let ctors = variants
                        .iter()
                        .map(|variant| DebugCtor {
                            name: self.name(variant.name),
                            fields: variant
                                .fields
                                .iter()
                                .enumerate()
                                .map(|(index, ty)| DebugField {
                                    name: variant
                                        .field_names
                                        .as_ref()
                                        .map(|names| self.name(names[index])),
                                    ty: ty.display_with(self.interner),
                                })
                                .collect(),
                        })
                        .collect();
                    self.data_ctors.insert(self.name(*name), ctors);
                }
                CoreTopLevelItem::EffectDecl { name, ops, span } => {
                    let effect = DebugEffect {
                        name: self.name(*name),
                        ops: ops.iter().map(|op| self.name(op.name)).collect(),
                        span: *span,
                    };
                    self.info.effects.push(effect);
                }
                CoreTopLevelItem::Module { body, .. } => self.collect_declarations(body),
                _ => {}
            }
        }
    }

    fn add_def(&mut self, def: &CoreDef) {
        let source_name = self.name(def.name);
        let (kind, artificial) = if def.is_dict_def || is_dictionary(&source_name) {
            (FnKind::DictDef, true)
        } else if source_name.starts_with("__tc_") {
            (FnKind::DictMethod, true)
        } else {
            // A definition with no source position was made by the compiler
            // (a class method's dispatcher, for one), whatever its name.
            (FnKind::User, is_generated(&source_name) || is_unplaced(def.span))
        };
        let id = self.push_function(DebugFunction {
            id: DebugFnId(0),
            binder: Some(def.binder.id.0),
            source_name,
            file: self.file,
            decl_span: def.span,
            kind,
            artificial,
            vars: Vec::new(),
        });
        let ctx = DefCtx { def, function: id };
        // A definition whose body is a lambda is a function: the lambda's
        // parameters are the function's own, not a nested lambda's.
        match &def.expr {
            CoreExpr::Lam { params, param_types, body, span, .. } => {
                self.add_params(&ctx, params, param_types, *span);
                self.walk(&ctx, body);
            }
            expr => self.walk(&ctx, expr),
        }
    }

    fn push_function(&mut self, mut function: DebugFunction) -> DebugFnId {
        let id = DebugFnId(self.info.functions.len() as u32);
        function.id = id;
        self.info.functions.push(function);
        id
    }

    fn add_params(
        &mut self,
        ctx: &DefCtx,
        params: &[CoreBinder],
        param_types: &[Option<CoreType>],
        span: Span,
    ) {
        for (index, param) in params.iter().enumerate() {
            let ty = param_types.get(index).cloned().flatten();
            self.add_var(ctx, param, ty.as_ref(), span, VarKind::Param);
        }
    }

    fn add_var(
        &mut self,
        ctx: &DefCtx,
        binder: &CoreBinder,
        ty: Option<&CoreType>,
        decl_span: Span,
        kind: VarKind,
    ) {
        let name = self.name(binder.name);
        let ty = ty.or_else(|| ctx.def.binder_types.get(&binder.id));
        let ty = match ty {
            Some(ty) => self.type_id(ty),
            None => self.intern(DebugType::Dynamic),
        };
        let (kind, artificial) = if is_dictionary(&name) {
            (VarKind::Evidence, true)
        } else {
            (kind, is_generated(&name))
        };
        self.info.functions[ctx.function.0 as usize].vars.push(DebugVar {
            name,
            binder: binder.id.0,
            decl_span,
            ty,
            kind,
            artificial,
        });
    }

    fn walk(&mut self, ctx: &DefCtx, expr: &CoreExpr) {
        match expr {
            CoreExpr::Var { .. } | CoreExpr::Lit(..) => {}
            CoreExpr::Lam { params, param_types, body, span, .. } => {
                let parent = &self.info.functions[ctx.function.0 as usize];
                // A lambda with no source position was made by the compiler,
                // so it is named after the function it belongs to.
                let (source_name, artificial) = if is_unplaced(*span) {
                    (format!("<lambda in {}>", parent.source_name), true)
                } else {
                    (
                        format!(
                            "<lambda {}:{}:{}>",
                            self.file_label,
                            span.start.line,
                            span.start.column + 1
                        ),
                        parent.artificial,
                    )
                };
                let lambda = self.push_function(DebugFunction {
                    id: DebugFnId(0),
                    binder: None,
                    source_name,
                    file: self.file,
                    decl_span: *span,
                    kind: FnKind::Lambda { parent: ctx.function },
                    artificial,
                    vars: Vec::new(),
                });
                let inner = DefCtx { def: ctx.def, function: lambda };
                self.add_params(&inner, params, param_types, *span);
                self.walk(&inner, body);
            }
            CoreExpr::App { func, args, .. } => {
                self.walk(ctx, func);
                for arg in args {
                    self.walk(ctx, arg);
                }
            }
            CoreExpr::Let { var, rhs, body, span } | CoreExpr::LetRec { var, rhs, body, span } => {
                self.add_var(ctx, var, None, *span, VarKind::Let);
                self.walk(ctx, rhs);
                self.walk(ctx, body);
            }
            CoreExpr::LetRecGroup { bindings, body, span } => {
                for (var, _) in bindings {
                    self.add_var(ctx, var, None, *span, VarKind::Let);
                }
                for (_, rhs) in bindings {
                    self.walk(ctx, rhs);
                }
                self.walk(ctx, body);
            }
            CoreExpr::Case { scrutinee, alts, .. } => {
                self.walk(ctx, scrutinee);
                for alt in alts {
                    self.walk_alt(ctx, alt);
                }
            }
            CoreExpr::Con { fields, .. } => {
                for field in fields {
                    self.walk(ctx, field);
                }
            }
            CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
                for arg in args {
                    self.walk(ctx, arg);
                }
            }
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                self.walk(ctx, object);
            }
            CoreExpr::Return { value, .. } => self.walk(ctx, value),
            CoreExpr::Handle { body, parameter, handlers, .. } => {
                self.walk(ctx, body);
                if let Some(parameter) = parameter {
                    self.walk(ctx, parameter);
                }
                for handler in handlers {
                    for (index, param) in handler.params.iter().enumerate() {
                        let ty = handler.param_types.get(index).cloned().flatten();
                        self.add_var(ctx, param, ty.as_ref(), handler.span, VarKind::HandlerParam);
                    }
                    if let Some(state) = &handler.state {
                        self.add_var(
                            ctx,
                            state,
                            handler.state_ty.as_ref(),
                            handler.span,
                            VarKind::HandlerParam,
                        );
                    }
                    self.add_var(
                        ctx,
                        &handler.resume,
                        handler.resume_ty.as_ref(),
                        handler.span,
                        VarKind::HandlerParam,
                    );
                    self.walk(ctx, &handler.body);
                }
            }
        }
    }

    fn walk_alt(&mut self, ctx: &DefCtx, alt: &CoreAlt) {
        self.add_pattern_vars(ctx, &alt.pat, alt.span);
        if let Some(guard) = &alt.guard {
            self.walk(ctx, guard);
        }
        self.walk(ctx, &alt.rhs);
    }

    fn add_pattern_vars(&mut self, ctx: &DefCtx, pat: &CorePat, span: Span) {
        match pat {
            CorePat::Var(binder) => self.add_var(ctx, binder, None, span, VarKind::PatternBound),
            CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
                for field in fields {
                    self.add_pattern_vars(ctx, field, span);
                }
            }
            CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
        }
    }

    fn intern(&mut self, ty: DebugType) -> DebugTypeId {
        if let Some(id) = self.type_ids.get(&ty) {
            return *id;
        }
        let id = DebugTypeId(self.info.types.len() as u32);
        self.info.types.push(ty.clone());
        self.type_ids.insert(ty, id);
        id
    }

    fn type_id(&mut self, ty: &CoreType) -> DebugTypeId {
        let debug_ty = match ty {
            CoreType::Int => DebugType::Int,
            CoreType::Float => DebugType::Float,
            CoreType::Bool => DebugType::Bool,
            CoreType::String => DebugType::String,
            CoreType::Unit => DebugType::Unit,
            CoreType::Never => DebugType::Never,
            CoreType::List(elem) => DebugType::List(self.type_id(elem)),
            CoreType::Array(elem) => DebugType::Array(self.type_id(elem)),
            CoreType::Option(inner) => DebugType::Option(self.type_id(inner)),
            CoreType::Either(left, right) => {
                DebugType::Either(self.type_id(left), self.type_id(right))
            }
            CoreType::Map(key, value) => DebugType::Map(self.type_id(key), self.type_id(value)),
            CoreType::Tuple(elems) => {
                DebugType::Tuple(elems.iter().map(|elem| self.type_id(elem)).collect())
            }
            CoreType::Function(params, result) => DebugType::Function(
                params.iter().map(|param| self.type_id(param)).collect(),
                self.type_id(result),
            ),
            CoreType::Adt(name, args) => self.adt(*name, args),
            CoreType::Abstract(crate::core::CoreAbstractType::Named(name, args)) => {
                self.adt(*name, args)
            }
            CoreType::Forall(_, body) => return self.type_id(body),
            CoreType::Var(_) | CoreType::Abstract(_) => DebugType::Dynamic,
        };
        self.intern(debug_ty)
    }

    fn adt(&mut self, name: crate::syntax::Identifier, args: &[CoreType]) -> DebugType {
        let name = self.name(name);
        let args = args.iter().map(|arg| self.type_id(arg)).collect();
        // A qualified name (`Shapes.Shape`) finds a type declared in a
        // `module` block under its short name.
        let short = name.rsplit('.').next().unwrap_or(&name);
        let ctors = self
            .data_ctors
            .get(&name)
            .or_else(|| self.data_ctors.get(short))
            .cloned()
            .unwrap_or_default();
        DebugType::Adt { name, args, ctors }
    }
}

/// The definition being walked and the function its variables belong to.
struct DefCtx<'d> {
    def: &'d CoreDef,
    function: DebugFnId,
}

fn is_dictionary(name: &str) -> bool {
    name.starts_with("__dict")
}

/// Names the compiler makes up start with `__`; user names cannot.
fn is_generated(name: &str) -> bool {
    name.starts_with("__")
}

/// Code the compiler synthesizes carries the default span: lines are 1-based,
/// so line 0 is no position at all.
fn is_unplaced(span: Span) -> bool {
    span.start.line == 0
}
