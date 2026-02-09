use std::collections::HashMap;

use crate::syntax::{
    diagnostics::{Diagnostic, IMPORT_CYCLE},
    position::{Position, Span},
};

use super::{ModuleId, ModuleNode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

pub(super) fn topo_order(
    nodes: &HashMap<ModuleId, ModuleNode>,
    entry: &ModuleId,
) -> Result<Vec<ModuleId>, Box<Diagnostic>> {
    let mut colors: HashMap<ModuleId, Color> = HashMap::new();
    let mut stack: Vec<ModuleId> = Vec::new();
    let mut order: Vec<ModuleId> = Vec::new();

    fn dfs(
        id: &ModuleId,
        nodes: &HashMap<ModuleId, ModuleNode>,
        colors: &mut HashMap<ModuleId, Color>,
        stack: &mut Vec<ModuleId>,
        order: &mut Vec<ModuleId>,
    ) -> Result<(), Vec<ModuleId>> {
        colors.insert(id.clone(), Color::Gray);
        stack.push(id.clone());

        if let Some(node) = nodes.get(id) {
            for edge in &node.imports {
                let next = &edge.target;
                match colors.get(next).copied().unwrap_or(Color::White) {
                    Color::White => dfs(next, nodes, colors, stack, order)?,
                    Color::Gray => {
                        if let Some(start) = stack.iter().position(|item| item == next) {
                            let mut cycle = stack[start..].to_vec();
                            cycle.push(next.clone());
                            return Err(cycle);
                        }
                    }
                    Color::Black => {}
                }
            }
        }

        stack.pop();
        colors.insert(id.clone(), Color::Black);
        order.push(id.clone());
        Ok(())
    }

    if let Err(cycle) = dfs(entry, nodes, &mut colors, &mut stack, &mut order) {
        let cycle_str = cycle
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(" -> ");
        let error_spec = &IMPORT_CYCLE;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&cycle_str],
            entry.as_str().to_string(),
            Span::new(Position::default(), Position::default()),
        );
        return Err(Box::new(diag));
    }

    Ok(order)
}
