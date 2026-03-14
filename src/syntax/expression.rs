use std::fmt;

use crate::{
    diagnostics::position::Span,
    syntax::{
        Identifier, block::Block, effect_expr::EffectExpr, interner::Interner, type_expr::TypeExpr,
    },
};

/// Stable identifier for one expression node within a parsed program.
///
/// Assigned monotonically by the parser during construction. Survives cloning
/// and AST rewrites, allowing downstream passed (HM inference, PASS 2 codegen)
/// to share a stable type-map keyed by id instead of fragile pointer addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprId(pub u32);

impl ExprId {
    /// Sentinel value for expressions created outside the parser
    /// (AST transforms, compiler-generated nodes, tests).
    pub const UNSET: ExprId = ExprId(0);
}

/// Monotonic counter for allocating fresh [`ExprId`] values.
///
/// The parser owns one; after parsing the counter value can be extracted
/// and handed to AST transforms that create new expressions.
#[derive(Debug, Clone)]
pub struct ExprIdGen {
    next: u32,
}

impl ExprIdGen {
    pub fn new() -> Self {
        // Start at 1 so ExprId(0) remains the UNSET sentinel.
        Self { next: 1 }
    }

    /// Resume allocation from a previously-saved counter value.
    pub fn from_counter(next: u32) -> Self {
        Self { next }
    }

    /// Allocate the next unique [`ExprId`].
    pub fn next_id(&mut self) -> ExprId {
        let id = ExprId(self.next);
        self.next = self.next.saturating_add(1);
        id
    }

    /// Current counter value - save this to resume allocation later.
    pub fn counter(&self) -> u32 {
        self.next
    }
}

impl Default for ExprIdGen {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub enum StringPart {
    Literal(String),
    Interpolation(Box<Expression>),
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Pattern {
    Wildcard {
        span: Span,
    },
    Literal {
        expression: Expression,
        span: Span,
    },
    Identifier {
        name: Identifier,
        span: Span,
    },
    None {
        span: Span,
    },
    Some {
        pattern: Box<Pattern>,
        span: Span,
    },
    Left {
        pattern: Box<Pattern>,
        span: Span,
    },
    Right {
        pattern: Box<Pattern>,
        span: Span,
    },
    Cons {
        head: Box<Pattern>,
        tail: Box<Pattern>,
        span: Span,
    },
    EmptyList {
        span: Span,
    },
    Tuple {
        elements: Vec<Pattern>,
        span: Span,
    },
    /// User-defined ADT constructor pattern: `Circle(r)`, `Red`, `Node(l, v, r)`
    Constructor {
        /// Constructor symbol (for example `Circle`, `Red`, `Node`).
        name: Identifier,
        /// Nested subpatterns for constructor fields.
        fields: Vec<Pattern>,
        /// Source span covering the full constructor pattern.
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expression>,
    pub body: Expression,
    pub span: Span,
}

/// One arm inside a `handle` block.
///
/// ```flux
/// handle Console {
///     print(resume, msg) -> body
/// //         ^^^^^^  ^^^
/// //   resume_param  params[0]
/// }
/// ```
#[derive(Debug, Clone)]
pub struct HandleArm {
    /// The effect operation name
    pub operation_name: Identifier,
    /// First parameter receives the captured continuation.
    /// Typically named 'resume' by convetion but can be any identifier.
    pub resume_param: Identifier,
    /// Remaining operation argument names.
    pub params: Vec<Identifier>,
    pub body: Expression,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Expression {
    Identifier {
        name: Identifier,
        span: Span,
        id: ExprId,
    },
    Integer {
        value: i64,
        span: Span,
        id: ExprId,
    },
    Float {
        value: f64,
        span: Span,
        id: ExprId,
    },
    String {
        value: String,
        span: Span,
        id: ExprId,
    },
    InterpolatedString {
        parts: Vec<StringPart>,
        span: Span,
        id: ExprId,
    },
    Boolean {
        value: bool,
        span: Span,
        id: ExprId,
    },
    Prefix {
        operator: String,
        right: Box<Expression>,
        span: Span,
        id: ExprId,
    },
    Infix {
        left: Box<Expression>,
        operator: String,
        right: Box<Expression>,
        span: Span,
        id: ExprId,
    },
    If {
        condition: Box<Expression>,
        consequence: Block,
        alternative: Option<Block>,
        span: Span,
        id: ExprId,
    },
    DoBlock {
        block: Block,
        span: Span,
        id: ExprId,
    },
    Function {
        parameters: Vec<Identifier>,
        parameter_types: Vec<Option<TypeExpr>>,
        return_type: Option<TypeExpr>,
        effects: Vec<EffectExpr>,
        body: Block,
        span: Span,
        id: ExprId,
    },
    Call {
        function: Box<Expression>,
        arguments: Vec<Expression>,
        span: Span,
        id: ExprId,
    },
    ListLiteral {
        elements: Vec<Expression>,
        span: Span,
        id: ExprId,
    },
    ArrayLiteral {
        elements: Vec<Expression>,
        span: Span,
        id: ExprId,
    },
    TupleLiteral {
        elements: Vec<Expression>,
        span: Span,
        id: ExprId,
    },
    EmptyList {
        span: Span,
        id: ExprId,
    },
    Index {
        left: Box<Expression>,
        index: Box<Expression>,
        span: Span,
        id: ExprId,
    },
    Hash {
        pairs: Vec<(Expression, Expression)>,
        span: Span,
        id: ExprId,
    },
    MemberAccess {
        object: Box<Expression>,
        member: Identifier,
        span: Span,
        id: ExprId,
    },
    TupleFieldAccess {
        object: Box<Expression>,
        index: usize,
        span: Span,
        id: ExprId,
    },
    Match {
        scrutinee: Box<Expression>,
        arms: Vec<MatchArm>,
        span: Span,
        id: ExprId,
    },
    None {
        span: Span,
        id: ExprId,
    },
    Some {
        value: Box<Expression>,
        span: Span,
        id: ExprId,
    },
    // Either type expressions
    Left {
        value: Box<Expression>,
        span: Span,
        id: ExprId,
    },
    Right {
        value: Box<Expression>,
        span: Span,
        id: ExprId,
    },
    Cons {
        head: Box<Expression>,
        tail: Box<Expression>,
        span: Span,
        id: ExprId,
    },
    /// `perform Effect.operation(args)` — performs a user-declared effect operation.
    Perform {
        effect: Identifier,
        operation: Identifier,
        args: Vec<Expression>,
        span: Span,
        id: ExprId,
    },
    /// `expr handle Effect { op(resume, args) -> body, ... }` — handles an effect.
    Handle {
        expr: Box<Expression>,
        effect: Identifier,
        arms: Vec<HandleArm>,
        span: Span,
        id: ExprId,
    },
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expression::Identifier { name, .. } => write!(f, "{}", name),
            Expression::Integer { value, .. } => write!(f, "{}", value),
            Expression::Float { value, .. } => write!(f, "{}", value),
            Expression::String { value, .. } => write!(f, "\"{}\"", value),
            Expression::InterpolatedString { parts, .. } => {
                write!(f, "\"")?;
                for part in parts {
                    match part {
                        StringPart::Literal(s) => write!(f, "{}", s)?,
                        StringPart::Interpolation(expr) => write!(f, "#{{{}}}", expr)?,
                    }
                }
                write!(f, "\"")
            }
            Expression::Boolean { value, .. } => write!(f, "{}", value),
            Expression::Prefix {
                operator, right, ..
            } => {
                write!(f, "({}{})", operator, right)
            }
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => {
                write!(f, "({} {} {})", left, operator, right)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                write!(f, "if {} {}", condition, consequence)?;
                if let Some(alt) = alternative {
                    write!(f, " else {}", alt)?;
                }
                Ok(())
            }
            Expression::DoBlock { block, .. } => write!(f, "do {}", block),
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                let params: Vec<String> = parameters
                    .iter()
                    .enumerate()
                    .map(
                        |(idx, param)| match parameter_types.get(idx).and_then(|ta| ta.as_ref()) {
                            Some(ta) => format!("{param}: {ta}"),
                            None => param.to_string(),
                        },
                    )
                    .collect();
                if let Some(return_type) = return_type {
                    if effects.is_empty() {
                        write!(f, "fn({}) -> {} {}", params.join(", "), return_type, body)
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(ToString::to_string).collect();
                        write!(
                            f,
                            "fn({}) -> {} with {} {}",
                            params.join(", "),
                            return_type,
                            effects_text.join(", "),
                            body
                        )
                    }
                } else if effects.is_empty() {
                    write!(f, "fn({}) {}", params.join(", "), body)
                } else {
                    let effects_text: Vec<String> =
                        effects.iter().map(ToString::to_string).collect();
                    write!(
                        f,
                        "fn({}) with {} {}",
                        params.join(", "),
                        effects_text.join(", "),
                        body
                    )
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let args: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
                write!(f, "{}({})", function, args.join(", "))
            }
            Expression::ListLiteral { elements, .. } => {
                let elems: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                write!(f, "[{}]", elems.join(", "))
            }
            Expression::ArrayLiteral { elements, .. } => {
                let elems: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                write!(f, "[|{}|]", elems.join(", "))
            }
            Expression::TupleLiteral { elements, .. } => {
                let elems: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                match elems.len() {
                    0 => write!(f, "()"),
                    1 => write!(f, "({},)", elems[0]),
                    _ => write!(f, "({})", elems.join(", ")),
                }
            }
            Expression::EmptyList { .. } => write!(f, "[]"),
            Expression::Index { left, index, .. } => {
                write!(f, "({}[{}])", left, index)
            }
            Expression::Hash { pairs, .. } => {
                let items: Vec<String> =
                    pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", items.join(", "))
            }
            Expression::MemberAccess { object, member, .. } => {
                write!(f, "{}.{}", object, member)
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                write!(f, "{}.{}", object, index)
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                write!(f, "match {} {{", scrutinee)?;
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        write!(f, " {} if {} -> {};", arm.pattern, guard, arm.body)?;
                    } else {
                        write!(f, " {} -> {};", arm.pattern, arm.body)?;
                    }
                }
                write!(f, " }}")
            }
            Expression::None { .. } => write!(f, "None"),
            Expression::Some { value, .. } => write!(f, "Some({})", value),
            Expression::Left { value, .. } => write!(f, "Left({})", value),
            Expression::Right { value, .. } => write!(f, "Right({})", value),
            Expression::Cons { head, tail, .. } => write!(f, "[{} | {}]", head, tail),
            Expression::Perform {
                effect,
                operation,
                args,
                ..
            } => {
                let args_str: Vec<String> = args.iter().map(|a| format!("{a}")).collect();
                write!(
                    f,
                    "perform {}.{}({})",
                    effect,
                    operation,
                    args_str.join(", ")
                )
            }
            Expression::Handle {
                expr, effect, arms, ..
            } => {
                write!(f, "{} handle {} {{", expr, effect)?;
                for arm in arms {
                    let param: Vec<String> = std::iter::once(arm.resume_param)
                        .chain(arm.params.iter().copied())
                        .map(|p| format!("{p}"))
                        .collect();
                    write!(
                        f,
                        " {}({}) -> {},",
                        arm.operation_name,
                        param.join(", "),
                        arm.body
                    )?;
                }
                write!(f, " }}")
            }
        }
    }
}

impl Expression {
    /// Parser-assigned stable identifier for this expression node.
    pub fn expr_id(&self) -> ExprId {
        match self {
            Expression::Identifier { id, .. }
            | Expression::Integer { id, .. }
            | Expression::Float { id, .. }
            | Expression::String { id, .. }
            | Expression::InterpolatedString { id, .. }
            | Expression::Boolean { id, .. }
            | Expression::Prefix { id, .. }
            | Expression::Infix { id, .. }
            | Expression::If { id, .. }
            | Expression::DoBlock { id, .. }
            | Expression::Function { id, .. }
            | Expression::Call { id, .. }
            | Expression::ListLiteral { id, .. }
            | Expression::ArrayLiteral { id, .. }
            | Expression::TupleLiteral { id, .. }
            | Expression::EmptyList { id, .. }
            | Expression::Index { id, .. }
            | Expression::Hash { id, .. }
            | Expression::MemberAccess { id, .. }
            | Expression::TupleFieldAccess { id, .. }
            | Expression::Match { id, .. }
            | Expression::None { id, .. }
            | Expression::Some { id, .. }
            | Expression::Left { id, .. }
            | Expression::Right { id, .. }
            | Expression::Cons { id, .. }
            | Expression::Perform { id, .. }
            | Expression::Handle { id, .. } => *id,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Expression::Identifier { span, .. }
            | Expression::Integer { span, .. }
            | Expression::Float { span, .. }
            | Expression::String { span, .. }
            | Expression::InterpolatedString { span, .. }
            | Expression::Boolean { span, .. }
            | Expression::Prefix { span, .. }
            | Expression::Infix { span, .. }
            | Expression::If { span, .. }
            | Expression::DoBlock { span, .. }
            | Expression::Function { span, .. }
            | Expression::Call { span, .. }
            | Expression::ListLiteral { span, .. }
            | Expression::ArrayLiteral { span, .. }
            | Expression::TupleLiteral { span, .. }
            | Expression::EmptyList { span, .. }
            | Expression::Index { span, .. }
            | Expression::Hash { span, .. }
            | Expression::MemberAccess { span, .. }
            | Expression::TupleFieldAccess { span, .. }
            | Expression::Match { span, .. }
            | Expression::None { span, .. }
            | Expression::Some { span, .. } => *span,
            // Either type expressions
            Expression::Left { span, .. } | Expression::Right { span, .. } => *span,
            Expression::Cons { span, .. } => *span,
            Expression::Perform { span, .. } => *span,
            Expression::Handle { span, .. } => *span,
        }
    }
}

impl Expression {
    /// Formats this expression using the interner to resolve identifier names.
    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            Expression::Identifier { name, .. } => interner.resolve(*name).to_string(),
            Expression::Integer { value, .. } => format!("{}", value),
            Expression::Float { value, .. } => format!("{}", value),
            Expression::String { value, .. } => format!("\"{}\"", value),
            Expression::InterpolatedString { parts, .. } => {
                let mut out = String::from("\"");
                for part in parts {
                    match part {
                        StringPart::Literal(s) => out.push_str(s),
                        StringPart::Interpolation(expr) => {
                            out.push_str(&format!("#{{{}}}", expr.display_with(interner)));
                        }
                    }
                }
                out.push('"');
                out
            }
            Expression::Boolean { value, .. } => format!("{}", value),
            Expression::Prefix {
                operator, right, ..
            } => format!("({}{})", operator, right.display_with(interner)),
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => format!(
                "({} {} {})",
                left.display_with(interner),
                operator,
                right.display_with(interner)
            ),
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                let mut out = format!("if {} {}", condition.display_with(interner), consequence);
                if let Some(alt) = alternative {
                    out.push_str(&format!(" else {}", alt));
                }
                out
            }
            Expression::DoBlock { block, .. } => {
                format!("do {}", block)
            }
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                let params: Vec<String> = parameters
                    .iter()
                    .enumerate()
                    .map(|(idx, param)| {
                        let param_name = interner.resolve(*param);
                        match parameter_types.get(idx).and_then(|ty| ty.as_ref()) {
                            Some(ty) => format!("{param_name}: {}", ty.display_with(interner)),
                            None => param_name.to_string(),
                        }
                    })
                    .collect();
                if let Some(return_type) = return_type {
                    if effects.is_empty() {
                        format!(
                            "fn({}) -> {} {}",
                            params.join(", "),
                            return_type.display_with(interner),
                            body
                        )
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(|e| e.display_with(interner)).collect();
                        format!(
                            "fn({}) -> {} with {} {}",
                            params.join(", "),
                            return_type.display_with(interner),
                            effects_text.join(", "),
                            body
                        )
                    }
                } else if effects.is_empty() {
                    format!("fn({}) {}", params.join(", "), body)
                } else {
                    let effects_text: Vec<String> =
                        effects.iter().map(|e| e.display_with(interner)).collect();
                    format!(
                        "fn({}) with {} {}",
                        params.join(", "),
                        effects_text.join(", "),
                        body
                    )
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let args: Vec<String> =
                    arguments.iter().map(|a| a.display_with(interner)).collect();
                format!("{}({})", function.display_with(interner), args.join(", "))
            }
            Expression::ListLiteral { elements, .. } => {
                let elems: Vec<String> =
                    elements.iter().map(|e| e.display_with(interner)).collect();
                format!("[{}]", elems.join(", "))
            }
            Expression::ArrayLiteral { elements, .. } => {
                let elems: Vec<String> =
                    elements.iter().map(|e| e.display_with(interner)).collect();
                format!("[|{}|]", elems.join(", "))
            }
            Expression::TupleLiteral { elements, .. } => {
                let elems: Vec<String> =
                    elements.iter().map(|e| e.display_with(interner)).collect();
                match elems.len() {
                    0 => "()".to_string(),
                    1 => format!("({},)", elems[0]),
                    _ => format!("({})", elems.join(", ")),
                }
            }
            Expression::EmptyList { .. } => "[]".to_string(),
            Expression::Index { left, index, .. } => {
                format!(
                    "({}[{}])",
                    left.display_with(interner),
                    index.display_with(interner)
                )
            }
            Expression::Hash { pairs, .. } => {
                let items: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| {
                        format!("{}: {}", k.display_with(interner), v.display_with(interner))
                    })
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
            Expression::MemberAccess { object, member, .. } => {
                format!(
                    "{}.{}",
                    object.display_with(interner),
                    interner.resolve(*member)
                )
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                format!("{}.{}", object.display_with(interner), index)
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                let mut out = format!("match {} {{", scrutinee.display_with(interner));
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        out.push_str(&format!(
                            " {} if {} -> {};",
                            arm.pattern.display_with(interner),
                            guard.display_with(interner),
                            arm.body.display_with(interner)
                        ));
                    } else {
                        out.push_str(&format!(
                            " {} -> {};",
                            arm.pattern.display_with(interner),
                            arm.body.display_with(interner)
                        ));
                    }
                }
                out.push_str(" }}");
                out
            }
            Expression::None { .. } => "None".to_string(),
            Expression::Some { value, .. } => {
                format!("Some({})", value.display_with(interner))
            }
            Expression::Left { value, .. } => {
                format!("Left({})", value.display_with(interner))
            }
            Expression::Right { value, .. } => {
                format!("Right({})", value.display_with(interner))
            }
            Expression::Cons { head, tail, .. } => {
                format!(
                    "[{} | {}]",
                    head.display_with(interner),
                    tail.display_with(interner)
                )
            }
            Expression::Perform {
                effect,
                operation,
                args,
                ..
            } => {
                let args_str: Vec<String> = args.iter().map(|a| a.display_with(interner)).collect();
                format!(
                    "perform {}.{}({})",
                    interner.resolve(*effect),
                    interner.resolve(*operation),
                    args_str.join(", ")
                )
            }
            Expression::Handle {
                expr, effect, arms, ..
            } => {
                let mut out = format!(
                    "{} handle {} {{",
                    expr.display_with(interner),
                    interner.resolve(*effect)
                );
                for arm in arms {
                    let mut param_names: Vec<&str> = vec![interner.resolve(arm.resume_param)];
                    for p in &arm.params {
                        param_names.push(interner.resolve(*p));
                    }
                    out.push_str(&format!(
                        " {}({}) -> {},",
                        interner.resolve(arm.operation_name),
                        param_names.join(", "),
                        arm.body.display_with(interner)
                    ));
                }
                out.push_str(" }");
                out
            }
        }
    }
}

impl Pattern {
    /// Formats this pattern using the interner to resolve identifier names.
    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            Pattern::Wildcard { .. } => "_".to_string(),
            Pattern::Literal { expression, .. } => expression.display_with(interner),
            Pattern::Identifier { name, .. } => interner.resolve(*name).to_string(),
            Pattern::None { .. } => "None".to_string(),
            Pattern::Some { pattern, .. } => {
                format!("Some({})", pattern.display_with(interner))
            }
            Pattern::Left { pattern, .. } => {
                format!("Left({})", pattern.display_with(interner))
            }
            Pattern::Right { pattern, .. } => {
                format!("Right({})", pattern.display_with(interner))
            }
            Pattern::Cons { head, tail, .. } => {
                format!(
                    "[{} | {}]",
                    head.display_with(interner),
                    tail.display_with(interner)
                )
            }
            Pattern::EmptyList { .. } => "[]".to_string(),
            Pattern::Tuple { elements, .. } => {
                let elems: Vec<String> =
                    elements.iter().map(|e| e.display_with(interner)).collect();
                match elems.len() {
                    0 => "()".to_string(),
                    1 => format!("({},)", elems[0]),
                    _ => format!("({})", elems.join(", ")),
                }
            }
            Pattern::Constructor { name, fields, .. } => {
                if fields.is_empty() {
                    interner.resolve(*name).to_string()
                } else {
                    let fs: Vec<String> = fields.iter().map(|p| p.display_with(interner)).collect();
                    format!("{}({})", interner.resolve(*name), fs.join(", "))
                }
            }
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Wildcard { .. } => write!(f, "_"),
            Pattern::Literal { expression, .. } => write!(f, "{}", expression),
            Pattern::Identifier { name, .. } => write!(f, "{}", name),
            Pattern::None { .. } => write!(f, "None"),
            Pattern::Some { pattern, .. } => write!(f, "Some({})", pattern),
            Pattern::Left { pattern, .. } => write!(f, "Left({})", pattern),
            Pattern::Right { pattern, .. } => write!(f, "Right({})", pattern),
            Pattern::Cons { head, tail, .. } => write!(f, "[{} | {}]", head, tail),
            Pattern::EmptyList { .. } => write!(f, "[]"),
            Pattern::Tuple { elements, .. } => {
                let elems: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                match elems.len() {
                    0 => write!(f, "()"),
                    1 => write!(f, "({},)", elems[0]),
                    _ => write!(f, "({})", elems.join(", ")),
                }
            }
            Pattern::Constructor { name, fields, .. } => {
                if fields.is_empty() {
                    write!(f, "{}", name)
                } else {
                    let fs: Vec<String> = fields.iter().map(|p| p.to_string()).collect();
                    write!(f, "{}({})", name, fs.join(", "))
                }
            }
        }
    }
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard { span }
            | Pattern::Literal { span, .. }
            | Pattern::Identifier { span, .. }
            | Pattern::None { span }
            | Pattern::Some { span, .. }
            | Pattern::Left { span, .. }
            | Pattern::Right { span, .. }
            | Pattern::Tuple { span, .. }
            | Pattern::Constructor { span, .. } => *span,
            Pattern::Cons { span, .. } | Pattern::EmptyList { span, .. } => *span,
        }
    }
}

impl fmt::Display for MatchArm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(guard) = &self.guard {
            write!(f, "{} if {} -> {}", self.pattern, guard, self.body)
        } else {
            write!(f, "{} -> {}", self.pattern, self.body)
        }
    }
}

impl MatchArm {
    pub fn span(&self) -> Span {
        self.span
    }
}
