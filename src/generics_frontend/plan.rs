//! Grouping a statement list into minimal binding groups.
//!
//! A run of function definitions is not one recursive binding: only the
//! definitions that actually reference one another need to be bound together.
//! Splitting them into strongly connected components gives each group the
//! smallest scope that still lets its members see each other, which is what
//! makes the rest of the pipeline able to generalize a group at a time.
//!
//! # Why statements between definitions do not end a group
//!
//! Lowering used to scan for a *contiguous* run of `fn` statements, so any
//! other statement ended the run. Two mutually recursive functions separated by
//! a `let` were then emitted as two nested bindings, and the outer one's
//! reference to the inner was not in scope — which type-checked and then failed
//! at run time with `E1001 ... (got Uninit)`. See
//! `docs/known_issues.md#ki-087`.
//!
//! Grouping here is by reference, not by adjacency. What an intervening
//! statement *does* affect is where the group is placed: a `let` initializer
//! runs, so a group that reads a `let`'s binding cannot be hoisted above it.

use std::collections::{HashMap, HashSet};

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
        /// The position the group is emitted at. Usually its first member,
        /// but its *last* when the group reads a name bound in between — see
        /// [`plan_block`]. A consumer that assumes the first member will put
        /// the group above a binding it reads.
        anchor: usize,
        members: Vec<(usize, &'a Statement)>,
    },
    /// Any other statement, with its index, emitted where it stands.
    Other(usize, &'a Statement),
}

/// The name a statement binds, when it binds exactly one.
///
/// Used to decide whether a function group may be hoisted above a statement:
/// it may not, if a member reads what that statement binds.
fn bound_name(stmt: &Statement) -> Option<Identifier> {
    match stmt {
        Statement::Let { name, .. } | Statement::Function { name, .. } => Some(*name),
        _ => None,
    }
}

/// Partitions `stmts` into binding groups and everything else.
///
/// `free_vars` is asked, for each function statement, which names its body
/// references — parameters and locally bound names already excluded. The caller
/// supplies it so that this module does not depend on lowering internals.
///
/// Non-function statements keep their source order relative to each other,
/// because a `let` initializer is evaluated and reordering one would change
/// what the program does. A function group is placed at its **first** member,
/// which is where the old contiguous-run scan put it and therefore preserves
/// the behaviour of every program that already worked — unless a member reads a
/// name bound between the group's first and last member, in which case the
/// group is placed at its last member so that name is in scope.
pub fn plan_block<'a, F>(stmts: &'a [Statement], free_vars: F) -> Vec<PlanItem<'a>>
where
    F: Fn(&'a Statement) -> HashSet<Identifier>,
{
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

    if fn_indices.len() < 2 {
        // Nothing to group: a single definition is its own group, and a block
        // with none needs no analysis.
        return stmts
            .iter()
            .enumerate()
            .map(|(index, stmt)| match stmt {
                Statement::Function { .. } => PlanItem::Group {
                    anchor: index,
                    members: vec![(index, stmt)],
                },
                other => PlanItem::Other(index, other),
            })
            .collect();
    }

    // What each definition references, restricted to sibling definitions.
    let references: HashMap<usize, HashSet<Identifier>> = fn_indices
        .iter()
        .map(|&index| (index, free_vars(&stmts[index])))
        .collect();

    let groups = flux_generics::strongly_connected_components(&fn_indices, |index| {
        references
            .get(&index)
            .into_iter()
            .flatten()
            .filter_map(|name| index_of_fn.get(name).copied())
            .collect::<Vec<_>>()
    });

    // Where each group is emitted, and which group each definition belongs to.
    let mut anchor_of: HashMap<usize, usize> = HashMap::new();
    let mut members_at: HashMap<usize, Vec<usize>> = HashMap::new();

    for group in &groups {
        let mut members = group.clone();
        members.sort_unstable();
        let first = members[0];
        let last = members[members.len() - 1];

        // A statement between the members that binds a name a member reads
        // forces the group down to its last member: the binding must exist
        // before the group is created, because a closure captures it.
        let reads_intervening_binding = (first + 1..last).any(|between| {
            !matches!(stmts[between], Statement::Function { .. })
                && bound_name(&stmts[between])
                    .is_some_and(|name| members.iter().any(|m| references[m].contains(&name)))
        });

        let anchor = if reads_intervening_binding {
            last
        } else {
            first
        };
        for member in &members {
            anchor_of.insert(*member, anchor);
        }
        members_at.insert(anchor, members);
    }

    let mut plan = Vec::with_capacity(stmts.len());
    for (index, stmt) in stmts.iter().enumerate() {
        match stmt {
            Statement::Function { .. } => {
                // Emit the whole group once, at its anchor; skip its other
                // members where they stand.
                if anchor_of.get(&index) == Some(&index) {
                    let members = &members_at[&index];
                    plan.push(PlanItem::Group {
                        anchor: index,
                        members: members.iter().map(|m| (*m, &stmts[*m])).collect(),
                    });
                }
            }
            other => plan.push(PlanItem::Other(index, other)),
        }
    }
    plan
}

/// Index a plan for a caller that walks the statement slice by position.
///
/// Returns the members of each multi-member group keyed by the anchor index it
/// is emitted at, and the set of positions whose group was emitted elsewhere
/// and which the caller must therefore skip. A group of one is left out of
/// both: it is emitted where it stands, so an index-driven loop needs to know
/// nothing about it.
///
/// Every consumer of a plan — Core lowering, VM compilation, inference — drives
/// its own loop this way, so the indexing lives here rather than three times
/// over.
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
