//! Grouping a statement list into minimal binding groups, in dependency order.
//!
//! Dependency analysis: which definitions must be bound together, and in what
//! order. GHC does this in the renamer, before the constraint solver runs and
//! without the solver ever seeing surface syntax; the same split holds here,
//! which is why this walks the AST and delegates the graph algorithm to
//! [`crate::shared::scc::strongly_connected_components`].
//!
//! # Why grouping is by reference, not adjacency
//!
//! Lowering used to scan for a *contiguous* run of `fn` statements, so any
//! other statement ended the run. Two mutually recursive functions separated by
//! a `let` were then emitted as two nested bindings, and the outer one's
//! reference to the inner was not in scope — which type-checked and then failed
//! at run time with `E1001 ... (got Uninit)`. See
//! `docs/known_issues.md#ki-087`.
//!
//! # Why order is by dependency, not by source position
//!
//! Grouping alone is not enough. Core lowering folds the plan from the back, so
//! the *first* item becomes the outermost binding, and a definition's
//! dependencies must be bound outside it. An earlier version of this module
//! emitted each group at its source position, which put
//!
//! ```flux
//! fn a() -> Int { b() + 1 }
//! fn b() -> Int { 41 }
//! ```
//!
//! in the order written — so `a` closed over `b`'s uninitialised slot and the
//! same `E1001 ... (got Uninit)` came back for a plainer program than the one
//! the grouping had fixed.
//!
//! So the plan is ordered by a topological sort over its items, not by source
//! position. It is not a free reordering: a `let` initializer *runs*, so
//! statements that are not definitions keep their order relative to each other,
//! a group may not be hoisted above a binding it reads, and it may not sink
//! below a statement that calls it. Ties are broken by source position, so a
//! program whose order was already correct is laid out exactly as written.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::ast::free_vars::{
    collect_free_vars_in_function_body, collect_free_vars_in_statement, collect_pattern_bindings,
};
use crate::syntax::{Identifier, statement::Statement};

/// One item of a planned statement list, in emission order.
#[derive(Debug)]
pub enum PlanItem<'a> {
    /// A binding group: one or more function definitions that reference one
    /// another, in source order, each with its index in the original slice.
    /// A group of one is an ordinary, possibly self-recursive, binding.
    ///
    /// Indices are carried because the VM backend and inference both drive
    /// their own statement loop by index and need to know which positions a
    /// group covers, and which to skip.
    Group {
        /// The position an *index-driven* consumer emits the group at. Usually
        /// its first member, but its last when the group reads a name bound in
        /// between.
        ///
        /// This is not the same thing as the group's place in the plan. A
        /// consumer that walks the plan in order (Core lowering) gets
        /// dependency order and should ignore this; a consumer that walks the
        /// statement slice by index (inference, the AST bytecode path) uses it
        /// via [`group_index`].
        anchor: usize,
        members: Vec<(usize, &'a Statement)>,
    },
    /// Any other statement, with its index.
    Other(usize, &'a Statement),
}

/// The names a statement binds.
///
/// Used to decide whether a function group may be hoisted above a statement:
/// it may not, if a member reads what that statement binds. A destructuring
/// `let` binds every name in its pattern, and missing those would let a group
/// be placed above a binding it reads.
fn bound_names(stmt: &Statement) -> HashSet<Identifier> {
    match stmt {
        Statement::Let { name, .. } | Statement::Function { name, .. } => {
            HashSet::from_iter([*name])
        }
        Statement::LetDestructure { pattern, .. } => collect_pattern_bindings(pattern),
        _ => HashSet::new(),
    }
}

/// A node of the ordering graph: either a binding group or a lone statement.
enum Node {
    Group { members: Vec<usize>, anchor: usize },
    Other(usize),
}

impl Node {
    /// The source position a node sorts at when nothing else separates it from
    /// another. For a group that is its first member, so an already-correct
    /// program keeps the order it was written in.
    fn key(&self) -> usize {
        match self {
            Node::Group { members, .. } => members[0],
            Node::Other(index) => *index,
        }
    }
}

/// Partitions `stmts` into binding groups and everything else, in the order
/// they must be emitted.
///
/// Function definitions are grouped by mutual reference into strongly connected
/// components, and the resulting items are ordered so that a definition's
/// dependencies come first — which is what Core lowering needs, since it folds
/// the plan from the back and the first item becomes the outermost binding.
///
/// Statements that are not definitions keep their order relative to each other,
/// because a `let` initializer is evaluated and reordering one would change what
/// the program does. Groups may move past them in either direction, because
/// binding a function evaluates nothing — but only as far as the names allow:
/// not above a binding a member reads, and not below a statement that calls one.
pub fn plan_block(stmts: &[Statement]) -> Vec<PlanItem<'_>> {
    // Index every function definition by name. A shadowing redefinition keeps
    // the first, matching the scoping the rest of the pipeline assumes.
    let mut index_of_fn: HashMap<Identifier, usize> = HashMap::new();
    let mut fn_indices: Vec<usize> = Vec::new();
    for (index, stmt) in stmts.iter().enumerate() {
        if let Statement::Function { name, .. } = stmt {
            fn_indices.push(index);
            index_of_fn.entry(*name).or_insert(index);
        }
    }

    // What each definition references. Function bodies are asked the same
    // question they were asked before dependency order arrived, so which
    // definitions land in a group together is unchanged.
    let references: HashMap<usize, HashSet<Identifier>> = fn_indices
        .iter()
        .map(|&index| {
            let refs = match &stmts[index] {
                Statement::Function {
                    parameters, body, ..
                } => collect_free_vars_in_function_body(parameters, body),
                _ => HashSet::new(),
            };
            (index, refs)
        })
        .collect();

    let groups = crate::shared::scc::strongly_connected_components(&fn_indices, |index| {
        references
            .get(&index)
            .into_iter()
            .flatten()
            .filter_map(|name| index_of_fn.get(name).copied())
            .collect::<Vec<_>>()
    });

    let mut nodes: Vec<Node> = Vec::with_capacity(stmts.len());
    for group in &groups {
        let mut members = group.clone();
        members.sort_unstable();
        let anchor = group_anchor(stmts, &members, &references);
        nodes.push(Node::Group { members, anchor });
    }
    for (index, stmt) in stmts.iter().enumerate() {
        if !matches!(stmt, Statement::Function { .. }) {
            nodes.push(Node::Other(index));
        }
    }
    nodes.sort_by_key(Node::key);

    let order = topological_order(stmts, &nodes, &references);

    order
        .into_iter()
        .map(|node| match &nodes[node] {
            Node::Group { members, anchor } => PlanItem::Group {
                anchor: *anchor,
                members: members.iter().map(|m| (*m, &stmts[*m])).collect(),
            },
            Node::Other(index) => PlanItem::Other(*index, &stmts[*index]),
        })
        .collect()
}

/// Where an index-driven consumer emits a group.
///
/// Its first member, unless a statement in between binds a name a member reads
/// — then its last, because the binding must exist before the closure that
/// captures it is created.
fn group_anchor(
    stmts: &[Statement],
    members: &[usize],
    references: &HashMap<usize, HashSet<Identifier>>,
) -> usize {
    let first = members[0];
    let last = members[members.len() - 1];
    let reads_intervening_binding = (first + 1..last).any(|between| {
        !matches!(stmts[between], Statement::Function { .. })
            && bound_names(&stmts[between])
                .iter()
                .any(|name| members.iter().any(|m| references[m].contains(name)))
    });
    if reads_intervening_binding {
        last
    } else {
        first
    }
}

/// Order the nodes so that each comes after everything it depends on.
///
/// Four kinds of edge, all meaning "must come first":
///
/// - consecutive non-definition statements, so their relative order — and with
///   it the order their side effects happen in — is preserved;
/// - a statement that binds a name a group reads, before that group;
/// - a group, before a statement that references one of its members;
/// - a group, before another group that references one of its members.
///
/// Ties go to the lower source position, so a plan that was already in
/// dependency order is emitted exactly as written. A cycle — `let x = f()`
/// alongside `fn f() { x }` — is a program that cannot be lowered either way;
/// its nodes are emitted in source order rather than dropped.
fn topological_order(
    stmts: &[Statement],
    nodes: &[Node],
    references: &HashMap<usize, HashSet<Identifier>>,
) -> Vec<usize> {
    let member_names: Vec<HashSet<Identifier>> = nodes
        .iter()
        .map(|node| match node {
            Node::Group { members, .. } => members
                .iter()
                .filter_map(|m| match &stmts[*m] {
                    Statement::Function { name, .. } => Some(*name),
                    _ => None,
                })
                .collect(),
            Node::Other(_) => HashSet::new(),
        })
        .collect();

    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut indegree: Vec<usize> = vec![0; nodes.len()];
    let mut edge = |from: usize, to: usize, successors: &mut Vec<Vec<usize>>| {
        if from != to && !successors[from].contains(&to) {
            successors[from].push(to);
            indegree[to] += 1;
        }
    };

    let mut previous_other: Option<usize> = None;
    for (node, item) in nodes.iter().enumerate() {
        match item {
            Node::Other(index) => {
                if let Some(previous) = previous_other {
                    edge(previous, node, &mut successors);
                }
                previous_other = Some(node);

                // This statement reads a group's member: the group first.
                let free = collect_free_vars_in_statement(&stmts[*index]);
                for (other, names) in member_names.iter().enumerate() {
                    if !names.is_disjoint(&free) {
                        edge(other, node, &mut successors);
                    }
                }
            }
            Node::Group { members, .. } => {
                let group_refs: HashSet<Identifier> = members
                    .iter()
                    .flat_map(|m| references[m].iter().copied())
                    .collect();

                for (other, item) in nodes.iter().enumerate() {
                    match item {
                        // A statement binding a name this group reads: it first.
                        Node::Other(index) => {
                            if !bound_names(&stmts[*index]).is_disjoint(&group_refs) {
                                edge(other, node, &mut successors);
                            }
                        }
                        // A group defining a name this group reads: it first.
                        Node::Group { .. } => {
                            if !member_names[other].is_disjoint(&group_refs) {
                                edge(other, node, &mut successors);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut ready: BinaryHeap<Reverse<(usize, usize)>> = (0..nodes.len())
        .filter(|node| indegree[*node] == 0)
        .map(|node| Reverse((nodes[node].key(), node)))
        .collect();

    let mut order = Vec::with_capacity(nodes.len());
    while let Some(Reverse((_, node))) = ready.pop() {
        order.push(node);
        for &next in &successors[node] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push(Reverse((nodes[next].key(), next)));
            }
        }
    }

    if order.len() < nodes.len() {
        // A cycle. Emit what is left in source order: the program is not
        // lowerable in any order, and a deterministic plan gives the later
        // passes a chance to report it rather than a missing binding.
        let emitted: HashSet<usize> = order.iter().copied().collect();
        order.extend((0..nodes.len()).filter(|node| !emitted.contains(node)));
    }
    order
}

/// Index a plan for a caller that walks the statement slice by position.
///
/// Returns the members of each multi-member group keyed by the anchor index it
/// is emitted at, and the set of positions whose group was emitted elsewhere
/// and which the caller must therefore skip. A group of one is left out of
/// both: it is emitted where it stands, so an index-driven loop needs to know
/// nothing about it.
///
/// Note that this discards the plan's *order*. A caller that can honour
/// dependency order — Core lowering — should walk the plan itself instead.
pub fn group_index<'a>(
    plan: &[PlanItem<'a>],
) -> (HashMap<usize, Vec<&'a Statement>>, HashSet<usize>) {
    let mut group_at: HashMap<usize, Vec<&'a Statement>> = HashMap::new();
    let mut covered: HashSet<usize> = HashSet::new();
    for item in plan {
        let PlanItem::Group { anchor, members } = item else {
            continue;
        };
        if members.len() < 2 {
            continue;
        }
        group_at.insert(*anchor, members.iter().map(|(_, stmt)| *stmt).collect());
        covered.extend(members.iter().map(|(index, _)| *index));
    }
    (group_at, covered)
}

#[cfg(test)]
mod tests {
    use super::{PlanItem, plan_block};
    use crate::syntax::{interner::Interner, lexer::Lexer, parser::Parser, statement::Statement};

    /// Parse a program and describe its plan as a list of names, in emission
    /// order. A group of more than one member is rendered `a+b`.
    fn plan_names(source: &str) -> Vec<String> {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        let describe = |stmt: &Statement, interner: &Interner| match stmt {
            Statement::Let { name, .. } | Statement::Function { name, .. } => {
                interner.resolve(*name).to_string()
            }
            _ => "_".to_string(),
        };

        plan_block(&program.statements)
            .iter()
            .map(|item| match item {
                PlanItem::Group { members, .. } => members
                    .iter()
                    .map(|(_, stmt)| describe(stmt, &interner))
                    .collect::<Vec<_>>()
                    .join("+"),
                PlanItem::Other(_, stmt) => describe(stmt, &interner),
            })
            .collect()
    }

    #[test]
    fn a_definition_is_emitted_after_the_one_it_calls() {
        // The regression: source order would put `a` first, and Core lowering
        // folds the plan from the back, so `a` would become the outermost
        // binding and close over b's uninitialised slot.
        assert_eq!(
            plan_names("fn a() { b() }\nfn b() { 41 }\n"),
            vec!["b", "a"]
        );
    }

    #[test]
    fn a_chain_is_emitted_deepest_dependency_first() {
        assert_eq!(
            plan_names("fn a() { b() }\nfn b() { c() }\nfn c() { 40 }\n"),
            vec!["c", "b", "a"]
        );
    }

    #[test]
    fn an_already_ordered_program_is_left_alone() {
        // Ties break by source position, so nothing moves without a reason.
        assert_eq!(
            plan_names("fn c() { 40 }\nfn b() { c() }\nfn a() { b() }\n"),
            vec!["c", "b", "a"]
        );
    }

    #[test]
    fn unrelated_definitions_keep_source_order() {
        assert_eq!(
            plan_names("fn a() { 1 }\nfn b() { 2 }\nfn c() { 3 }\n"),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn mutually_recursive_definitions_form_one_group() {
        let plan = plan_names("fn even(n) { odd(n) }\nfn odd(n) { even(n) }\n");
        assert_eq!(plan.len(), 1, "expected a single group, got {plan:?}");
        assert_eq!(plan[0], "even+odd");
    }

    #[test]
    fn a_group_split_by_a_value_binding_is_still_one_group() {
        // KI-087: grouping is by reference, not adjacency.
        let plan = plan_names("fn even(n) { odd(n) }\nlet k = 1\nfn odd(n) { even(n - k) }\n");
        assert!(
            plan.contains(&"even+odd".to_string()),
            "expected one group, got {plan:?}"
        );
    }

    #[test]
    fn a_group_is_emitted_after_a_binding_it_reads() {
        let plan = plan_names("fn f() { base }\nlet base = 10\n");
        assert_eq!(plan, vec!["base", "f"]);
    }

    #[test]
    fn a_group_is_emitted_before_a_statement_that_calls_it() {
        let plan = plan_names("let answer = f()\nfn f() { 42 }\n");
        assert_eq!(plan, vec!["f", "answer"]);
    }

    #[test]
    fn value_bindings_keep_their_order_relative_to_each_other() {
        // A `let` initializer runs, so reordering one would change what the
        // program does — only definitions may move.
        let plan = plan_names("let a = 1\nfn f() { 2 }\nlet b = 3\nlet c = 4\n");
        let lets: Vec<&String> = plan.iter().filter(|n| *n != "f").collect();
        assert_eq!(lets, vec!["a", "b", "c"]);
    }

    #[test]
    fn every_statement_is_emitted_exactly_once() {
        let plan = plan_names(
            "fn a() { b() }\nlet x = 1\nfn b() { x }\nlet y = a()\nfn c() { y }\nlet z = c()\n",
        );
        let mut sorted = plan.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), plan.len(), "duplicate items in {plan:?}");
        assert_eq!(plan.len(), 6, "missing items in {plan:?}");
    }

    #[test]
    fn a_cycle_between_a_definition_and_a_binding_still_emits_everything() {
        // `let x = f()` needs `f`; `fn f() { x }` needs `x`. Not lowerable in
        // any order — but the plan must still name both, so a later pass can
        // report it rather than a binding going missing.
        let plan = plan_names("let x = f()\nfn f() { x }\n");
        assert_eq!(plan.len(), 2, "expected both items, got {plan:?}");
    }
}
