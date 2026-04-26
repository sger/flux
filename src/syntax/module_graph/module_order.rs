use std::collections::HashMap;

use crate::diagnostics::{
    position::{Position, Span},
    render_display_path, {Diagnostic, DiagnosticBuilder, IMPORT_CYCLE},
};

use super::{ModuleId, ModuleNode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

struct CycleDiagnostic {
    cycle: Vec<ModuleId>,
    anchor_file: String,
    anchor_position: Position,
}

impl CycleDiagnostic {
    fn new(cycle: Vec<ModuleId>, anchor_file: String, anchor_position: Position) -> Self {
        Self {
            cycle,
            anchor_file,
            anchor_position,
        }
    }

    fn contains(&self, id: &ModuleId) -> bool {
        self.cycle.iter().any(|cycle_id| cycle_id == id)
    }

    fn anchor_on(&mut self, node: &ModuleNode, position: Position) {
        self.anchor_file = node.path.to_string_lossy().to_string();
        self.anchor_position = position;
    }

    fn display_cycle(&self) -> String {
        self.cycle
            .iter()
            .map(|id| render_display_path(id.as_str()).into_owned())
            .collect::<Vec<_>>()
            .join(" -> ")
    }
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
    ) -> Result<(), CycleDiagnostic> {
        colors.insert(id.clone(), Color::Gray);
        stack.push(id.clone());

        if let Some(node) = nodes.get(id) {
            for edge in &node.imports {
                let next = &edge.target;
                match colors.get(next).copied().unwrap_or(Color::White) {
                    Color::White => {
                        if let Err(mut cycle) = dfs(next, nodes, colors, stack, order) {
                            if cycle.contains(next) {
                                cycle.anchor_on(node, edge.position);
                            }
                            return Err(cycle);
                        }
                    }
                    Color::Gray => {
                        if let Some(start) = stack.iter().position(|item| item == next) {
                            let mut cycle = stack[start..].to_vec();
                            cycle.push(next.clone());
                            return Err(CycleDiagnostic::new(
                                cycle,
                                node.path.to_string_lossy().to_string(),
                                edge.position,
                            ));
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
        let cycle_str = cycle.display_cycle();
        let error_spec = &IMPORT_CYCLE;
        let span = Span::new(cycle.anchor_position, cycle.anchor_position);
        let diag = Diagnostic::make_error(error_spec, &[&cycle_str], cycle.anchor_file, span)
            .with_primary_label(span, "this import participates in the cycle");
        return Err(Box::new(diag));
    }

    Ok(order)
}
