//! Strongly connected components, for grouping mutually recursive definitions.
//!
//! Two copies of Tarjan's algorithm used to live in the compiler — one in Core
//! lowering, one in Aether's borrow inference. Both were recursive, and both
//! took their successors straight out of a `HashSet`, whose iteration order
//! varies from process to process. Neither defect had been observed to change
//! an artifact, but both are the kind that surfaces as an unreproducible build
//! in a compiler whose caches are content-addressed, and one of the two had
//! already needed a depth budget added after a real stack overflow.
//!
//! This implementation is iterative, and **deterministic by construction**: it
//! orders each node's successors by the node's position in `nodes` rather than
//! trusting the caller to pass them in a stable order. A caller may therefore
//! hand it a `HashSet` without introducing nondeterminism.

use std::collections::HashMap;
use std::hash::Hash;

/// Partitions `nodes` into strongly connected components.
///
/// A component is a maximal set of nodes each reachable from every other; a
/// node with no cycle through it is a component of one. Components are returned
/// in **reverse topological order** — every component appears before any
/// component that depends on it — which is the order a compiler wants for
/// emitting definitions, since a group's dependencies are then already bound.
///
/// `successors` is asked for each node's outgoing edges. Edges to nodes outside
/// `nodes` are ignored, so a caller may return a definition's whole free-variable
/// set without filtering it first. Duplicate edges are ignored.
///
/// The result is a pure function of `nodes` and the *set* of edges: the order
/// `successors` yields them in does not affect the output, nor does the hasher
/// of any collection it came from. Within a component, members appear in a
/// deterministic order derived from the traversal.
///
/// Runs in O(V + E) time, iteratively — a chain of a hundred thousand nodes
/// costs heap, not stack.
///
/// ```
/// use flux_generics::strongly_connected_components;
///
/// // a → b, b → a, c → a
/// let sccs = strongly_connected_components(&['a', 'b', 'c'], |n| match n {
///     'a' => vec!['b'],
///     'b' => vec!['a'],
///     _ => vec!['a'],
/// });
///
/// // {a, b} is one component and comes before {c}, which depends on it.
/// assert_eq!(sccs.len(), 2);
/// assert_eq!(sccs[0].len(), 2);
/// assert_eq!(sccs[1], vec!['c']);
/// ```
pub fn strongly_connected_components<N, F, I>(nodes: &[N], successors: F) -> Vec<Vec<N>>
where
    N: Copy + Eq + Hash,
    F: Fn(N) -> I,
    I: IntoIterator<Item = N>,
{
    let count = nodes.len();
    if count == 0 {
        return Vec::new();
    }

    // Later duplicates in `nodes` are unreachable: the first occurrence wins,
    // and every edge to that node resolves to it.
    let mut index_of: HashMap<N, usize> = HashMap::with_capacity(count);
    for (index, node) in nodes.iter().enumerate() {
        index_of.entry(*node).or_insert(index);
    }

    // Successors as indices, sorted and deduplicated. This is where
    // determinism is established: whatever order the caller's collection
    // iterated in, the traversal below sees one canonical order.
    let adjacency: Vec<Vec<usize>> = nodes
        .iter()
        .map(|node| {
            let mut edges: Vec<usize> = successors(*node)
                .into_iter()
                .filter_map(|to| index_of.get(&to).copied())
                .collect();
            edges.sort_unstable();
            edges.dedup();
            edges
        })
        .collect();

    const UNVISITED: usize = usize::MAX;

    let mut indices = vec![UNVISITED; count];
    let mut lowlinks = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut stack: Vec<usize> = Vec::new();
    let mut components: Vec<Vec<N>> = Vec::new();
    let mut next_index = 0usize;

    // The explicit traversal stack. Each frame is a node together with how far
    // through its successors we have got — the state a recursive call would
    // have kept in its own frame.
    let mut frames: Vec<(usize, usize)> = Vec::new();

    for root in 0..count {
        if indices[root] != UNVISITED {
            continue;
        }

        indices[root] = next_index;
        lowlinks[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;
        frames.push((root, 0));

        while let Some(&mut (node, ref mut cursor)) = frames.last_mut() {
            if *cursor < adjacency[node].len() {
                let successor = adjacency[node][*cursor];
                *cursor += 1;

                if indices[successor] == UNVISITED {
                    // Descend. The parent's lowlink is updated when this frame
                    // is popped, which is where the recursive form did it.
                    indices[successor] = next_index;
                    lowlinks[successor] = next_index;
                    next_index += 1;
                    stack.push(successor);
                    on_stack[successor] = true;
                    frames.push((successor, 0));
                } else if on_stack[successor] {
                    lowlinks[node] = lowlinks[node].min(indices[successor]);
                }
                continue;
            }

            // Every successor visited: this node is finished.
            frames.pop();

            if lowlinks[node] == indices[node] {
                // A root of its component: everything above it on the stack,
                // inclusive, is one strongly connected component.
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(nodes[member]);
                    if member == node {
                        break;
                    }
                }
                components.push(component);
            }

            // Propagate to the parent, as the return from recursion did.
            if let Some(&mut (parent, _)) = frames.last_mut() {
                lowlinks[parent] = lowlinks[parent].min(lowlinks[node]);
            }
        }
    }

    components
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Builds the successor lookup the compiler's call sites use: a map from
    /// node to an unordered set of nodes.
    fn graph(edges: &[(u32, &[u32])]) -> HashMap<u32, HashSet<u32>> {
        edges
            .iter()
            .map(|(from, to)| (*from, to.iter().copied().collect()))
            .collect()
    }

    fn sccs(nodes: &[u32], edges: &[(u32, &[u32])]) -> Vec<Vec<u32>> {
        let deps = graph(edges);
        strongly_connected_components(nodes, |n| {
            deps.get(&n)
                .into_iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
        })
    }

    #[test]
    fn chain_produces_separate_components() {
        // a→b→c, no cycle.
        let out = sccs(&[0, 1, 2], &[(0, &[1]), (1, &[2]), (2, &[])]);
        assert_eq!(out.len(), 3, "chain should produce 3 components: {out:?}");
        assert!(out.iter().all(|c| c.len() == 1));
    }

    #[test]
    fn mutual_recursion_produces_single_group() {
        let out = sccs(&[0, 1], &[(0, &[1]), (1, &[0])]);
        assert_eq!(out.len(), 1, "mutual recursion is one component: {out:?}");
        assert_eq!(out[0].len(), 2);
    }

    #[test]
    fn mixed_cycle_and_independent() {
        // a↔b, c independent.
        let out = sccs(&[0, 1, 2], &[(0, &[1]), (1, &[0]), (2, &[])]);
        assert_eq!(out.len(), 2, "should produce 2 components: {out:?}");
        let cycle = out.iter().find(|c| c.len() == 2).expect("a cycle group");
        assert!(cycle.contains(&0) && cycle.contains(&1));
    }

    #[test]
    fn three_way_cycle() {
        let out = sccs(&[0, 1, 2], &[(0, &[1]), (1, &[2]), (2, &[0])]);
        assert_eq!(out.len(), 1, "triangle is one component: {out:?}");
        assert_eq!(out[0].len(), 3);
    }

    #[test]
    fn no_dependencies() {
        let out = sccs(&[0, 1, 2], &[(0, &[]), (1, &[]), (2, &[])]);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn self_recursive_single() {
        let out = sccs(&[0], &[(0, &[0])]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], vec![0]);
    }

    #[test]
    fn reverse_topological_order() {
        // a→b→c: dependencies come first, so [c], [b], [a].
        let out = sccs(&[0, 1, 2], &[(0, &[1]), (1, &[2]), (2, &[])]);
        assert_eq!(out[0][0], 2, "c has no dependencies and comes first");
        assert_eq!(out[1][0], 1);
        assert_eq!(out[2][0], 0, "a depends on b and comes last");
    }

    #[test]
    fn edges_outside_the_node_set_are_ignored() {
        // A definition's free variables include names that are not siblings;
        // the caller is allowed to pass them through unfiltered.
        let out = sccs(&[0, 1], &[(0, &[1, 99]), (1, &[42])]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().flatten().all(|n| *n == 0 || *n == 1));
    }

    #[test]
    fn output_does_not_depend_on_successor_order() {
        // The same graph, with each node's successors presented in every
        // rotation. A `HashSet` gives no order guarantee, so the algorithm must
        // impose one itself.
        let nodes = [0u32, 1, 2, 3, 4, 5, 6];
        let base: Vec<(u32, Vec<u32>)> = vec![
            (0, vec![1, 2, 3]),
            (1, vec![4]),
            (2, vec![4]),
            (3, vec![4, 5]),
            (4, vec![0]),
            (5, vec![6]),
            (6, vec![5]),
        ];

        let expected = strongly_connected_components(&nodes, |n| {
            base.iter()
                .find(|(from, _)| *from == n)
                .map(|(_, to)| to.clone())
                .unwrap_or_default()
        });

        for rotation in 1..4 {
            let rotated: Vec<(u32, Vec<u32>)> = base
                .iter()
                .map(|(from, to)| {
                    let mut to = to.clone();
                    let shift = rotation % to.len().max(1);
                    to.rotate_left(shift);
                    (*from, to)
                })
                .collect();
            let got = strongly_connected_components(&nodes, |n| {
                rotated
                    .iter()
                    .find(|(from, _)| *from == n)
                    .map(|(_, to)| to.clone())
                    .unwrap_or_default()
            });
            assert_eq!(
                got, expected,
                "rotation {rotation} changed the result: {got:?} vs {expected:?}"
            );
        }
    }

    #[test]
    fn long_chain_does_not_overflow_the_stack() {
        // The recursive implementations this replaces would blow the stack
        // here; one of them had already needed a depth budget bolted on.
        const N: u32 = 100_000;
        let nodes: Vec<u32> = (0..N).collect();
        let out =
            strongly_connected_components(
                &nodes,
                |n| {
                    if n + 1 < N { vec![n + 1] } else { Vec::new() }
                },
            );
        assert_eq!(out.len(), N as usize);
        assert_eq!(out[0], vec![N - 1], "the tail has no dependencies");
        assert_eq!(out[(N - 1) as usize], vec![0]);
    }

    #[test]
    fn long_cycle_is_one_component() {
        const N: u32 = 50_000;
        let nodes: Vec<u32> = (0..N).collect();
        let out = strongly_connected_components(&nodes, |n| vec![(n + 1) % N]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), N as usize);
    }

    #[test]
    fn empty_input() {
        let out: Vec<Vec<u32>> = strongly_connected_components(&[], |_: u32| Vec::new());
        assert!(out.is_empty());
    }
}
