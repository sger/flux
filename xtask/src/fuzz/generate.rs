//! A type-directed generator of well-typed Flux programs (0190 §6).
//!
//! It keeps an environment of typed variables and functions, and builds an
//! expression of a requested type within a depth budget, preferring
//! variables already in scope so that values flow from one binding to the
//! next. `main` prints every value it binds, so a wrong value shows up in
//! stdout and not only in the program's final result.
//!
//! What it leaves out, and why, comes from the open known issues a program
//! would otherwise trip over on every run:
//! - helpers are pure, since a lambda can't call an effectful function
//!   (KI-029), and only `main` prints;
//! - every `*` is reduced `% 9973` and literals stay below 100, so no value
//!   gets near the backends' different Int widths (KI-038, KI-039);
//! - only `Int` and `String` are ordered, and only scalars compared, since
//!   ordering anything else fails at run time (KI-157) and `Eq` on tuples
//!   doesn't resolve (KI-162);
//! - functions come before their callers (KI-117), and a recursive call's
//!   arguments are variables (KI-155);
//! - no Floats, whose printed form differs between the backends.
//!
//! Layout is one statement per line and one `match` arm per line, so the
//! reducer (§7) can work line by line.
//!
//! Parts of the language beyond that subset are turned on one at a time
//! (0190 S17), and every random choice a part makes is behind its flag, so a
//! seed's programs without the part stay as they were. `classes` (S17a) adds
//! a generic data type, user classes with instances (one contextual, on the
//! generic type), generic and constrained helpers, and `deriving (Eq, Show)`.
//! Its open KIs: an instance method never calls its own method at its own type
//! (KI-104); a constrained function is never passed as a value (KI-090) and is
//! fully annotated (KI-101); classes take one parameter (KI-075); nothing is
//! inside a `module` (KI-076, KI-086), and no name is a prelude method's.
//!
//! `values` (S17b) adds named-field records, closures as values (bound,
//! passed, returned from helpers, stored in lists and record fields and called
//! later), Arrays, Maps and `Either`. Its open KIs: a record's name has a
//! lowercase letter (KI-180), and no record is built inside an `if`
//! condition or a `match` scrutinee (KI-181); a Map's key is an `Int` or a
//! `String` (KI-157); Arrays and Maps are used through their aliases, with
//! exact arity (KI-146), and an index or `get` gives an `Option`, never
//! `array_get` (KI-147); a function value is called with exactly its arity
//! (KI-107), and no generic, constrained or recursive helper is one (KI-090).
//! A value holding a function is never printed whole, since a closure's
//! printed form is the backend's own; `main` prints what it gives instead.
//! Nor are a Map's `keys` or `values`, whose order is the hash's.
//!
//! `effects` (S17c) adds one or two user effects, helpers that declare and
//! perform them, and handlers around a call of one, in helpers and in `main`,
//! nested when the callee has two effects. A clause resumes once, gives a
//! value without resuming, or does one or the other on a condition; it
//! resumes with its parameter and a capture combined in an order that shows.
//! Its open KIs: an effect has one operation (KI-185) of one parameter
//! (KI-184); its name is never a builtin's (KI-114, KI-102); a lambda never
//! performs or calls an effectful helper (KI-029); a handled expression is a
//! call, bound to a `let` (KI-025). Left out: multi-shot resume, handlers with
//! state, and a clause that prints.
//!
//! `async` (S17d) adds helpers with `Async` in their row: `both` over two of
//! them, tasks spawned and then awaited in a fixed order, a channel sent to
//! and drained within one fiber, and one a producer fills while a consumer
//! drains it, under `both`. One body calls them all, and `main` runs it with
//! the program's only `run_async` (KI-169) and prints what it gives. Only
//! `main` prints, so nothing a fiber does on another worker prints (KI-144);
//! what fibers send and give is combined in an order that shows, so the output
//! doesn't depend on scheduling but on that order. Calls go through the
//! modules' aliases, which check their arity (KI-146). Left out: anything
//! timed (`sleep`, `timeout`, `race`, `select`), `yield_now`, worker counts,
//! and a user effect inside an async helper.

use crate::rng::Rng;
use crate::sweep::fnv64;

/// The parts of the language a program may use beyond S8's subset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    /// Generics and type classes (S17a).
    pub classes: bool,
    /// Records, closures as values, Arrays, Maps and Either (S17b).
    pub values: bool,
    /// User effects, the helpers that perform them, and handlers (S17c).
    pub effects: bool,
    /// Async helpers, tasks and channels, run by one `run_async` (S17d).
    pub async_: bool,
}

impl Parts {
    pub const NONE: Parts = Parts { classes: false, values: false, effects: false, async_: false };

    /// `--with`'s names for the parts that are on, comma-separated.
    pub fn names(&self) -> String {
        [
            (self.classes, "classes"),
            (self.values, "values"),
            (self.effects, "effects"),
            (self.async_, "async"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect::<Vec<_>>()
        .join(",")
    }

    /// Reads `--with`'s value.
    pub fn parse(names: &str) -> Result<Parts, String> {
        let mut parts = Parts::NONE;
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            match name {
                "classes" => parts.classes = true,
                "values" => parts.values = true,
                "effects" => parts.effects = true,
                "async" => parts.async_ = true,
                other => {
                    return Err(format!(
                        "unknown part `{other}`; the parts are: classes, values, effects, async"
                    ));
                }
            }
        }
        Ok(parts)
    }
}

/// A generated program's types. `Pair` and `List` hold scalars only, and
/// `Opt` a scalar or a data type, so nesting stays two deep.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ty {
    Int,
    Bool,
    Str,
    Opt(Box<Ty>),
    List(Box<Ty>),
    Pair(Box<Ty>, Box<Ty>),
    Data(usize),
    /// `T`: the type parameter of the generic helper, class or contextual
    /// instance being written. There is only ever one in scope.
    Var,
    /// The program's generic data type applied to a scalar, a data type or `T`.
    GData(Box<Ty>),
    /// A named-field record type (S17b): one constructor, fields by name.
    Rec(usize),
    /// A function of one or two scalars to a scalar.
    Fun(Vec<Ty>, Box<Ty>),
    /// `Array<scalar>`.
    Arr(Box<Ty>),
    /// `Map<Int or String, scalar>` (KI-157: no other key type).
    Map(Box<Ty>, Box<Ty>),
    /// `Either<scalar, scalar>`.
    Either(Box<Ty>, Box<Ty>),
}

/// A record type: `data Rec<n> { Rec<n> { f<n>: T, ... } }`. Its name has a
/// lowercase letter, which named-field construction needs (KI-180).
struct RecDef {
    name: String,
    fields: Vec<(String, Ty)>,
}

struct Ctor {
    name: String,
    fields: Vec<Ty>,
}

struct DataDef {
    name: String,
    ctors: Vec<Ctor>,
    derives_eq: bool,
    derives_show: bool,
}

struct FnSig {
    name: String,
    params: Vec<Ty>,
    ret: Ty,
    /// Its first parameter is a countdown; callers bound it with `% 20`.
    recursive: bool,
    /// `Some` for a generic helper, with the class constraining its `T`.
    tparam: Option<Option<usize>>,
    /// The effects in its row (S17c), by index in `Gen::effects`.
    effects: Vec<usize>,
}

/// `effect Eff<n> { op<n> : (P) -> R }`: one operation of one parameter.
/// Natively, a second operation runs the first one's clause (KI-185), and a
/// second parameter is dropped (KI-184).
struct EffectDef {
    name: String,
    op: String,
    param: Ty,
    ret: Ty,
}

struct Method {
    name: String,
    /// `(x: T, y: T)` rather than `(x: T)`.
    binary: bool,
    ret: Ty,
    /// Its body is in the class, so instances don't give one.
    default: bool,
}

struct ClassDef {
    name: String,
    methods: Vec<Method>,
    /// The types with an instance, other than the generic type's.
    instances: Vec<Ty>,
    /// Whether `instance C<T> => C<G<T>>` exists.
    contextual: bool,
}

/// Which class methods an expression may call.
#[derive(Clone, Copy, PartialEq)]
enum Methods {
    /// Any, at any type with an instance.
    Any,
    /// None: the body of a plain instance (KI-104).
    No,
    /// Only the non-default methods of one class, on `T`: a default body or a
    /// contextual instance, whose `T` has that class's dictionary.
    OwnOnT(usize),
}

/// Limits that keep every program small and its run short.
const MAX_DEPTH: u32 = 3;
const MAX_HELPERS: usize = 4;
const MAX_MAIN_BINDINGS: usize = 10;
const MAX_LETS_PER_BLOCK: usize = 2;
const STRINGS: [&str; 5] = ["", "a", "bc", "xyz", "Flux"];
/// The type parameter's name. Lowercase: an uppercase one works on a
/// function but not in a generic type's `deriving` or an instance's context.
const TVAR: &str = "a";

/// The program for `case` of `seed`. A case doesn't depend on how many
/// others are generated, so `--count` can change without renaming any.
#[cfg(test)]
pub fn program(seed: u64, case: usize) -> String {
    program_with(seed, case, &Parts::NONE)
}

/// The program for `case` of `seed` with `parts` on.
pub fn program_with(seed: u64, case: usize, parts: &Parts) -> String {
    let mut rng = Rng::new(seed ^ fnv64(format!("fuzz:{case}").as_bytes()));
    let mut g = Gen::new(&mut rng, *parts);
    g.data_types();
    if parts.values {
        g.records();
    }
    if parts.effects {
        g.declare_effects();
    }
    if parts.classes {
        g.classes();
    }
    g.helpers();
    if parts.async_ {
        g.async_helpers();
    }
    g.main();
    let with =
        if *parts == Parts::NONE { String::new() } else { format!(" --with {}", parts.names()) };
    let mut out = format!(
        "// expect: any\n// Generated by cargo xtask fuzz --seed {seed}{with}, case {case}.\n\n"
    );
    if parts.values {
        out.push_str("import Flow.Array as Array\nimport Flow.Map as Map\n\n");
    }
    if parts.async_ {
        out.push_str(
            "import Flow.Async as Async\nimport Flow.Task as Task\nimport Flow.Channel as Channel\n\n",
        );
    }
    for line in &g.lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

struct Gen<'r> {
    rng: &'r mut Rng,
    parts: Parts,
    data: Vec<DataDef>,
    /// The generic data type, `G<T>`: its name and constructors, whose
    /// fields may be `Ty::Var`.
    gdata: Option<DataDef>,
    recs: Vec<RecDef>,
    /// The function types drawn so far, so that one recurs: a helper's
    /// result type is then asked for again, and its closure called.
    fun_tys: Vec<Ty>,
    /// The helpers that return a closure over their arguments, by index in
    /// `fns`.
    closure_helpers: Vec<usize>,
    /// The helpers that handle an effectful helper's effects with a clause
    /// over their own parameters, by index in `fns`.
    handler_helpers: Vec<usize>,
    effects: Vec<EffectDef>,
    /// The effects the function being written may perform.
    row: Vec<usize>,
    /// While a lambda's body is written: it can't perform or call an
    /// effectful function (KI-029).
    in_lambda: bool,
    /// While an async helper is written: no handler, so no user effect, in
    /// it (S17d leaves the two apart).
    in_async: bool,
    /// The function `main` runs with `run_async`, once the async helpers are
    /// written.
    async_body: Option<String>,
    classes: Vec<ClassDef>,
    fns: Vec<FnSig>,
    vars: Vec<(String, Ty)>,
    /// `Some` while `T` is in scope, with the class whose dictionary it has.
    tvar: Option<Option<usize>>,
    methods: Methods,
    /// While an `if` condition or a `match` scrutinee is written: a record
    /// construction there doesn't parse, even inside a call (KI-181).
    no_struct: bool,
    next: usize,
    lines: Vec<String>,
}

impl<'r> Gen<'r> {
    fn new(rng: &'r mut Rng, parts: Parts) -> Self {
        Gen {
            rng,
            parts,
            data: Vec::new(),
            gdata: None,
            recs: Vec::new(),
            fun_tys: Vec::new(),
            closure_helpers: Vec::new(),
            handler_helpers: Vec::new(),
            effects: Vec::new(),
            row: Vec::new(),
            in_lambda: false,
            in_async: false,
            async_body: None,
            classes: Vec::new(),
            fns: Vec::new(),
            vars: Vec::new(),
            tvar: None,
            methods: Methods::Any,
            no_struct: false,
            next: 0,
            lines: Vec::new(),
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.rng.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.rng.below(items.len())]
    }

    // ---- types ----

    fn scalar(&mut self) -> Ty {
        match self.rng.below(5) {
            0 | 1 => Ty::Int,
            2 => Ty::Bool,
            _ => Ty::Str,
        }
    }

    fn any_ty(&mut self) -> Ty {
        if self.parts.values && self.chance(30) {
            return self.value_ty();
        }
        if self.parts.classes && self.gdata.is_some() && self.chance(12) {
            let arg = if !self.data.is_empty() && self.chance(25) {
                Ty::Data(self.rng.below(self.data.len()))
            } else {
                self.scalar()
            };
            return Ty::GData(Box::new(arg));
        }
        match self.rng.below(10) {
            0..=3 => self.scalar(),
            4 => {
                let inner = if !self.data.is_empty() && self.chance(30) {
                    Ty::Data(self.rng.below(self.data.len()))
                } else {
                    self.scalar()
                };
                Ty::Opt(Box::new(inner))
            }
            5 | 6 => Ty::List(Box::new(self.scalar())),
            7 => Ty::Pair(Box::new(self.scalar()), Box::new(self.scalar())),
            _ if !self.data.is_empty() => Ty::Data(self.rng.below(self.data.len())),
            _ => self.scalar(),
        }
    }

    fn render(&self, ty: &Ty) -> String {
        match ty {
            Ty::Int => "Int".into(),
            Ty::Bool => "Bool".into(),
            Ty::Str => "String".into(),
            Ty::Opt(t) => format!("Option<{}>", self.render(t)),
            Ty::List(t) => format!("List<{}>", self.render(t)),
            Ty::Pair(a, b) => format!("({}, {})", self.render(a), self.render(b)),
            Ty::Data(i) => self.data[*i].name.clone(),
            Ty::Var => TVAR.into(),
            Ty::GData(t) => format!("{}<{}>", self.gdata_def().name, self.render(t)),
            Ty::Rec(i) => self.recs[*i].name.clone(),
            Ty::Fun(params, ret) => {
                let shown: Vec<String> = params.iter().map(|p| self.render(p)).collect();
                format!("({}) -> {}", shown.join(", "), self.render(ret))
            }
            Ty::Arr(t) => format!("Array<{}>", self.render(t)),
            Ty::Map(k, v) => format!("Map<{}, {}>", self.render(k), self.render(v)),
            Ty::Either(a, b) => format!("Either<{}, {}>", self.render(a), self.render(b)),
        }
    }

    fn gdata_def(&self) -> &DataDef {
        self.gdata.as_ref().expect("a GData type needs the generic data type")
    }

    /// The generic type's constructors with `T` replaced by `arg`.
    fn gdata_ctors(&self, arg: &Ty) -> Vec<(String, Vec<Ty>)> {
        self.gdata_def()
            .ctors
            .iter()
            .map(|c| (c.name.clone(), c.fields.iter().map(|f| subst(f, arg)).collect()))
            .collect()
    }

    // ---- top level ----

    fn data_types(&mut self) {
        for _ in 0..2 + self.rng.below(2) {
            let name = self.fresh("D");
            let mut ctors = Vec::new();
            for _ in 0..1 + self.rng.below(3) {
                let ctor = self.fresh("C");
                let fields =
                    (0..self.rng.below(3))
                        .map(|_| {
                            if self.chance(20) { Ty::Opt(Box::new(Ty::Int)) } else { self.scalar() }
                        })
                        .collect();
                ctors.push(Ctor { name: ctor, fields });
            }
            let (derives_eq, derives_show) = if self.parts.classes {
                (self.chance(40), self.chance(30))
            } else {
                (false, false)
            };
            let def = DataDef { name, ctors, derives_eq, derives_show };
            self.lines.push(self.data_line(&def, ""));
            self.data.push(def);
        }
        if self.parts.classes && self.chance(70) {
            self.generic_data();
        }
        self.lines.push(String::new());
    }

    /// `data <name><params> { <ctors> } deriving (...)`.
    fn data_line(&self, def: &DataDef, params: &str) -> String {
        let shown: Vec<String> = def
            .ctors
            .iter()
            .map(|c| {
                if c.fields.is_empty() {
                    c.name.clone()
                } else {
                    let fields: Vec<String> = c.fields.iter().map(|f| self.render(f)).collect();
                    format!("{}({})", c.name, fields.join(", "))
                }
            })
            .collect();
        let derived: Vec<&str> = [(def.derives_eq, "Eq"), (def.derives_show, "Show")]
            .into_iter()
            .filter_map(|(on, class)| on.then_some(class))
            .collect();
        let deriving = if derived.is_empty() {
            String::new()
        } else {
            format!(" deriving ({})", derived.join(", "))
        };
        format!("data {}{params} {{ {} }}{deriving}", def.name, shown.join(", "))
    }

    /// `data G<T> { ... }`: two or three constructors whose fields are `T` or
    /// a scalar, at least one of them holding a `T`.
    fn generic_data(&mut self) {
        let name = self.fresh("G");
        let mut ctors = Vec::new();
        for i in 0..2 + self.rng.below(2) {
            let ctor = self.fresh("C");
            let count = if i == 0 { 1 + self.rng.below(2) } else { self.rng.below(3) };
            let fields = (0..count)
                .map(
                    |j| if (i == 0 && j == 0) || self.chance(50) { Ty::Var } else { self.scalar() },
                )
                .collect();
            ctors.push(Ctor { name: ctor, fields });
        }
        let def = DataDef { name, ctors, derives_eq: self.chance(40), derives_show: false };
        self.lines.push(self.data_line(&def, &format!("<{TVAR}>")));
        self.gdata = Some(def);
    }

    // ---- classes and instances (S17a) ----

    /// One or two classes, each followed by its instances. An instance's
    /// methods come before every helper, so they call none, and a plain
    /// instance calls no class method at all (KI-104).
    fn classes(&mut self) {
        for _ in 0..1 + self.rng.below(2) {
            let c = self.class_decl();
            self.instances(c);
        }
    }

    fn class_decl(&mut self) -> usize {
        let name = self.fresh("Cls");
        let mut methods = Vec::new();
        let count = 1 + self.rng.below(3);
        let shared = self.scalar();
        for i in 0..count {
            let binary = self.chance(20);
            // Two methods of one type sometimes, so that a dictionary whose
            // slots were swapped would still type-check and run.
            let ret = if binary {
                Ty::Bool
            } else if i > 0 && self.chance(50) {
                shared.clone()
            } else {
                self.scalar()
            };
            let default = i > 0 && self.chance(25);
            methods.push(Method { name: self.fresh("m"), binary, ret, default });
        }
        if methods.iter().all(|m| m.default) {
            methods[0].default = false;
        }
        let c = self.classes.len();
        self.classes.push(ClassDef {
            name: name.clone(),
            methods,
            instances: Vec::new(),
            contextual: false,
        });
        self.lines.push(format!("class {name}<{TVAR}> {{"));
        for m in 0..self.classes[c].methods.len() {
            let (mname, binary, ret, default) = {
                let m = &self.classes[c].methods[m];
                (m.name.clone(), m.binary, m.ret.clone(), m.default)
            };
            let params =
                if binary { format!("x: {TVAR}, y: {TVAR}") } else { format!("x: {TVAR}") };
            let sig = format!("    fn {mname}({params}) -> {}", self.render(&ret));
            if !default {
                self.lines.push(sig);
                continue;
            }
            self.lines.push(format!("{sig} {{"));
            let binders = method_binders(binary, Ty::Var);
            self.in_scope(binders, Some(Some(c)), Methods::OwnOnT(c), |g| {
                g.block(&ret, MAX_DEPTH - 1, 2)
            });
            self.lines.push("    }".into());
        }
        self.lines.push("}".into());
        self.lines.push(String::new());
        c
    }

    /// Instances for some of Int, Bool, String and the data types, and
    /// sometimes the contextual one for the generic type.
    fn instances(&mut self, c: usize) {
        let mut tys = vec![Ty::Int, Ty::Bool, Ty::Str];
        tys.extend((0..self.data.len()).map(Ty::Data));
        let mut chosen: Vec<Ty> = tys.into_iter().filter(|_| self.chance(50)).collect();
        if chosen.is_empty() {
            chosen.push(Ty::Int);
        }
        for ty in &chosen {
            let head = format!("instance {}<{}> {{", self.classes[c].name, self.render(ty));
            self.instance_body(c, head, ty.clone(), None, Methods::No);
        }
        self.classes[c].instances = chosen;
        if self.gdata.is_some() && self.chance(50) {
            let class = self.classes[c].name.clone();
            let g = self.gdata_def().name.clone();
            let head = format!("instance {class}<{TVAR}> => {class}<{g}<{TVAR}>> {{");
            let on = Ty::GData(Box::new(Ty::Var));
            self.instance_body(c, head, on, Some(Some(c)), Methods::OwnOnT(c));
            self.classes[c].contextual = true;
        }
    }

    /// An instance's non-default methods. A contextual instance's method
    /// matches on `x` and can call the class's methods on a `T` field, which
    /// go through the context's dictionary, not this instance's.
    fn instance_body(
        &mut self,
        c: usize,
        head: String,
        on: Ty,
        tvar: Option<Option<usize>>,
        methods: Methods,
    ) {
        self.lines.push(head);
        let todo: Vec<(String, bool, Ty)> = self.classes[c]
            .methods
            .iter()
            .filter(|m| !m.default)
            .map(|m| (m.name.clone(), m.binary, m.ret.clone()))
            .collect();
        for (name, binary, ret) in todo {
            let params = if binary { "x, y" } else { "x" };
            self.lines.push(format!("    fn {name}({params}) {{"));
            let binders = method_binders(binary, on.clone());
            self.in_scope(binders, tvar, methods, |g| {
                if matches!(on, Ty::GData(_)) {
                    let head = "        ".to_string();
                    g.matching_on("x".into(), &on, &ret, MAX_DEPTH, 2, head);
                } else {
                    g.block(&ret, MAX_DEPTH - 1, 2);
                }
            });
            self.lines.push("    }".into());
        }
        self.lines.push("}".into());
        self.lines.push(String::new());
    }

    /// Runs `f` with `binders` in scope, `T` as `tvar` and `methods` callable.
    fn in_scope(
        &mut self,
        binders: Vec<(String, Ty)>,
        tvar: Option<Option<usize>>,
        methods: Methods,
        f: impl FnOnce(&mut Self),
    ) {
        let (scope, saved_tvar, saved_methods) = (self.vars.len(), self.tvar, self.methods);
        self.vars.extend(binders);
        self.tvar = tvar;
        self.methods = methods;
        f(self);
        self.vars.truncate(scope);
        self.tvar = saved_tvar;
        self.methods = saved_methods;
    }

    fn helpers(&mut self) {
        let count = 1 + self.rng.below(MAX_HELPERS);
        let recursive_at = self.rng.below(count + 1); // == count: none
        for i in 0..count {
            if i == recursive_at {
                self.recursive_helper();
            } else if self.parts.classes && self.chance(50) {
                self.generic_helper();
            } else if self.parts.values && self.chance(25) {
                self.closure_helper();
            } else if self.parts.effects && self.chance(40) {
                self.effect_helper(false);
            } else {
                self.plain_helper();
            }
            self.lines.push(String::new());
        }
        if self.parts.effects {
            self.effect_helper(true);
            self.lines.push(String::new());
            self.handler_helper(self.fns.len() - 1);
            self.lines.push(String::new());
        }
    }

    /// `fn f<T>(x: T, ...) -> R`, or `fn f<T: C>(...)` whose value calls one
    /// of `C`'s methods on `x` through the dictionary it is passed. Fully
    /// annotated (KI-101).
    fn generic_helper(&mut self) {
        let name = self.fresh("f");
        let constraint = if !self.classes.is_empty() && self.chance(60) {
            Some(self.rng.below(self.classes.len()))
        } else {
            None
        };
        let mut params = vec![(self.fresh("a"), Ty::Var)];
        for _ in 0..self.rng.below(3) {
            let ty = if self.chance(30) { Ty::Var } else { self.any_ty() };
            params.push((self.fresh("a"), ty));
        }
        let ret = match constraint {
            Some(c) => {
                let m = self.rng.below(self.classes[c].methods.len());
                self.classes[c].methods[m].ret.clone()
            }
            None => match self.rng.below(6) {
                0 | 1 => Ty::Var,
                2 => Ty::Opt(Box::new(Ty::Var)),
                3 => Ty::List(Box::new(Ty::Var)),
                4 if self.gdata.is_some() => Ty::GData(Box::new(Ty::Var)),
                _ => self.scalar(),
            },
        };
        let bound = match constraint {
            Some(c) => format!("<{TVAR}: {}>", self.classes[c].name),
            None => format!("<{TVAR}>"),
        };
        let sig = self.signature(&name, &params, &ret).replacen('(', &format!("{bound}("), 1);
        self.lines.push(sig);
        let x = params[0].0.clone();
        self.in_scope(params.clone(), Some(constraint), Methods::Any, |g| match constraint {
            Some(c) => g.constrained_body(c, &x, &ret),
            None => g.block(&ret, MAX_DEPTH, 1),
        });
        self.lines.push("}".into());
        let params = params.into_iter().map(|(_, t)| t).collect();
        self.fns.push(FnSig {
            name,
            params,
            ret,
            recursive: false,
            tparam: Some(constraint),
            effects: Vec::new(),
        });
    }

    /// Lets, then a call of one of class `c`'s methods returning `ret` on `x`,
    /// so the helper always uses the dictionary it takes.
    fn constrained_body(&mut self, c: usize, x: &str, ret: &Ty) {
        let scope = self.vars.len();
        for _ in 0..self.rng.below(MAX_LETS_PER_BLOCK + 1) {
            let ty = self.any_ty();
            let name = self.fresh("v");
            self.binding(&name, &ty, MAX_DEPTH - 1, 1);
            self.vars.push((name, ty));
        }
        let fitting: Vec<(String, bool)> = self.classes[c]
            .methods
            .iter()
            .filter(|m| m.ret == *ret)
            .map(|m| (m.name.clone(), m.binary))
            .collect();
        let (method, binary) = self.pick(&fitting).clone();
        let args = if binary {
            let y = self.var_of(&Ty::Var).expect("x is a T");
            format!("{x}, {y}")
        } else {
            x.to_string()
        };
        self.lines.push(format!("    {method}({args})"));
        self.vars.truncate(scope);
    }

    fn params(&mut self, count: usize) -> Vec<(String, Ty)> {
        (0..count).map(|_| (self.fresh("a"), self.any_ty())).collect()
    }

    fn signature(&self, name: &str, params: &[(String, Ty)], ret: &Ty) -> String {
        let shown: Vec<String> =
            params.iter().map(|(n, t)| format!("{n}: {}", self.render(t))).collect();
        format!("fn {name}({}) -> {} {{", shown.join(", "), self.render(ret))
    }

    fn plain_helper(&mut self) {
        let name = self.fresh("f");
        let count = 1 + self.rng.below(3);
        let params = self.params(count);
        let ret = self.any_ty();
        self.lines.push(self.signature(&name, &params, &ret));
        let scope = self.vars.len();
        self.vars.extend(params.iter().cloned());
        self.block(&ret, MAX_DEPTH, 1);
        self.vars.truncate(scope);
        self.lines.push("}".into());
        let params = params.into_iter().map(|(_, t)| t).collect();
        self.fns.push(FnSig {
            name,
            params,
            ret,
            recursive: false,
            tparam: None,
            effects: Vec::new(),
        });
    }

    /// `if n <= 0 { base } else { ... f(n - k, ...) ... }`, with one
    /// recursive call whose other arguments are variables.
    fn recursive_helper(&mut self) {
        let name = self.fresh("f");
        let n = self.fresh("n");
        let mut params = vec![(n.clone(), Ty::Int)];
        let count = self.rng.below(3);
        params.extend(self.params(count));
        let ret = self.any_ty();
        self.lines.push(self.signature(&name, &params, &ret));
        let scope = self.vars.len();
        self.vars.extend(params.iter().cloned());
        self.lines.push(format!("    if ({n} <= 0) {{"));
        self.block(&ret, MAX_DEPTH - 1, 2);
        self.lines.push("    } else {".into());
        let step = 1 + self.rng.below(3);
        let args: Vec<String> = std::iter::once(format!("({n} - {step})"))
            .chain(
                params[1..].iter().map(|(_, t)| self.var_of(t).expect("a parameter has this type")),
            )
            .collect();
        let call = format!("{name}({})", args.join(", "));
        if self.chance(40) {
            self.lines.push(format!("        {call}"));
        } else {
            let result = self.fresh("v");
            self.lines.push(format!("        let {result}: {} = {call};", self.render(&ret)));
            self.vars.push((result, ret.clone()));
            self.block(&ret, MAX_DEPTH - 1, 2);
        }
        self.lines.push("    }".into());
        self.vars.truncate(scope);
        self.lines.push("}".into());
        let params = params.into_iter().map(|(_, t)| t).collect();
        self.fns.push(FnSig {
            name,
            params,
            ret,
            recursive: true,
            tparam: None,
            effects: Vec::new(),
        });
    }

    fn main(&mut self) {
        self.lines.push("fn main() with IO {".into());
        if let Some(body) = self.async_body.clone() {
            let name = self.fresh("v");
            self.lines.push(format!("    let {name}: String = Async.run_async({body});"));
            self.lines.push(format!("    print({name});"));
            self.vars.push((name, Ty::Str));
        }
        // Half the closures that helpers return are bound here and called when
        // printed: built in one function, run in another.
        for f in self.closure_helpers.clone() {
            if self.chance(50) {
                let (callee, params, ty) =
                    (self.fns[f].name.clone(), self.fns[f].params.clone(), self.fns[f].ret.clone());
                let args: Vec<String> = params.iter().map(|p| self.expr(p, 1)).collect();
                let name = self.fresh("v");
                self.lines.push(format!(
                    "    let {name}: {} = {callee}({});",
                    self.render(&ty),
                    args.join(", ")
                ));
                for shown in self.observe(&name, &ty) {
                    self.lines.push(format!("    print({shown});"));
                }
                self.vars.push((name, ty));
            }
        }
        // Each handler helper is called, so its clause runs.
        for f in self.handler_helpers.clone() {
            let (callee, params, ty) =
                (self.fns[f].name.clone(), self.fns[f].params.clone(), self.fns[f].ret.clone());
            let args: Vec<String> = params.iter().map(|p| self.expr(p, 1)).collect();
            let name = self.fresh("v");
            self.lines.push(format!(
                "    let {name}: {} = {callee}({});",
                self.render(&ty),
                args.join(", ")
            ));
            for shown in self.observe(&name, &ty) {
                self.lines.push(format!("    print({shown});"));
            }
            self.vars.push((name, ty));
        }
        for _ in 0..3 + self.rng.below(MAX_MAIN_BINDINGS - 2) {
            let ty = self.any_ty();
            let name = self.fresh("v");
            self.binding(&name, &ty, MAX_DEPTH, 1);
            for shown in self.observe(&name, &ty) {
                self.lines.push(format!("    print({shown});"));
            }
            self.vars.push((name, ty));
        }
        self.lines.push("}".into());
    }

    // ---- statements ----

    /// `let <name>: <ty> = <value>;` at `indent`, possibly over several lines.
    fn binding(&mut self, name: &str, ty: &Ty, depth: u32, indent: usize) {
        let pad = "    ".repeat(indent);
        if self.parts.effects
            && !self.in_async
            && self.chance(35)
            && let Some(handled) = self.handled(ty, depth)
        {
            self.lines.push(format!("{pad}let {name}: {} = {handled};", self.render(ty)));
            return;
        }
        let head = format!("{pad}let {name}: {} = ", self.render(ty));
        self.value(ty, depth, indent, head);
        let last = self.lines.last_mut().expect("value wrote a line");
        last.push(';');
    }

    /// Lets, then a value of `ty`, each on its own lines at `indent`.
    fn block(&mut self, ty: &Ty, depth: u32, indent: usize) {
        let scope = self.vars.len();
        for _ in 0..self.rng.below(MAX_LETS_PER_BLOCK + 1) {
            let let_ty = self.any_ty();
            let name = self.fresh("v");
            self.binding(&name, &let_ty, depth.saturating_sub(1), indent);
            self.vars.push((name, let_ty));
        }
        self.value(ty, depth, indent, "    ".repeat(indent));
        self.vars.truncate(scope);
    }

    /// A value of `ty` whose first line starts with `head`: an `if`, a
    /// `match` or an inline expression.
    fn value(&mut self, ty: &Ty, depth: u32, indent: usize, head: String) {
        let pad = "    ".repeat(indent);
        let roll = if depth == 0 { 99 } else { self.rng.below(100) };
        if roll < 15 {
            let cond = self.without_struct(|g| g.expr(&Ty::Bool, depth - 1));
            self.lines.push(format!("{head}if {cond} {{"));
            self.block(ty, depth - 1, indent + 1);
            self.lines.push(format!("{pad}}} else {{"));
            self.block(ty, depth - 1, indent + 1);
            self.lines.push(format!("{pad}}}"));
        } else if roll < 35 {
            self.matching(ty, depth, indent, head);
        } else {
            let e = self.expr(ty, depth);
            self.lines.push(format!("{head}{e}"));
        }
    }

    /// `match <scrutinee> { <arm>, ... }`, exhaustive by construction: one arm
    /// per constructor, or literal arms and a final wildcard.
    fn matching(&mut self, ty: &Ty, depth: u32, indent: usize, head: String) {
        let mut on = self.any_ty();
        while !self.matchable(&on) {
            on = self.any_ty();
        }
        let scrutinee = if holds_rec(&on) {
            self.var_of(&on).expect("matchable: a variable holds it")
        } else {
            self.without_struct(|g| g.expr(&on, depth - 1))
        };
        self.matching_on(scrutinee, &on, ty, depth, indent, head);
    }

    /// `match <scrutinee> { ... }` on a value of `on`, each arm a `ty`.
    fn matching_on(
        &mut self,
        scrutinee: String,
        on: &Ty,
        ty: &Ty,
        depth: u32,
        indent: usize,
        head: String,
    ) {
        let pad = "    ".repeat(indent);
        let on = on.clone();
        self.lines.push(format!("{head}match {scrutinee} {{"));
        let arm_pad = "    ".repeat(indent + 1);
        let mut arms: Vec<(String, Vec<(String, Ty)>)> = Vec::new();
        match &on {
            Ty::Bool => {
                arms.push(("true".into(), vec![]));
                arms.push(("false".into(), vec![]));
            }
            Ty::Int | Ty::Str => {
                for _ in 0..1 + self.rng.below(2) {
                    let lit = self.literal(&on);
                    if !arms.iter().any(|(p, _)| *p == lit) {
                        arms.push((lit, vec![]));
                    }
                }
                arms.push(("_".into(), vec![]));
            }
            Ty::Opt(t) => {
                let p = self.fresh("p");
                arms.push((format!("Some({p})"), vec![(p, (**t).clone())]));
                arms.push(("None".into(), vec![]));
            }
            Ty::List(t) => {
                let (h, rest) = (self.fresh("p"), self.fresh("p"));
                arms.push((
                    format!("[{h} | {rest}]"),
                    vec![(h, (**t).clone()), (rest, on.clone())],
                ));
                arms.push(("[]".into(), vec![]));
            }
            Ty::Pair(a, b) => {
                let (x, y) = (self.fresh("p"), self.fresh("p"));
                arms.push((format!("({x}, {y})"), vec![(x, (**a).clone()), (y, (**b).clone())]));
            }
            Ty::Var => arms.push(("_".into(), vec![])),
            Ty::Rec(i) => {
                let (name, fields) = (self.recs[*i].name.clone(), self.recs[*i].fields.clone());
                let binders: Vec<(String, Ty)> =
                    fields.iter().map(|(_, t)| (self.fresh("p"), t.clone())).collect();
                let shown: Vec<String> = fields
                    .iter()
                    .zip(&binders)
                    .map(|((f, _), (p, _))| format!("{f}: {p}"))
                    .collect();
                arms.push((format!("{name} {{ {} }}", shown.join(", ")), binders));
            }
            Ty::Either(a, b) => {
                let (l, r) = (self.fresh("p"), self.fresh("p"));
                arms.push((format!("Left({l})"), vec![(l, (**a).clone())]));
                arms.push((format!("Right({r})"), vec![(r, (**b).clone())]));
            }
            Ty::Fun(..) | Ty::Arr(_) | Ty::Map(..) => unreachable!("not matchable"),
            Ty::Data(_) | Ty::GData(_) => {
                let ctors: Vec<(String, Vec<Ty>)> = match &on {
                    Ty::Data(i) => self.data[*i]
                        .ctors
                        .iter()
                        .map(|c| (c.name.clone(), c.fields.clone()))
                        .collect(),
                    Ty::GData(arg) => self.gdata_ctors(arg),
                    _ => unreachable!(),
                };
                for (name, fields) in ctors {
                    if fields.is_empty() {
                        arms.push((name, vec![]));
                    } else {
                        let binders: Vec<(String, Ty)> =
                            fields.into_iter().map(|f| (self.fresh("p"), f)).collect();
                        let shown: Vec<&str> = binders.iter().map(|(n, _)| n.as_str()).collect();
                        arms.push((format!("{name}({})", shown.join(", ")), binders));
                    }
                }
                // Or the later arms folded into a final wildcard.
                if arms.len() > 1 && self.chance(25) {
                    arms.truncate(1 + self.rng.below(arms.len() - 1));
                    arms.push(("_".into(), vec![]));
                }
            }
        }
        for (pattern, binders) in arms {
            let scope = self.vars.len();
            self.vars.extend(binders);
            let body = self.expr(ty, depth - 1);
            self.vars.truncate(scope);
            self.lines.push(format!("{arm_pad}{pattern} -> {body},"));
        }
        self.lines.push(format!("{pad}}}"));
    }

    // ---- expressions on one line ----

    fn var_of(&mut self, ty: &Ty) -> Option<String> {
        let found: Vec<String> =
            self.vars.iter().filter(|(_, t)| t == ty).map(|(n, _)| n.clone()).collect();
        if found.is_empty() { None } else { Some(self.pick(&found).clone()) }
    }

    fn literal(&mut self, ty: &Ty) -> String {
        match ty {
            Ty::Int => self.rng.below(100).to_string(),
            Ty::Bool => (if self.chance(50) { "true" } else { "false" }).into(),
            Ty::Str => format!("\"{}\"", self.pick(&STRINGS)),
            _ => self.leaf(ty),
        }
    }

    /// A variable of `ty` if there is one, else the smallest value of it.
    fn leaf(&mut self, ty: &Ty) -> String {
        if let Some(v) = self.var_of(ty) {
            return v;
        }
        match ty {
            Ty::Int | Ty::Bool | Ty::Str => self.literal(ty),
            Ty::Opt(_) => "None".into(),
            Ty::List(_) => "[]".into(),
            Ty::Pair(a, b) => format!("({}, {})", self.leaf(a), self.leaf(b)),
            Ty::Data(i) => {
                let i = *i;
                let fewest = (0..self.data[i].ctors.len())
                    .min_by_key(|&c| self.data[i].ctors[c].fields.len())
                    .expect("a data type has a constructor");
                let fields = self.data[i].ctors[fewest].fields.clone();
                let name = self.data[i].ctors[fewest].name.clone();
                self.applied(&name, &fields, 0)
            }
            Ty::GData(arg) => {
                let ctors = self.gdata_ctors(arg);
                let (name, fields) =
                    ctors.into_iter().min_by_key(|(_, f)| f.len()).expect("a constructor");
                self.applied(&name, &fields, 0)
            }
            // Every scope `T` is in has a variable of it: the helper's or
            // the method's `x`.
            Ty::Var => panic!("no variable of type T in scope"),
            Ty::Rec(i) => self.construct(*i, 0),
            Ty::Fun(params, ret) => self.typed_lambda(params, ret, 0),
            Ty::Arr(t) => format!("[|{}|]", self.leaf(t)),
            Ty::Map(k, v) => format!("({{{}: {}}})", self.literal(k), self.leaf(v)),
            Ty::Either(a, _) => format!("Left({})", self.leaf(a)),
        }
    }

    fn applied(&mut self, name: &str, fields: &[Ty], depth: u32) -> String {
        if fields.is_empty() {
            return name.to_string();
        }
        let args: Vec<String> = fields.iter().map(|f| self.expr(f, depth)).collect();
        format!("{name}({})", args.join(", "))
    }

    /// A list of `elem` that is never the empty literal, for the places
    /// where nothing else fixes the element type.
    fn list_source(&mut self, elem: &Ty, depth: u32) -> String {
        let list = Ty::List(Box::new(elem.clone()));
        if let Some(v) = self.var_of(&list)
            && self.chance(70)
        {
            return v;
        }
        let items: Vec<String> =
            (0..1 + self.rng.below(3)).map(|_| self.expr(elem, depth.saturating_sub(1))).collect();
        format!("[{}]", items.join(", "))
    }

    /// `\x -> body` with `x: param` in scope for the body.
    fn lambda(&mut self, params: &[Ty], body: &Ty, depth: u32) -> String {
        let in_lambda = std::mem::replace(&mut self.in_lambda, true);
        let scope = self.vars.len();
        let names: Vec<String> = params
            .iter()
            .map(|t| {
                let n = self.fresh("x");
                self.vars.push((n.clone(), t.clone()));
                n
            })
            .collect();
        let e = self.expr(body, depth);
        self.vars.truncate(scope);
        self.in_lambda = in_lambda;
        if self.parts.values {
            // Annotated, so a parameter's field or call is typed where it's
            // read, whatever the call around the lambda infers first.
            let shown: Vec<String> =
                names.iter().zip(params).map(|(n, t)| format!("{n}: {}", self.render(t))).collect();
            format!("\\({}) -> {e}", shown.join(", "))
        } else if names.len() == 1 {
            format!("\\{} -> {e}", names[0])
        } else {
            format!("\\({}) -> {e}", names.join(", "))
        }
    }

    fn call(&mut self, ty: &Ty, depth: u32) -> Option<String> {
        let found: Vec<usize> = (0..self.fns.len())
            .filter(|&i| !(self.no_struct && self.fns[i].params.iter().any(holds_rec)))
            .filter(|&i| {
                let effects = &self.fns[i].effects;
                effects.is_empty()
                    || (!self.in_lambda && effects.iter().all(|e| self.row.contains(e)))
            })
            .filter(|&i| {
                let f = &self.fns[i];
                match f.tparam {
                    None => f.ret == *ty,
                    Some(_) => self.generic_fit(i, ty).is_some(),
                }
            })
            .collect();
        if found.is_empty() {
            return None;
        }
        let f = *self.pick(&found);
        let (name, mut params, recursive) =
            (self.fns[f].name.clone(), self.fns[f].params.clone(), self.fns[f].recursive);
        if self.fns[f].tparam.is_some() {
            let arg = match self.generic_fit(f, ty).expect("it matched above") {
                Some(t) => t,
                None => {
                    let constraint = self.fns[f].tparam.flatten();
                    let candidates: Vec<Ty> = self
                        .simple_types()
                        .into_iter()
                        .filter(|t| self.meets(constraint, t))
                        .collect();
                    self.pick(&candidates).clone()
                }
            };
            params = params.iter().map(|p| subst(p, &arg)).collect();
        }
        let args: Vec<String> = params
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let arg = self.expr(t, depth);
                if recursive && i == 0 { format!("({arg} % 20)") } else { arg }
            })
            .collect();
        Some(format!("{name}({})", args.join(", ")))
    }

    /// Whether generic helper `f` can be called where a `want` is needed:
    /// `Some(Some(t))` when its return type fixes `T` as `t`, `Some(None)`
    /// when `T` is free and some type meets its constraint.
    fn generic_fit(&self, f: usize, want: &Ty) -> Option<Option<Ty>> {
        let constraint = self.fns[f].tparam.flatten();
        let mut bound = None;
        if !unify(&self.fns[f].ret, want, &mut bound) {
            return None;
        }
        match bound {
            Some(t) => self.meets(constraint, &t).then_some(Some(t)),
            None => self.simple_types().iter().any(|t| self.meets(constraint, t)).then_some(None),
        }
    }

    /// The types a `T` may stand for: scalars, the data types, and `T`
    /// itself where one is in scope.
    fn simple_types(&self) -> Vec<Ty> {
        let mut tys = vec![Ty::Int, Ty::Bool, Ty::Str];
        tys.extend((0..self.data.len()).map(Ty::Data));
        if self.tvar.is_some() {
            tys.push(Ty::Var);
        }
        tys
    }

    /// Whether `ty` meets `constraint`: an instance of the class exists for it.
    fn meets(&self, constraint: Option<usize>, ty: &Ty) -> bool {
        match constraint {
            None => true,
            Some(c) => self.has_instance(c, ty),
        }
    }

    fn has_instance(&self, c: usize, ty: &Ty) -> bool {
        match ty {
            Ty::Var => self.tvar == Some(Some(c)),
            Ty::GData(arg) => self.classes[c].contextual && self.has_instance(c, arg),
            _ => self.classes[c].instances.contains(ty),
        }
    }

    /// With classes on: a class method's result, `==` on data deriving `Eq`,
    /// or `show` on data deriving `Show`.
    fn class_expr(&mut self, ty: &Ty, d: u32) -> Option<String> {
        match ty {
            Ty::Bool if self.chance(30) => {
                let eq: Vec<Ty> = (0..self.data.len())
                    .filter(|&i| self.data[i].derives_eq)
                    .map(Ty::Data)
                    .collect();
                if eq.is_empty() {
                    return self.method_call(ty, d);
                }
                let on = self.pick(&eq).clone();
                let op = *self.pick(&["==", "!="]);
                Some(format!("({} {op} {})", self.expr(&on, d), self.expr(&on, d)))
            }
            Ty::Str if self.chance(30) => {
                let shown: Vec<usize> =
                    (0..self.data.len()).filter(|&i| self.data[i].derives_show).collect();
                if shown.is_empty() {
                    return self.method_call(ty, d);
                }
                let on = Ty::Data(*self.pick(&shown));
                Some(format!("show({})", self.expr(&on, d)))
            }
            _ => self.method_call(ty, d),
        }
    }

    /// A call of a class method returning `ty`, on a type with an instance.
    fn method_call(&mut self, ty: &Ty, d: u32) -> Option<String> {
        let mut options: Vec<(String, bool, Ty)> = Vec::new();
        for c in 0..self.classes.len() {
            let receivers: Vec<Ty> = match self.methods {
                Methods::No => continue,
                Methods::OwnOnT(own) if own != c => continue,
                Methods::OwnOnT(_) => {
                    if self.vars.iter().any(|(_, t)| *t == Ty::Var) {
                        vec![Ty::Var]
                    } else {
                        continue;
                    }
                }
                Methods::Any => {
                    let mut tys = self.simple_types();
                    // A value of the generic type only as an annotated
                    // variable: `m(C10)` with a nullary constructor leaves
                    // its parameter, and so the instance, undetermined.
                    tys.extend(
                        self.vars
                            .iter()
                            .filter(|(_, t)| matches!(t, Ty::GData(arg) if **arg != Ty::Var))
                            .map(|(_, t)| t.clone()),
                    );
                    tys.dedup();
                    tys.into_iter().filter(|t| self.has_instance(c, t)).collect()
                }
            };
            for m in &self.classes[c].methods {
                if m.ret != *ty || (matches!(self.methods, Methods::OwnOnT(_)) && m.default) {
                    continue;
                }
                for r in &receivers {
                    options.push((m.name.clone(), m.binary, r.clone()));
                }
            }
        }
        if options.is_empty() {
            return None;
        }
        let (name, binary, on) = self.pick(&options).clone();
        let receiver = |g: &mut Self| {
            if matches!(on, Ty::GData(_)) { g.leaf(&on) } else { g.expr(&on, d) }
        };
        let x = receiver(self);
        if binary {
            let y = receiver(self);
            Some(format!("{name}({x}, {y})"))
        } else {
            Some(format!("{name}({x})"))
        }
    }

    fn expr(&mut self, ty: &Ty, depth: u32) -> String {
        if depth == 0 || self.chance(25) {
            return self.leaf(ty);
        }
        let d = depth - 1;
        if self.chance(12)
            && let Some(call) = self.call(ty, d)
        {
            return call;
        }
        if self.parts.classes
            && self.chance(15)
            && let Some(e) = self.class_expr(ty, d)
        {
            return e;
        }
        if self.parts.values
            && self.chance(20)
            && let Some(e) = self.values_expr(ty, d)
        {
            return e;
        }
        if self.parts.effects
            && !self.row.is_empty()
            && !self.in_lambda
            && self.chance(12)
            && let Some(e) = self.perform_expr(ty, d)
        {
            return e;
        }
        match ty {
            Ty::Int => self.int_expr(d),
            Ty::Bool => self.bool_expr(d),
            Ty::Str => self.str_expr(d),
            Ty::Opt(t) => {
                if self.chance(70) {
                    format!("Some({})", self.expr(t, d))
                } else {
                    "None".into()
                }
            }
            Ty::List(t) => self.list_expr(t, d),
            Ty::Pair(a, b) => format!("({}, {})", self.expr(a, d), self.expr(b, d)),
            Ty::Data(i) => {
                let i = *i;
                let c = self.rng.below(self.data[i].ctors.len());
                let (name, fields) =
                    (self.data[i].ctors[c].name.clone(), self.data[i].ctors[c].fields.clone());
                self.applied(&name, &fields, d)
            }
            Ty::GData(arg) => {
                let ctors = self.gdata_ctors(arg);
                let (name, fields) = self.pick(&ctors).clone();
                self.applied(&name, &fields, d)
            }
            // Nothing makes a `T` but the variables that hold one.
            Ty::Var => self.leaf(ty),
            Ty::Rec(i) => match self.var_of(ty) {
                Some(v) if self.chance(40) => self.spread(*i, &v, d),
                _ => self.construct(*i, d),
            },
            Ty::Fun(params, ret) => self.fun_expr(params, ret, ty, d),
            Ty::Arr(t) => self.arr_expr(t, d),
            Ty::Map(k, v) => self.map_expr(k, v, d),
            Ty::Either(a, b) => {
                if self.chance(50) {
                    format!("Left({})", self.expr(a, d))
                } else {
                    format!("Right({})", self.expr(b, d))
                }
            }
        }
    }

    // ---- S17c: user effects and handlers ----

    /// One or two effects of one operation each.
    fn declare_effects(&mut self) {
        for _ in 0..1 + self.rng.below(2) {
            let name = self.fresh("Eff");
            let op = self.fresh("op");
            let ret = self.scalar();
            // Mostly of its result's type, so a clause can resume with its
            // parameter and a capture combined.
            // The first always is, for the handler helper.
            let param = if self.effects.is_empty() || self.chance(70) {
                ret.clone()
            } else {
                self.scalar()
            };
            self.lines.push(format!("effect {name} {{"));
            self.lines.push(format!(
                "    {op} : ({}) -> {}",
                self.render(&param),
                self.render(&ret)
            ));
            self.lines.push("}".into());
            self.effects.push(EffectDef { name, op, param, ret });
        }
        self.lines.push(String::new());
    }

    /// `fn f(..) -> R with Eff1[, Eff2] { let v = perform Eff1.op(..); ... }`:
    /// it performs each effect of its row before anything else.
    ///
    /// When `resumed`, its row is the first effect, whose operation takes
    /// and gives an `R`, and it gives what it performed combined with
    /// another value so that a different resumed value gives a different
    /// result: a value resumed and then dropped can't show how the clause
    /// computed it.
    fn effect_helper(&mut self, resumed: bool) {
        let name = self.fresh("f");
        let count = 1 + self.rng.below(2);
        let params = self.params(count);
        let ret = if resumed {
            self.effects[0].ret.clone()
        } else {
            // Not a function type: `-> (Int) -> R with E` gives the row to
            // the function returned, not to the helper.
            loop {
                let ty = self.any_ty();
                if !matches!(ty, Ty::Fun(..)) {
                    break ty;
                }
            }
        };
        let effects = if resumed {
            vec![0]
        } else if self.effects.len() > 1 && self.chance(40) {
            vec![0, 1]
        } else {
            vec![self.rng.below(self.effects.len())]
        };
        let row: Vec<String> = effects.iter().map(|&e| self.effects[e].name.clone()).collect();
        let sig = self.signature(&name, &params, &ret);
        let sig = format!("{} with {} {{", sig.trim_end_matches(" {"), row.join(", "));
        self.lines.push(sig);
        let scope = self.vars.len();
        self.vars.extend(params.iter().cloned());
        let outer_row = std::mem::replace(&mut self.row, effects.clone());
        for &e in &effects {
            let (effect, op, param, ty) = {
                let d = &self.effects[e];
                (d.name.clone(), d.op.clone(), d.param.clone(), d.ret.clone())
            };
            let arg = self.expr(&param, MAX_DEPTH - 1);
            let v = self.fresh("v");
            self.lines
                .push(format!("    let {v}: {} = perform {effect}.{op}({arg});", self.render(&ty)));
            self.vars.push((v, ty));
        }
        if resumed {
            // `e` doesn't read `v`, so that it can't cancel it: `(v - v)`.
            let (v, _) = self.vars.pop().expect("it performed");
            let e = self.expr(&ret, MAX_DEPTH - 1);
            self.lines.push(match ret {
                Ty::Int => format!("    ({v} - {e})"),
                Ty::Str => format!("    ({v} + {e})"),
                _ => format!("    ({v} != {e})"),
            });
        } else {
            self.block(&ret, MAX_DEPTH, 1);
        }
        self.row = outer_row;
        self.vars.truncate(scope);
        self.lines.push("}".into());
        let params = params.into_iter().map(|(_, t)| t).collect();
        self.fns.push(FnSig { name, params, ret, recursive: false, tparam: None, effects });
    }

    /// `perform E.op(e)` for an effect in the row whose operation gives `ty`.
    fn perform_expr(&mut self, ty: &Ty, d: u32) -> Option<String> {
        let fitting: Vec<usize> =
            self.row.iter().copied().filter(|&e| self.effects[e].ret == *ty).collect();
        if fitting.is_empty() {
            return None;
        }
        let e = *self.pick(&fitting);
        let (effect, op, param) = (
            self.effects[e].name.clone(),
            self.effects[e].op.clone(),
            self.effects[e].param.clone(),
        );
        Some(format!("perform {effect}.{op}({})", self.expr(&param, d)))
    }

    /// `g(args) handle E1 { ... }`, or `do { g(args) handle E1 {..} } handle
    /// E2 {..}`, for an effectful helper `g` returning `ty`: every effect of
    /// its row handled, around a direct call (KI-025).
    fn handled(&mut self, ty: &Ty, depth: u32) -> Option<String> {
        let found: Vec<usize> = (0..self.fns.len())
            .filter(|&i| !self.fns[i].effects.is_empty() && self.fns[i].ret == *ty)
            .collect();
        if found.is_empty() {
            return None;
        }
        let f = *self.pick(&found);
        let (name, params, effects) =
            (self.fns[f].name.clone(), self.fns[f].params.clone(), self.fns[f].effects.clone());
        let args: Vec<String> =
            params.iter().map(|p| self.expr(p, depth.saturating_sub(1))).collect();
        let mut text = format!("{name}({})", args.join(", "));
        for (k, &e) in effects.iter().enumerate() {
            let clause = self.clause(e, ty);
            let effect = self.effects[e].name.clone();
            text = if k == 0 {
                format!("{text} handle {effect} {{ {clause} }}")
            } else {
                format!("do {{ {text} }} handle {effect} {{ {clause} }}")
            };
        }
        Some(text)
    }

    /// A clause for effect `e` whose `handle` gives `ty`: it resumes once,
    /// gives a `ty` without resuming, or does one or the other on a
    /// condition. It never performs, nor prints. When its parameter has the
    /// operation's result type it resumes with the parameter and a variable
    /// in scope, a capture, combined so that their order shows.
    fn clause(&mut self, e: usize, ty: &Ty) -> String {
        let (op, param, ret) = (
            self.effects[e].op.clone(),
            self.effects[e].param.clone(),
            self.effects[e].ret.clone(),
        );
        let p = self.fresh("p");
        let outer_row = std::mem::take(&mut self.row);
        let resumed = |g: &mut Self| -> String {
            let capture: Vec<String> = g
                .vars
                .iter()
                .filter(|(n, t)| *t == ret && *n != p)
                .map(|(n, _)| n.clone())
                .collect();
            if param == ret && !capture.is_empty() {
                let k = g.pick(&capture).clone();
                g.resume_with(&p, &k, &ret)
            } else {
                format!("resume({})", g.expr(&ret, 1))
            }
        };
        let scope = self.vars.len();
        let body = match self.rng.below(10) {
            0..=5 => {
                let body = resumed(self);
                self.vars.push((p.clone(), param.clone()));
                body
            }
            _ => {
                self.vars.push((p.clone(), param.clone()));
                let abort = self.expr(ty, 1);
                if self.chance(50) {
                    let cond = self.without_struct(|g| g.expr(&Ty::Bool, 1));
                    let again = resumed(self);
                    format!("if {cond} {{ {again} }} else {{ {abort} }}")
                } else {
                    abort
                }
            }
        };
        self.vars.truncate(scope);
        self.row = outer_row;
        format!("{op}(resume, {p}) -> {body}")
    }

    /// `resume(((p - k) + e))`: the clause's parameter `p` and a capture `k`
    /// combined so that their order shows (`+` on Strings, `p && !k` on
    /// Bools), as in a closure helper.
    fn resume_with(&mut self, p: &str, k: &str, ret: &Ty) -> String {
        let e = self.expr(ret, 1);
        match ret {
            Ty::Int => format!("resume((({p} - {k}) + {e}))"),
            Ty::Str => format!("resume((({p} + {k}) + {e}))"),
            _ => format!("resume((({p} && !{k}) != {e}))"),
        }
    }

    /// `fn h(a: R, ..) -> R { let v: R = g(..) handle E { op(resume, p) ->
    /// resume(((p - a) + e)) }; v }`: a handler around `g`, an effectful
    /// helper written `resumed`, whose clause resumes with its parameter and
    /// the helper's own parameter `a`. `main` calls it, so the clause runs,
    /// and `a` is known only at run time: a constant capture is folded into
    /// the clause, and then binding the parameter and the capture the wrong
    /// way round would print the same.
    fn handler_helper(&mut self, g: usize) {
        let (callee, gparams, ret) =
            (self.fns[g].name.clone(), self.fns[g].params.clone(), self.fns[g].ret.clone());
        let e = self.fns[g].effects[0];
        let (effect, op) = (self.effects[e].name.clone(), self.effects[e].op.clone());
        let name = self.fresh("f");
        let a = (self.fresh("a"), ret.clone());
        let count = self.rng.below(2);
        let extra = self.params(count);
        let params: Vec<(String, Ty)> = std::iter::once(a.clone()).chain(extra.clone()).collect();
        self.lines.push(self.signature(&name, &params, &ret));
        let scope = self.vars.len();
        // The call reads only the other parameters, and the clause only `a`:
        // a clause that reads a value the call takes over stops the compiler
        // (KI-186).
        self.vars.extend(extra);
        let args: Vec<String> = gparams.iter().map(|p| self.expr(p, MAX_DEPTH - 1)).collect();
        self.vars.truncate(scope);
        // Nor does the rest of the resumed value read `a`, which could cancel
        // the swap: `((p - a) + (a + a))` is `((a - p) + (p + p))`.
        let p = self.fresh("p");
        let body = self.resume_with(&p, &a.0, &ret);
        let text = format!(
            "{callee}({}) handle {effect} {{ {op}(resume, {p}) -> {body} }}",
            args.join(", ")
        );
        let v = self.fresh("v");
        self.lines.push(format!("    let {v}: {} = {text};", self.render(&ret)));
        self.lines.push(format!("    {v}"));
        self.vars.truncate(scope);
        self.lines.push("}".into());
        let params = params.into_iter().map(|(_, t)| t).collect();
        self.handler_helpers.push(self.fns.len());
        self.fns.push(FnSig {
            name,
            params,
            ret,
            recursive: false,
            tparam: None,
            effects: Vec::new(),
        });
    }

    // ---- S17d: async ----

    /// `a` and `b` combined so that their order shows: `-` on Ints, `+` on
    /// Strings, `a && !b` on Bools.
    fn ordered(a: &str, b: &str, ty: &Ty) -> String {
        match ty {
            Ty::Int => format!("({a} - {b})"),
            Ty::Str => format!("({a} + {b})"),
            _ => format!("({a} && !{b})"),
        }
    }

    fn none_value(ty: &Ty) -> &'static str {
        match ty {
            Ty::Int => "0",
            Ty::Str => "\"\"",
            _ => "false",
        }
    }

    /// `fn f(params) -> R with Async {`.
    fn async_signature(&self, name: &str, params: &[(String, Ty)], ret: &Ty) -> String {
        let sig = self.signature(name, params, ret);
        format!("{} with Async {{", sig.trim_end_matches(" {"))
    }

    /// The async helpers, and the body that calls each of them, which `main`
    /// runs. Each form has a helper of its own and the body always calls it,
    /// so every program runs every form.
    fn async_helpers(&mut self) {
        let in_async = std::mem::replace(&mut self.in_async, true);
        let mut calls: Vec<(String, Ty)> = Vec::new();
        let (left, right) = (self.async_value_helper(), self.async_value_helper());
        let pair = Ty::Pair(Box::new(left.1.clone()), Box::new(right.1.clone()));
        let both = format!("Async.both({}, {})", left.0, right.0);
        for helper in [self.task_helper(), self.channel_helper(), self.pipe_helper()] {
            calls.push(helper);
        }
        let name = self.fresh("f");
        self.lines.push(self.async_signature(&name, &[], &Ty::Str));
        let p = self.fresh("v");
        self.lines.push(format!("    let {p}: {} = {both};", self.render(&pair)));
        let mut shown = vec![format!("{p}.0"), format!("{p}.1")];
        for (callee, ty) in calls {
            let v = self.fresh("v");
            let arg = self.expr(&ty, 1);
            self.lines.push(format!("    let {v}: {} = {callee}({arg});", self.render(&ty)));
            shown.push(v);
        }
        let text: Vec<String> = shown.iter().map(|v| format!("#{{{v}}}")).collect();
        self.lines.push(format!("    \"{}\"", text.join("|")));
        self.lines.push("}".into());
        self.lines.push(String::new());
        self.async_body = Some(name);
        self.in_async = in_async;
    }

    /// `fn f() -> S with Async { ... }`: no parameters, so `both` takes it by
    /// name rather than in a lambda (KI-029).
    fn async_value_helper(&mut self) -> (String, Ty) {
        let name = self.fresh("f");
        let ty = self.scalar();
        self.lines.push(self.async_signature(&name, &[], &ty));
        self.block(&ty, MAX_DEPTH, 1);
        self.lines.push("}".into());
        self.lines.push(String::new());
        (name, ty)
    }

    /// `fn f(b: S) -> S with Async`: two tasks spawned over pure values of
    /// `b`, then awaited in order, their results combined in that order. A
    /// task's body sees only `b`, a Sendable scalar.
    fn task_helper(&mut self) -> (String, Ty) {
        let name = self.fresh("f");
        let ty = self.scalar();
        let b = self.fresh("a");
        self.lines.push(self.async_signature(&name, &[(b.clone(), ty.clone())], &ty));
        let scope = self.vars.len();
        self.vars.push((b, ty.clone()));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let t = self.fresh("v");
            let in_lambda = std::mem::replace(&mut self.in_lambda, true);
            let e = self.expr(&ty, MAX_DEPTH - 1);
            self.in_lambda = in_lambda;
            self.lines.push(format!(
                "    let {t}: Task<{}> = Task.spawn(fn() {{ {e} }});",
                self.render(&ty)
            ));
            tasks.push(t);
        }
        let mut got = Vec::new();
        for t in tasks {
            let v = self.fresh("v");
            self.lines.push(format!("    let {v}: {} = Task.await({t});", self.render(&ty)));
            got.push(v);
        }
        self.lines.push(format!("    {}", Self::ordered(&got[0], &got[1], &ty)));
        self.vars.truncate(scope);
        self.lines.push("}".into());
        self.lines.push(String::new());
        (name, ty)
    }

    /// `fn f(b: S) -> S with Async`: a channel filled with two or three
    /// values, closed, and drained in one fiber, what it gives combined in
    /// the order received. Its capacity holds every value, so no send waits.
    fn channel_helper(&mut self) -> (String, Ty) {
        let name = self.fresh("f");
        let ty = self.scalar();
        let b = self.fresh("a");
        self.lines.push(self.async_signature(&name, &[(b.clone(), ty.clone())], &ty));
        let scope = self.vars.len();
        self.vars.push((b, ty.clone()));
        let count = 2 + self.rng.below(2);
        let cap = count + self.rng.below(2);
        let ch = self.fresh("v");
        let shown = self.render(&ty);
        self.lines.push(format!("    let {ch}: Channel<{shown}> = Channel.make({cap});"));
        for _ in 0..count {
            let e = self.expr(&ty, MAX_DEPTH - 1);
            self.lines.push(format!("    Channel.send({ch}, {e});"));
        }
        self.lines.push(format!("    Channel.close({ch});"));
        let mut got: Vec<String> = Vec::new();
        for _ in 0..count {
            let (v, x) = (self.fresh("v"), self.fresh("p"));
            self.lines.push(format!("    let {v}: {shown} = match Channel.recv({ch}) {{"));
            self.lines.push(format!("        Some({x}) -> {x},"));
            self.lines.push(format!("        None -> {},", Self::none_value(&ty)));
            self.lines.push("    };".into());
            got.push(v);
        }
        let mut value = Self::ordered(&got[0], &got[1], &ty);
        for v in &got[2..] {
            value = Self::ordered(&value, v, &ty);
        }
        self.lines.push(format!("    {value}"));
        self.vars.truncate(scope);
        self.lines.push("}".into());
        self.lines.push(String::new());
        (name, ty)
    }

    /// `fn f(b: S) -> S with Async`: a producer sends two to four values over
    /// a channel and closes it, while a consumer, under `both`, drains it and
    /// combines what it gets in the order received.
    fn pipe_helper(&mut self) -> (String, Ty) {
        let name = self.fresh("f");
        let ty = self.scalar();
        let b = self.fresh("a");
        let shown = self.render(&ty);
        self.lines.push(self.async_signature(&name, &[(b.clone(), ty.clone())], &ty));
        let scope = self.vars.len();
        self.vars.push((b.clone(), ty.clone()));
        let cap = 1 + self.rng.below(2);
        let ch = self.fresh("v");
        self.lines.push(format!("    let {ch}: Channel<{shown}> = Channel.make({cap});"));
        let (produce, consume) = (self.fresh("f"), self.fresh("f"));
        let n = self.fresh("n");
        self.lines.push(format!("    fn {produce}({n}: Int) -> Int with Async {{"));
        self.lines.push(format!("        if ({n} <= 0) {{"));
        self.lines.push(format!("            Channel.close({ch});"));
        self.lines.push("            0".into());
        self.lines.push("        } else {".into());
        self.vars.push((n.clone(), Ty::Int));
        let e = self.expr(&ty, MAX_DEPTH - 1);
        self.vars.pop();
        self.lines.push(format!("            Channel.send({ch}, {e});"));
        self.lines.push(format!("            {produce}(({n} - 1))"));
        self.lines.push("        }".into());
        self.lines.push("    }".into());
        let (acc, x) = (self.fresh("a"), self.fresh("p"));
        self.lines.push(format!("    fn {consume}({acc}: {shown}) -> {shown} with Async {{"));
        self.lines.push(format!("        match Channel.recv({ch}) {{"));
        self.lines
            .push(format!("            Some({x}) -> {consume}({}),", Self::ordered(&acc, &x, &ty)));
        self.lines.push(format!("            None -> {acc},"));
        self.lines.push("        }".into());
        self.lines.push("    }".into());
        let (producer, consumer) = (self.fresh("f"), self.fresh("f"));
        let count = 2 + self.rng.below(3);
        self.lines.push(format!("    fn {producer}() -> Int with Async {{"));
        self.lines.push(format!("        {produce}({count})"));
        self.lines.push("    }".into());
        self.lines.push(format!("    fn {consumer}() -> {shown} with Async {{"));
        self.lines.push(format!("        {consume}({b})"));
        self.lines.push("    }".into());
        let p = self.fresh("v");
        self.lines
            .push(format!("    let {p}: (Int, {shown}) = Async.both({producer}, {consumer});"));
        self.lines.push(format!("    {p}.1"));
        self.vars.truncate(scope);
        self.lines.push("}".into());
        self.lines.push(String::new());
        (name, ty)
    }

    // ---- S17b: records, closures as values, Arrays, Maps and Either ----

    /// One of S17b's types.
    fn value_ty(&mut self) -> Ty {
        let elem = self.scalar();
        match self.rng.below(8) {
            0 | 1 if !self.recs.is_empty() => Ty::Rec(self.rng.below(self.recs.len())),
            2 => self.fun_ty(),
            3 => Ty::Arr(Box::new(elem)),
            4 => {
                let key = if self.chance(50) { Ty::Int } else { Ty::Str };
                Ty::Map(Box::new(key), Box::new(elem))
            }
            5 => Ty::Either(Box::new(elem), Box::new(self.scalar())),
            6 if !self.recs.is_empty() && self.chance(50) => {
                Ty::List(Box::new(Ty::Rec(self.rng.below(self.recs.len()))))
            }
            6 => Ty::List(Box::new(self.fun_ty())),
            _ => self.fun_ty(),
        }
    }

    /// A function type: one already drawn, more often than not, or a new one.
    fn fun_ty(&mut self) -> Ty {
        if !self.fun_tys.is_empty() && self.chance(60) {
            let known = self.fun_tys.clone();
            return self.pick(&known).clone();
        }
        let params = (0..1 + self.rng.below(2)).map(|_| self.scalar()).collect();
        let ty = Ty::Fun(params, Box::new(self.scalar()));
        self.fun_tys.push(ty.clone());
        ty
    }

    /// One or two record types of two or three fields: scalars, `Option<Int>`,
    /// and at most one function.
    fn records(&mut self) {
        for _ in 0..1 + self.rng.below(2) {
            let name = self.fresh("Rec");
            let mut fields = Vec::new();
            let mut has_fun = false;
            for _ in 0..2 + self.rng.below(2) {
                let roll = self.rng.below(100);
                let ty = if roll < 15 && !has_fun {
                    has_fun = true;
                    self.fun_ty()
                } else if roll < 30 {
                    Ty::Opt(Box::new(Ty::Int))
                } else {
                    self.scalar()
                };
                fields.push((self.fresh("f"), ty));
            }
            let shown: Vec<String> =
                fields.iter().map(|(f, t)| format!("{f}: {}", self.render(t))).collect();
            self.lines.push(format!("data {name} {{ {name} {{ {} }} }}", shown.join(", ")));
            self.recs.push(RecDef { name, fields });
        }
        self.lines.push(String::new());
    }

    /// `fn f(a: R, b: R, ...) -> (A) -> R { \(x: A) -> ((a - b) + e) }`: a
    /// closure over two of the helper's parameters, returned to be called
    /// later. It combines them so that their order shows: `-` on Ints, `+`
    /// on Strings, `a && !b` on Bools. A closure that swapped its captures
    /// would give another value, where `+` on Ints or `&&` would hide it.
    fn closure_helper(&mut self) {
        let name = self.fresh("f");
        let ret = self.fun_ty();
        let Ty::Fun(fparams, fret) = &ret else { unreachable!("fun_ty is a Fun") };
        let (fparams, fret) = (fparams.clone(), (**fret).clone());
        // Two parameters of the lambda's result type come first, so that the
        // lambda can capture both and combine them in an order that shows.
        let mut params = vec![(self.fresh("a"), fret.clone()), (self.fresh("a"), fret.clone())];
        let extra = self.rng.below(2);
        params.extend(self.params(extra));
        self.lines.push(self.signature(&name, &params, &ret));
        let scope = self.vars.len();
        self.vars.extend(params.iter().cloned());
        let (a, b) = (params[0].0.clone(), params[1].0.clone());
        let lambda = self.typed_lambda_with(&fparams, |g| {
            let e = g.expr(&fret, MAX_DEPTH - 1);
            match fret {
                Ty::Int => format!("(({a} - {b}) + {e})"),
                Ty::Str => format!("(({a} + {b}) + {e})"),
                Ty::Bool => format!("(({a} && !{b}) != {e})"),
                _ => unreachable!("a function type returns a scalar"),
            }
        });
        self.lines.push(format!("    {lambda}"));
        self.vars.truncate(scope);
        self.lines.push("}".into());
        let params = params.into_iter().map(|(_, t)| t).collect();
        self.closure_helpers.push(self.fns.len());
        self.fns.push(FnSig {
            name,
            params,
            ret,
            recursive: false,
            tparam: None,
            effects: Vec::new(),
        });
    }

    fn without_struct<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let was = std::mem::replace(&mut self.no_struct, true);
        let out = f(self);
        self.no_struct = was;
        out
    }

    /// Whether a `match` can take a value of `on`: not a function, an Array
    /// or a Map, and a record only from a variable (KI-181).
    fn matchable(&self, on: &Ty) -> bool {
        !matches!(on, Ty::Fun(..) | Ty::Arr(_) | Ty::Map(..))
            && (!holds_rec(on) || self.vars.iter().any(|(_, t)| t == on))
    }

    /// What `main` prints of a value: the value, or, when its type holds a
    /// function, whose printed form is the backend's own, what it gives.
    fn observe(&mut self, name: &str, ty: &Ty) -> Vec<String> {
        match ty {
            Ty::Fun(params, _) => {
                let args: Vec<String> = params.iter().map(|p| self.literal(p)).collect();
                vec![format!("{name}({})", args.join(", "))]
            }
            Ty::List(t) => match &**t {
                Ty::Fun(params, _) => {
                    let args: Vec<String> = params.iter().map(|p| self.literal(p)).collect();
                    let g = self.fresh("x");
                    vec![format!("map({name}, \\{g} -> {g}({}))", args.join(", "))]
                }
                Ty::Rec(i) if self.rec_holds_fun(*i) => {
                    let field = self.printable_fields(*i)[0].clone();
                    let r = self.fresh("x");
                    let rec = self.recs[*i].name.clone();
                    vec![format!("map({name}, \\({r}: {rec}) -> {r}.{field})")]
                }
                _ => vec![name.to_string()],
            },
            Ty::Rec(i) if self.rec_holds_fun(*i) => {
                self.printable_fields(*i).into_iter().map(|f| format!("{name}.{f}")).collect()
            }
            _ => vec![name.to_string()],
        }
    }

    fn rec_holds_fun(&self, i: usize) -> bool {
        self.recs[i].fields.iter().any(|(_, t)| matches!(t, Ty::Fun(..)))
    }

    fn printable_fields(&self, i: usize) -> Vec<String> {
        self.recs[i]
            .fields
            .iter()
            .filter(|(_, t)| !matches!(t, Ty::Fun(..)))
            .map(|(f, _)| f.clone())
            .collect()
    }

    /// `Rec { f: e, ... }`, its fields in a random order.
    fn construct(&mut self, i: usize, depth: u32) -> String {
        let mut fields = self.recs[i].fields.clone();
        for k in (1..fields.len()).rev() {
            let j = self.rng.below(k + 1);
            fields.swap(k, j);
        }
        let shown: Vec<String> =
            fields.iter().map(|(f, t)| format!("{f}: {}", self.expr(t, depth))).collect();
        format!("{} {{ {} }}", self.recs[i].name, shown.join(", "))
    }

    /// `({ ...v, f: e })`, in parentheses so it never starts a statement.
    fn spread(&mut self, i: usize, v: &str, depth: u32) -> String {
        let fields = self.recs[i].fields.clone();
        let (f, t) = self.pick(&fields).clone();
        format!("({{ ...{v}, {f}: {} }})", self.expr(&t, depth))
    }

    /// `\(x: A, y: B) -> e`, whose body may read anything in scope.
    fn typed_lambda(&mut self, params: &[Ty], ret: &Ty, depth: u32) -> String {
        self.typed_lambda_with(params, |g| g.expr(ret, depth))
    }

    /// `\(x: A, y: B) -> <body>`, with the parameters in scope for `body`.
    fn typed_lambda_with(
        &mut self,
        params: &[Ty],
        body: impl FnOnce(&mut Self) -> String,
    ) -> String {
        let scope = self.vars.len();
        let shown: Vec<String> = params
            .iter()
            .map(|t| {
                let n = self.fresh("x");
                self.vars.push((n.clone(), t.clone()));
                format!("{n}: {}", self.render(t))
            })
            .collect();
        let in_lambda = std::mem::replace(&mut self.in_lambda, true);
        let e = body(self);
        self.in_lambda = in_lambda;
        self.vars.truncate(scope);
        format!("\\({}) -> {e}", shown.join(", "))
    }

    /// `v.f` for each variable's field of type `ty`.
    fn field_paths(&self, ty: &Ty) -> Vec<String> {
        let mut paths = Vec::new();
        for (v, t) in &self.vars {
            if let Ty::Rec(i) = t {
                for (f, ft) in &self.recs[*i].fields {
                    if ft == ty {
                        paths.push(format!("{v}.{f}"));
                    }
                }
            }
        }
        paths
    }

    /// A function value: a helper's result, a lambda, a record's field, or a
    /// helper's name. A
    /// generic or constrained helper is never one (KI-090), nor a recursive
    /// one, whose countdown only its callers bound.
    fn fun_expr(&mut self, params: &[Ty], ret: &Ty, ty: &Ty, d: u32) -> String {
        // A helper that returns one, often a closure over its arguments.
        if self.chance(40)
            && let Some(call) = self.call(ty, d)
        {
            return call;
        }
        let mut named: Vec<String> = self
            .fns
            .iter()
            .filter(|f| {
                f.tparam.is_none()
                    && !f.recursive
                    && f.effects.is_empty()
                    && f.params == params
                    && f.ret == *ret
            })
            .map(|f| f.name.clone())
            .collect();
        named.extend(self.field_paths(ty));
        if named.is_empty() || self.chance(50) {
            self.typed_lambda(params, ret, d)
        } else {
            self.pick(&named).clone()
        }
    }

    fn arr_source(&mut self, elem: &Ty, d: u32) -> String {
        let arr = Ty::Arr(Box::new(elem.clone()));
        if let Some(v) = self.var_of(&arr)
            && self.chance(70)
        {
            return v;
        }
        let items: Vec<String> =
            (0..1 + self.rng.below(3)).map(|_| self.expr(elem, d.saturating_sub(1))).collect();
        format!("[|{}|]", items.join(", "))
    }

    /// Every Array function through `Array.` (KI-146).
    fn arr_expr(&mut self, elem: &Ty, d: u32) -> String {
        match self.rng.below(6) {
            0 | 1 => {
                let items: Vec<String> =
                    (0..1 + self.rng.below(3)).map(|_| self.expr(elem, d)).collect();
                format!("[|{}|]", items.join(", "))
            }
            2 => format!("Array.push({}, {})", self.arr_source(elem, d), self.expr(elem, d)),
            3 => {
                let from = self.scalar();
                let xs = self.arr_source(&from, d);
                let f = self.lambda(&[from], elem, d);
                format!("Array.map({xs}, {f})")
            }
            4 => {
                let xs = self.arr_source(elem, d);
                let f = self.lambda(std::slice::from_ref(elem), &Ty::Bool, d);
                format!("Array.filter({xs}, {f})")
            }
            _ if self.chance(50) => format!("Array.reverse({})", self.arr_source(elem, d)),
            _ => format!("Array.from_list({})", self.list_source(elem, d)),
        }
    }

    /// `({k: v, ...})` with distinct literal keys, in parentheses so it never
    /// starts a statement.
    fn map_literal(&mut self, key: &Ty, val: &Ty, d: u32) -> String {
        let mut keys: Vec<String> = Vec::new();
        for _ in 0..1 + self.rng.below(3) {
            let k = self.literal(key);
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let entries: Vec<String> =
            keys.iter().map(|k| format!("{k}: {}", self.expr(val, d.saturating_sub(1)))).collect();
        format!("({{{}}})", entries.join(", "))
    }

    fn map_source(&mut self, key: &Ty, val: &Ty, d: u32) -> String {
        let map = Ty::Map(Box::new(key.clone()), Box::new(val.clone()));
        if let Some(v) = self.var_of(&map)
            && self.chance(70)
        {
            return v;
        }
        self.map_literal(key, val, d)
    }

    /// Every Map function through `Map.` (KI-146); never `keys` or
    /// `values`, whose order is the hash's.
    fn map_expr(&mut self, key: &Ty, val: &Ty, d: u32) -> String {
        match self.rng.below(5) {
            0 | 1 => self.map_literal(key, val, d),
            2 => {
                let m = self.map_source(key, val, d);
                format!("Map.set({m}, {}, {})", self.expr(key, d), self.expr(val, d))
            }
            3 => {
                let m = self.map_source(key, val, d);
                format!("Map.delete({m}, {})", self.expr(key, d))
            }
            _ => {
                let (a, b) = (self.map_source(key, val, d), self.map_source(key, val, d));
                format!("Map.merge({a}, {b})")
            }
        }
    }

    fn map_key(&mut self) -> Ty {
        if self.chance(50) { Ty::Int } else { Ty::Str }
    }

    fn either_source(&mut self, a: &Ty, b: &Ty, d: u32) -> String {
        let either = Ty::Either(Box::new(a.clone()), Box::new(b.clone()));
        if let Some(v) = self.var_of(&either)
            && self.chance(70)
        {
            return v;
        }
        if self.chance(50) {
            format!("Left({})", self.expr(a, d))
        } else {
            format!("Right({})", self.expr(b, d))
        }
    }

    /// A `ty` read out of one of S17b's values: a call of a function
    /// variable with exactly its arity (KI-107), a record's field, an Array's
    /// or a Map's size, fold, membership or lookup (as an `Option`: never
    /// `array_get`, KI-147), or `either`.
    fn values_expr(&mut self, ty: &Ty, d: u32) -> Option<String> {
        let fun_vars: Vec<(String, Vec<Ty>)> = self
            .vars
            .iter()
            .filter_map(|(n, t)| match t {
                Ty::Fun(params, ret) if **ret == *ty => Some((n.clone(), params.clone())),
                _ => None,
            })
            .collect();
        let paths = self.field_paths(ty);
        let scalar = matches!(ty, Ty::Int | Ty::Bool | Ty::Str);
        let mut options: Vec<u8> = Vec::new();
        if !fun_vars.is_empty() {
            options.push(0);
        }
        if !paths.is_empty() {
            options.push(1);
        }
        match ty {
            Ty::Int => options.extend([2, 3, 4]),
            Ty::Bool => options.push(5),
            Ty::Opt(t) if matches!(**t, Ty::Int | Ty::Bool | Ty::Str) => options.extend([6, 7]),
            _ => {}
        }
        if scalar {
            options.push(8);
        }
        if options.is_empty() {
            return None;
        }
        Some(match *self.pick(&options) {
            0 => {
                let (f, params) = self.pick(&fun_vars).clone();
                let args: Vec<String> = params.iter().map(|p| self.expr(p, d)).collect();
                format!("{f}({})", args.join(", "))
            }
            1 => self.pick(&paths).clone(),
            2 => {
                let elem = self.scalar();
                let a = self.arr_source(&elem, d);
                if self.chance(50) { format!("Array.length({a})") } else { format!("len({a})") }
            }
            3 => {
                let elem = self.scalar();
                let a = self.arr_source(&elem, d);
                let init = self.expr(&Ty::Int, d);
                let f = self.lambda(&[Ty::Int, elem], &Ty::Int, d);
                format!("Array.fold({a}, {init}, {f})")
            }
            4 => {
                let (key, val) = (self.map_key(), self.scalar());
                format!("Map.size({})", self.map_source(&key, &val, d))
            }
            5 => {
                let (key, val) = (self.map_key(), self.scalar());
                let m = self.map_source(&key, &val, d);
                format!("Map.has({m}, {})", self.expr(&key, d))
            }
            6 => {
                let Ty::Opt(elem) = ty else { unreachable!("an Option") };
                let a = self.arr_source(elem, d);
                let i = self.expr(&Ty::Int, d);
                if self.chance(50) { format!("Array.get({a}, {i})") } else { format!("{a}[{i}]") }
            }
            7 => {
                let Ty::Opt(val) = ty else { unreachable!("an Option") };
                let key = self.map_key();
                let m = self.map_source(&key, val, d);
                let k = self.expr(&key, d);
                if self.chance(50) { format!("Map.get({m}, {k})") } else { format!("{m}[{k}]") }
            }
            _ => {
                let (a, b) = (self.scalar(), self.scalar());
                let e = self.either_source(&a, &b, d);
                let l = self.lambda(&[a], ty, d);
                let r = self.lambda(&[b], ty, d);
                format!("either({e}, {l}, {r})")
            }
        })
    }

    fn int_expr(&mut self, d: u32) -> String {
        match self.rng.below(11) {
            0 => self.literal(&Ty::Int),
            1 | 2 => {
                let op = if self.chance(50) { "+" } else { "-" };
                format!("({} {op} {})", self.expr(&Ty::Int, d), self.expr(&Ty::Int, d))
            }
            3 => format!("(({} * {}) % 9973)", self.expr(&Ty::Int, d), self.expr(&Ty::Int, d)),
            4 => {
                let (op, k) = if self.chance(50) {
                    ("/", 1 + self.rng.below(9))
                } else {
                    ("%", 2 + self.rng.below(8))
                };
                format!("({} {op} {k})", self.expr(&Ty::Int, d))
            }
            5 => {
                let elem = self.scalar();
                let f = if self.chance(50) { "len" } else { "length" };
                format!("{f}({})", self.list_source(&elem, d))
            }
            6 => format!("sum({})", self.list_source(&Ty::Int, d)),
            7 => {
                let elem = self.scalar();
                let xs = self.list_source(&elem, d);
                let init = self.expr(&Ty::Int, d);
                let f = self.lambda(&[Ty::Int, elem], &Ty::Int, d);
                format!("fold({xs}, {init}, {f})")
            }
            8 => {
                let pairs: Vec<(String, usize)> = self
                    .vars
                    .iter()
                    .filter_map(|(n, t)| match t {
                        Ty::Pair(a, _) if **a == Ty::Int => Some((n.clone(), 0)),
                        Ty::Pair(_, b) if **b == Ty::Int => Some((n.clone(), 1)),
                        _ => None,
                    })
                    .collect();
                if pairs.is_empty() {
                    self.literal(&Ty::Int)
                } else {
                    let (n, i) = self.pick(&pairs).clone();
                    format!("{n}.{i}")
                }
            }
            9 => format!("len({})", self.expr(&Ty::Str, d)),
            _ => self.leaf(&Ty::Int),
        }
    }

    fn bool_expr(&mut self, d: u32) -> String {
        match self.rng.below(6) {
            0 => self.literal(&Ty::Bool),
            1 | 2 => {
                let op = *self.pick(&["<", "<=", ">", ">=", "==", "!="]);
                format!("({} {op} {})", self.expr(&Ty::Int, d), self.expr(&Ty::Int, d))
            }
            3 => {
                let op = *self.pick(&["==", "!="]);
                let on = if self.chance(50) { Ty::Str } else { Ty::Bool };
                format!("({} {op} {})", self.expr(&on, d), self.expr(&on, d))
            }
            4 => {
                let op = *self.pick(&["&&", "||"]);
                format!("({} {op} {})", self.expr(&Ty::Bool, d), self.expr(&Ty::Bool, d))
            }
            _ => format!("!{}", self.expr(&Ty::Bool, d)),
        }
    }

    fn str_expr(&mut self, d: u32) -> String {
        match self.rng.below(5) {
            0 => self.literal(&Ty::Str),
            1 => format!("({} + {})", self.expr(&Ty::Str, d), self.expr(&Ty::Str, d)),
            2 => format!("to_string({})", self.expr(&Ty::Int, d)),
            _ => {
                // Only a variable goes inside `#{}`, so no string literal
                // nests inside another.
                let on = if self.chance(50) { Ty::Int } else { Ty::Str };
                match self.var_of(&on) {
                    Some(v) => {
                        let (a, b) = (*self.pick(&STRINGS), *self.pick(&STRINGS));
                        format!("\"{a}#{{{v}}}{b}\"")
                    }
                    None => self.literal(&Ty::Str),
                }
            }
        }
    }

    fn list_expr(&mut self, elem: &Ty, d: u32) -> String {
        match self.rng.below(6) {
            0 => {
                let items: Vec<String> =
                    (0..self.rng.below(4)).map(|_| self.expr(elem, d)).collect();
                format!("[{}]", items.join(", "))
            }
            1 => {
                let list = Ty::List(Box::new(elem.clone()));
                format!("[{} | {}]", self.expr(elem, d), self.expr(&list, d))
            }
            2 | 3 => {
                let from = self.scalar();
                let xs = self.list_source(&from, d);
                let f = self.lambda(&[from], elem, d);
                if self.chance(50) {
                    format!("map({xs}, {f})")
                } else {
                    format!("({xs} |> map({f}))")
                }
            }
            _ => {
                let xs = self.list_source(elem, d);
                let f = self.lambda(std::slice::from_ref(elem), &Ty::Bool, d);
                if self.chance(50) {
                    format!("filter({xs}, {f})")
                } else {
                    format!("({xs} |> filter({f}))")
                }
            }
        }
    }
}

/// Whether a value of `ty` is, or holds, a record, which a condition or a
/// scrutinee can't construct (KI-181).
fn holds_rec(ty: &Ty) -> bool {
    match ty {
        Ty::Rec(_) => true,
        Ty::List(t) => holds_rec(t),
        _ => false,
    }
}

/// `ty` with `T` replaced by `arg`.
fn subst(ty: &Ty, arg: &Ty) -> Ty {
    match ty {
        Ty::Var => arg.clone(),
        Ty::Opt(t) => Ty::Opt(Box::new(subst(t, arg))),
        Ty::List(t) => Ty::List(Box::new(subst(t, arg))),
        Ty::Pair(a, b) => Ty::Pair(Box::new(subst(a, arg)), Box::new(subst(b, arg))),
        Ty::GData(t) => Ty::GData(Box::new(subst(t, arg))),
        _ => ty.clone(),
    }
}

/// Whether `pattern`, a generic helper's return type, can be `want`, with
/// `T` bound in `bound`. `T` only stands for a scalar, a data type or an
/// enclosing `T`.
fn unify(pattern: &Ty, want: &Ty, bound: &mut Option<Ty>) -> bool {
    match (pattern, want) {
        (Ty::Var, _) => {
            let simple = matches!(want, Ty::Int | Ty::Bool | Ty::Str | Ty::Data(_) | Ty::Var);
            match bound {
                Some(t) => t == want,
                None if simple => {
                    *bound = Some(want.clone());
                    true
                }
                None => false,
            }
        }
        (Ty::Opt(p), Ty::Opt(w)) | (Ty::List(p), Ty::List(w)) | (Ty::GData(p), Ty::GData(w)) => {
            unify(p, w, bound)
        }
        (Ty::Pair(p1, p2), Ty::Pair(w1, w2)) => unify(p1, w1, bound) && unify(p2, w2, bound),
        _ => pattern == want,
    }
}

/// A method's parameters: `x`, and `y` when it is binary.
fn method_binders(binary: bool, on: Ty) -> Vec<(String, Ty)> {
    let mut binders = vec![("x".to_string(), on.clone())];
    if binary {
        binders.push(("y".to_string(), on));
    }
    binders
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A seed names the same programs across versions of the generator, so a
    /// reported case can be regenerated later. Adding a part (0190 S17) must
    /// leave the programs without it as they were.
    #[test]
    fn programs_without_parts_are_unchanged() {
        let all: String = (0..50).map(|case| program(1, case)).collect();
        assert_eq!(fnv64(all.as_bytes()), 1246894218328004906);
    }

    #[test]
    fn a_seed_and_case_give_one_program() {
        for seed in 0..50 {
            assert_eq!(program(seed, 3), program(seed, 3));
        }
        assert_ne!(program(1, 1), program(1, 2));
        assert_ne!(program(1, 1), program(2, 1));
    }

    #[test]
    fn every_program_is_small_and_balanced() {
        for case in 0..500 {
            let text = program(1, case);
            assert!(text.lines().count() < 400, "case {case} has {} lines", text.lines().count());
            assert!(text.contains("\nfn main() with IO {\n"), "case {case}");
            let mut depth = [0i32; 3];
            let mut in_string = false;
            for c in text.chars() {
                match c {
                    '"' => in_string = !in_string,
                    _ if in_string => {}
                    '{' => depth[0] += 1,
                    '}' => depth[0] -= 1,
                    '(' => depth[1] += 1,
                    ')' => depth[1] -= 1,
                    '[' => depth[2] += 1,
                    ']' => depth[2] -= 1,
                    _ => {}
                }
                assert!(depth.iter().all(|&d| d >= 0), "case {case}:\n{text}");
            }
            assert_eq!(depth, [0, 0, 0], "case {case}:\n{text}");
        }
    }

    #[test]
    fn divisors_are_non_zero_literals_and_only_main_prints() {
        for case in 0..500 {
            let text = program(7, case);
            let mut in_main = false;
            for line in text.lines() {
                in_main |= line.starts_with("fn main(");
                assert!(in_main || !line.contains("print("), "case {case}: {line}");
                for op in [" / ", " % "] {
                    for (at, _) in line.match_indices(op) {
                        let rest = &line[at + op.len()..];
                        let digits: String =
                            rest.chars().take_while(char::is_ascii_digit).collect();
                        assert!(
                            !digits.is_empty() && digits.parse::<u32>().unwrap() > 0,
                            "case {case}: {line}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn no_name_is_bound_twice() {
        for case in 0..200 {
            let text = program(3, case);
            let mut seen = std::collections::HashSet::new();
            for line in text.lines() {
                if let Some(rest) = line.trim_start().strip_prefix("let ") {
                    let name = rest.split(':').next().unwrap().to_string();
                    assert!(seen.insert(name.clone()), "case {case}: {name} bound twice");
                }
            }
        }
    }

    const CLASSES: Parts = Parts { classes: true, ..Parts::NONE };
    const VALUES: Parts = Parts { values: true, ..Parts::NONE };
    const EFFECTS: Parts = Parts { effects: true, ..Parts::NONE };
    const ASYNC: Parts = Parts { async_: true, ..Parts::NONE };
    const ALL: Parts = Parts { classes: true, values: true, effects: true, async_: true };

    fn balanced(text: &str) -> bool {
        let mut depth = [0i32; 3];
        let mut in_string = false;
        for c in text.chars() {
            match c {
                '"' => in_string = !in_string,
                _ if in_string => {}
                '{' => depth[0] += 1,
                '}' => depth[0] -= 1,
                '(' => depth[1] += 1,
                ')' => depth[1] -= 1,
                '[' => depth[2] += 1,
                ']' => depth[2] -= 1,
                _ => {}
            }
            if depth.iter().any(|&d| d < 0) {
                return false;
            }
        }
        depth == [0, 0, 0]
    }

    #[test]
    fn parts_are_named_on_the_command_line() {
        assert_eq!(Parts::parse("classes"), Ok(CLASSES));
        assert_eq!(Parts::parse(""), Ok(Parts::NONE));
        assert!(Parts::parse("classes,nope").is_err());
        assert_eq!(CLASSES.names(), "classes");
        assert_eq!(Parts::parse("values"), Ok(VALUES));
        let both = Parts { classes: true, values: true, ..Parts::NONE };
        assert_eq!(Parts::parse("values, classes"), Ok(both));
        assert_eq!(both.names(), "classes,values");
        assert_eq!(Parts::parse("effects"), Ok(EFFECTS));
        assert_eq!(Parts::parse("async"), Ok(ASYNC));
        assert_eq!(ALL.names(), "classes,values,effects,async");
    }

    #[test]
    fn a_classes_program_is_one_program_per_seed_and_balanced() {
        for case in 0..300 {
            let text = program_with(2, case, &CLASSES);
            assert_eq!(text, program_with(2, case, &CLASSES));
            assert!(balanced(&text), "case {case}:\n{text}");
            assert!(text.contains("--with classes, case"), "case {case}");
            let mut seen = std::collections::HashSet::new();
            for line in text.lines() {
                if let Some(rest) = line.trim_start().strip_prefix("let ") {
                    let name = rest.split(':').next().unwrap().to_string();
                    assert!(seen.insert(name.clone()), "case {case}: {name} bound twice");
                }
            }
        }
    }

    /// Each feature S17a adds shows up in a sample of programs, so a change
    /// that stops generating one is noticed.
    #[test]
    fn classes_programs_use_every_form() {
        let all: Vec<String> = (0..100).map(|case| program_with(4, case, &CLASSES)).collect();
        let count = |needle: &str| all.iter().filter(|t| t.contains(needle)).count();
        for form in ["\nclass Cls", "\ninstance Cls", " => Cls", "<a>", "<a: Cls", "deriving ("] {
            assert!(count(form) > 5, "{form} appears in only {} of 100", count(form));
        }
        assert_eq!(count("--with"), 100);
    }

    /// A plain instance's methods call none of their class's methods, so none
    /// calls itself at its own type (KI-104).
    #[test]
    fn a_plain_instance_calls_no_method_of_its_class() {
        for case in 0..300 {
            let text = program_with(5, case, &CLASSES);
            let mut methods: std::collections::HashMap<String, Vec<String>> = Default::default();
            let mut class = None;
            let mut checking: Option<Vec<String>> = None;
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("class ") {
                    class = Some(rest.split('<').next().unwrap().to_string());
                } else if let Some(rest) = line.strip_prefix("instance ")
                    && !line.contains("=>")
                {
                    let name = rest.split('<').next().unwrap();
                    checking = Some(methods[name].clone());
                } else if line == "}" {
                    class = None;
                    checking = None;
                } else if let Some(c) = &class
                    && let Some(rest) = line.trim_start().strip_prefix("fn ")
                {
                    let m = rest.split('(').next().unwrap().to_string();
                    methods.entry(c.clone()).or_default().push(m);
                } else if let Some(own) = &checking
                    && !line.trim_start().starts_with("fn ")
                {
                    for m in own {
                        assert!(!line.contains(&format!("{m}(")), "case {case}: {line}\n{text}");
                    }
                }
            }
        }
    }

    /// A function with a type parameter is fully annotated (KI-101), and no
    /// name is a prelude class method's (KI-071).
    #[test]
    fn generic_functions_are_annotated_and_named_apart_from_the_prelude() {
        for case in 0..300 {
            let text = program_with(6, case, &CLASSES);
            for line in text.lines() {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix("fn ") {
                    let name = rest.split(['(', '<']).next().unwrap();
                    assert!(
                        !["show", "eq", "neq", "add", "compare", "append"].contains(&name),
                        "case {case}: {line}"
                    );
                    if line.starts_with("fn ") && rest.split('(').next().unwrap().contains('<') {
                        assert!(line.contains(") -> "), "case {case}: {line}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_values_program_is_one_program_per_seed_and_balanced() {
        let both = Parts { classes: true, values: true, ..Parts::NONE };
        for (parts, with) in [(VALUES, "values"), (both, "classes,values")] {
            for case in 0..200 {
                let text = program_with(2, case, &parts);
                assert_eq!(text, program_with(2, case, &parts));
                assert!(balanced(&text), "case {case}:\n{text}");
                assert!(text.contains(&format!("--with {with}, case")), "case {case}");
                assert!(text.contains("import Flow.Array as Array\nimport Flow.Map as Map\n"));
            }
        }
    }

    /// Each feature S17b adds shows up in a sample of programs.
    #[test]
    fn values_programs_use_every_form() {
        let all: String = (0..200).map(|case| program_with(1, case, &VALUES)).collect();
        for (form, needle) in [
            ("a record", "data Rec"),
            ("a spread", "{ ..."),
            ("a function type", ") -> "),
            ("a lambda with a typed parameter", "\\(x"),
            ("a list of functions", "List<("),
            ("an Array", "[|"),
            ("an Array function", "Array.fold("),
            ("an index", "]["),
            ("a Map function", "Map.set("),
            ("a Map lookup", "Map.get("),
            ("a Left", "Left("),
            ("either", "either("),
        ] {
            assert!(all.contains(needle), "no {form} (`{needle}`) in 200 programs");
        }
        let lines: Vec<&str> = all.lines().collect();
        let has = |what: &str, test: &dyn Fn(&str) -> bool| {
            assert!(lines.iter().any(|l| test(l)), "no {what} in 200 programs");
        };
        has("record construction", &|l| {
            l.contains("Rec") && l.contains(" { f") && !l.starts_with("data")
        });
        has("a record pattern", &|l| l.trim_start().starts_with("Rec") && l.contains(" -> "));
        has("an Either match", &|l| l.trim_start().starts_with("Left(p"));
        has("a helper returning a function", &|l| l.starts_with("fn f") && l.contains(") -> ("));
        has("a function field", &|l| l.starts_with("data Rec") && l.contains(": ("));
        has("a field read", &|l| {
            l.split(|c: char| !c.is_alphanumeric() && c != '.')
                .any(|w| w.starts_with('v') && w.contains(".f"))
        });
    }

    /// The rules S17b keeps from open KIs, over a sample of programs.
    #[test]
    fn values_programs_keep_clear_of_their_known_issues() {
        for case in 0..200 {
            let text = program_with(3, case, &VALUES);
            // KI-157: a Map's key is an Int or a String.
            for (at, _) in text.match_indices("Map<") {
                let key = &text[at + 4..];
                assert!(key.starts_with("Int,") || key.starts_with("String,"), "case {case}");
            }
            // KI-147, KI-146: no bare `array_get`, and Arrays and Maps only
            // through their alias; never the hash-ordered `keys`/`values`.
            assert!(!text.contains("array_get"), "case {case}");
            assert!(!text.contains("Map.keys") && !text.contains("Map.values"), "case {case}");
            // A closure's printed form is the backend's own: no `let` holding
            // a function is printed whole.
            let mut functions = Vec::new();
            for line in text.lines() {
                let line = line.trim_start();
                if let Some(rest) = line.strip_prefix("let ")
                    && let Some((name, ty)) = rest.split_once(": ")
                    && ty.split(" = ").next().unwrap().contains("->")
                {
                    functions.push(name.to_string());
                }
                if let Some(arg) = line.strip_prefix("print(").and_then(|a| a.strip_suffix(");")) {
                    assert!(!functions.iter().any(|f| f == arg), "case {case}: prints {arg}");
                }
            }
        }
    }

    /// A closure a helper returns reads two of the helper's parameters, in
    /// an order that changes its value.
    #[test]
    fn a_returned_closure_combines_two_captures_in_order() {
        for seed in 0..100 {
            let mut rng = Rng::new(seed);
            let mut g = Gen::new(&mut rng, VALUES);
            g.closure_helper();
            let (sig, body) = (g.lines[0].clone(), g.lines[1].clone());
            let params = sig.split_once('(').unwrap().1;
            let names: Vec<&str> =
                params.split(", ").take(2).map(|p| p.split(':').next().unwrap()).collect();
            let (a, b) = (names[0], names[1]);
            assert!(
                [format!("({a} - {b})"), format!("({a} + {b})"), format!("({a} && !{b})")]
                    .iter()
                    .any(|combined| body.contains(combined.as_str())),
                "seed {seed}: {sig}\n{body}"
            );
        }
    }

    /// A closure a helper returns is called: its function type recurs, so
    /// something asks for it.
    #[test]
    fn returned_closures_are_called() {
        let mut calling = 0;
        for case in 0..200 {
            let text = program_with(1, case, &VALUES);
            let helpers: Vec<String> = text
                .lines()
                .filter(|l| l.starts_with("fn f") && l.contains(") -> (") && !l.ends_with(") {"))
                .filter_map(|l| l[3..].split('(').next().map(str::to_string))
                .collect();
            let called = helpers.iter().any(|h| {
                text.lines()
                    .filter(|l| !l.starts_with("fn "))
                    .any(|l| l.contains(&format!("{h}(")) || l.contains(&format!(" {h},")))
            });
            calling += usize::from(called);
        }
        assert!(
            calling >= 30,
            "only {calling} of 200 programs call a helper that returns a function"
        );
    }

    /// A handler helper's clause resumes with its parameter and the helper's
    /// own parameter, a capture known only at run time, in an order that
    /// changes the value; and the helper it handles gives a result that
    /// reads the value resumed.
    #[test]
    fn a_handler_helpers_clause_resumes_with_its_parameter_and_a_capture_in_order() {
        for seed in 0..100 {
            let mut rng = Rng::new(seed);
            let mut g = Gen::new(&mut rng, EFFECTS);
            g.declare_effects();
            g.effect_helper(true);
            let start = g.lines.len();
            let resumed = g.lines[start - 3..start - 1].to_vec();
            let v =
                resumed[0].trim_start().strip_prefix("let ").unwrap().split(':').next().unwrap();
            assert!(resumed[1].starts_with(&format!("    ({v} ")), "seed {seed}: {resumed:?}");
            g.handler_helper(0);
            let helper = g.lines[start..].join("\n");
            let a = helper.split_once('(').unwrap().1.split(':').next().unwrap();
            let p = helper.split_once("(resume, ").unwrap().1.split(')').next().unwrap();
            assert!(
                [format!("(({p} - {a})"), format!("(({p} + {a})"), format!("(({p} && !{a})")]
                    .iter()
                    .any(|combined| helper.contains(&format!("resume({combined}"))),
                "seed {seed}:\n{helper}"
            );
        }
    }

    /// Every effects program has a handler helper, and `main` calls it, so
    /// a clause that resumes with a run-time capture always runs.
    #[test]
    fn every_effects_program_calls_a_handler_helper() {
        for case in 0..200 {
            let text = program_with(1, case, &EFFECTS);
            let (helpers, main) = text.split_once("\nfn main() with IO {\n").unwrap();
            // `fn h(..) -> T {`, `let v: T = g(..) handle ..;`, `v`, `}`.
            let names: Vec<&str> = helpers
                .split("\n\n")
                .filter_map(|f| {
                    let lines: Vec<&str> = f.lines().collect();
                    let [sig, handled, value, "}"] = lines[..] else { return None };
                    let v = handled.strip_prefix("    let ")?.split(':').next()?;
                    (sig.starts_with("fn ")
                        && !sig.contains(" with ")
                        && handled.contains(" handle Eff")
                        && value == format!("    {v}"))
                    .then(|| sig[3..].split('(').next().unwrap())
                })
                .collect();
            assert!(
                names.iter().any(|name| main.contains(&format!("{name}("))),
                "case {case}: no handler helper that main calls\n{text}"
            );
        }
    }

    /// Whether the signature `fn f(..) -> R ..` returns a function: `R`
    /// has an arrow outside brackets.
    fn returns_a_function(sig: &str) -> bool {
        let mut depth = 0;
        let mut params_closed = None;
        for (i, c) in sig.char_indices() {
            match c {
                '(' | '<' | '[' => depth += 1,
                ')' | '>' | ']' if !sig[..i].ends_with('-') => {
                    depth -= 1;
                    if depth == 0 && params_closed.is_none() {
                        params_closed = Some(i);
                    }
                }
                _ => {}
            }
        }
        let ret = &sig[params_closed.expect("a parameter list") + 1..];
        let ret = ret.trim_start().strip_prefix("->").unwrap_or(ret);
        let mut depth = 0;
        let bytes = ret.as_bytes();
        (0..bytes.len()).any(|i| {
            match bytes[i] {
                b'(' | b'<' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b'>' if i == 0 || bytes[i - 1] != b'-' => depth -= 1,
                b'-' if depth == 0 && bytes.get(i + 1) == Some(&b'>') => return true,
                _ => {}
            }
            false
        })
    }

    #[test]
    fn an_effects_program_is_one_program_per_seed_and_balanced() {
        let all = Parts { async_: false, ..ALL };
        for (parts, with) in [(EFFECTS, "effects"), (all, "classes,values,effects")] {
            for case in 0..200 {
                let text = program_with(2, case, &parts);
                assert_eq!(text, program_with(2, case, &parts));
                assert!(balanced(&text), "case {case}:\n{text}");
                assert!(text.contains(&format!("--with {with}, case")), "case {case}");
                assert!(
                    text.contains("\nfn main() with IO {\n"),
                    "case {case}: main has no user effect"
                );
                for sig in text.lines().filter(|l| l.starts_with("fn ") && l.contains(" with Eff"))
                {
                    assert!(
                        !returns_a_function(sig),
                        "case {case}: the row goes to the function returned: {sig}"
                    );
                }
            }
        }
    }

    /// An arm of a `match` on a literal that fixes no type (`None`, `[]`,
    /// `Left(..)`, `Right(..)`) reads none of its binders: a class method
    /// called on one couldn't be resolved (E459).
    #[test]
    fn a_match_on_an_untyped_literal_reads_no_binder() {
        for parts in [Parts { classes: true, values: true, ..Parts::NONE }, ALL] {
            for case in 0..200 {
                let text = program_with(1, case, &parts);
                let lines: Vec<&str> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    let Some(at) = line.find("match ") else { continue };
                    let scrutinee = &line[at + 6..];
                    if !["None {", "[] {", "Left(", "Right("]
                        .iter()
                        .any(|l| scrutinee.starts_with(l))
                    {
                        continue;
                    }
                    let indent = line.len() - line.trim_start().len();
                    for arm in lines[i + 1..]
                        .iter()
                        .take_while(|l| l.len() - l.trim_start().len() > indent)
                    {
                        let Some((pattern, body)) = arm.trim().split_once(" -> ") else { continue };
                        for binder in pattern
                            .split(|c: char| !c.is_alphanumeric())
                            .filter(|w| w.starts_with('p') && w[1..].parse::<u32>().is_ok())
                        {
                            let reads =
                                body.split(|c: char| !c.is_alphanumeric()).any(|w| w == binder);
                            assert!(!reads, "case {case}: {line}\n{arm}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn an_async_program_is_one_program_per_seed_and_balanced() {
        for (parts, with) in [(ASYNC, "async"), (ALL, "classes,values,effects,async")] {
            for case in 0..200 {
                let text = program_with(2, case, &parts);
                assert_eq!(text, program_with(2, case, &parts));
                assert!(balanced(&text), "case {case}:\n{text}");
                assert!(text.contains(&format!("--with {with}, case")), "case {case}");
                assert!(text.contains("\nfn main() with IO {\n"), "case {case}");
                // KI-169: one `run_async`.
                assert_eq!(text.matches("Async.run_async(").count(), 1, "case {case}");
            }
        }
    }

    /// Each feature S17d adds shows up in a sample of programs, and the
    /// rules it keeps hold: only `main` prints, no async call is inside a
    /// lambda (KI-029), and nothing depends on timing.
    #[test]
    fn async_programs_use_every_form_and_keep_clear_of_their_known_issues() {
        let all: String = (0..200).map(|case| program_with(1, case, &ASYNC)).collect();
        for needle in [
            "Async.both(",
            "Task.spawn(fn() { ",
            "Task.await(",
            "Channel.make(",
            "Channel.send(",
            "Channel.recv(",
            "Channel.close(",
            "Async.run_async(",
            ") -> Int with Async {",
            ") -> String with Async {",
            ") -> Bool with Async {",
        ] {
            assert!(all.contains(needle), "no `{needle}` in 200 programs");
        }
        for case in 0..200 {
            let text = program_with(1, case, &ASYNC);
            let (helpers, main) = text.split_once("\nfn main() with IO {\n").unwrap();
            assert!(!helpers.contains("print("), "case {case}: a helper prints");
            assert!(main.contains("Async.run_async("), "case {case}");
            for line in text.lines() {
                for lambda in ["fn() { ", "\\("] {
                    if let Some(at) = line.find(lambda) {
                        let body = &line[at..];
                        for call in ["Async.", "Channel.", "Task.await", "Task.spawn"] {
                            assert!(!body.contains(call), "case {case}: {line}");
                        }
                    }
                }
                for timed in ["sleep(", "timeout(", "race(", "yield_now(", "select", "workers"] {
                    assert!(!line.contains(timed), "case {case}: {line}");
                }
            }
        }
    }

    /// The body `main` runs calls every top-level async helper, so every
    /// form runs.
    #[test]
    fn the_async_body_calls_every_async_helper() {
        for case in 0..200 {
            let text = program_with(1, case, &ASYNC);
            let body_name =
                text.split("Async.run_async(").nth(1).unwrap().split(')').next().unwrap();
            let helpers: Vec<&str> = text
                .lines()
                .filter(|l| l.starts_with("fn ") && l.ends_with(" with Async {"))
                .map(|l| l[3..].split('(').next().unwrap())
                .collect();
            let start = text.find(&format!("\nfn {body_name}(")).unwrap();
            let body = &text[start..text[start..].find("\n}\n").unwrap() + start];
            for helper in helpers.iter().filter(|h| **h != body_name) {
                assert!(
                    body.contains(&format!("{helper}("))
                        || body.contains(&format!("{helper},"))
                        || body.contains(&format!(" {helper})")),
                    "case {case}: {body_name} never calls {helper}"
                );
            }
        }
    }

    /// Each feature S17c adds shows up in a sample of programs, and the
    /// rules it keeps from open KIs hold.
    #[test]
    fn effects_programs_use_every_form_and_keep_clear_of_their_known_issues() {
        let all: String = (0..200).map(|case| program_with(1, case, &EFFECTS)).collect();
        for (form, needle) in [
            ("an effect", "\neffect Eff"),
            ("a row", ") with Eff"),
            ("a row of two", ", Eff"),
            ("a perform", "perform Eff"),
            ("a handler", " handle Eff"),
            ("nested handlers", "do { "),
            ("a resume", "-> resume("),
            ("a conditional resume", "{ resume("),
        ] {
            assert!(all.contains(needle), "no {form} (`{needle}`) in 200 programs");
        }
        let clauses: Vec<&str> = all.split("(resume, ").skip(1).collect();
        assert!(
            clauses.iter().any(|c| {
                let body = c.split_once(" -> ").unwrap().1;
                !body.starts_with("resume(") && !body.starts_with("if ")
            }),
            "no clause that gives a value without resuming"
        );
        for line in all.lines() {
            if let Some(op) = line.trim_start().strip_prefix("op") {
                // KI-184, KI-185: one operation of one parameter.
                let params = op.split_once(": (").map(|(_, rest)| rest.split(')').next().unwrap());
                if let Some(params) = params {
                    assert!(!params.contains(','), "{line}");
                }
            }
            // KI-029: no `perform` after a lambda's arrow on the same line.
            if let Some(at) = line.find("\\(") {
                assert!(!line[at..].contains("perform "), "{line}");
            }
        }
        for name in ["Console", "IO", "Async", "Process", "Env", "Clock", "Panic", "Div"] {
            assert!(!all.contains(&format!("effect {name} ")), "an effect named {name}");
        }
    }
}
