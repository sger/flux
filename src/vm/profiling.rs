use std::time::Instant;

pub use crate::bytecode::debug_info::CostCentreInfo;

/// Runtime profiling data accumulated for a cost centre.
#[derive(Debug, Clone, Default)]
pub struct CostCentre {
    pub name: String,
    pub module: String,
    pub entries: u64,
    pub time_ns: u64,
    pub inner_time_ns: u64,
}

/// A single frame on the cost centre stack, pushed on function entry.
#[derive(Debug)]
pub(crate) struct CostCentreStackEntry {
    pub cc_index: u16,
    pub enter_time: Instant,
    pub child_time_ns: u64,
}

/// Print the profiling report in GHC-style format.
pub fn print_profile_report(centres: &[CostCentre], total_time_ns: u64) {
    if centres.is_empty() || total_time_ns == 0 {
        return;
    }

    // Filter to cost centres that were actually entered.
    let mut active: Vec<(usize, &CostCentre)> = centres
        .iter()
        .enumerate()
        .filter(|(_, cc)| cc.entries > 0)
        .collect();

    if active.is_empty() {
        return;
    }

    // Sort by self time descending.
    active.sort_by(|a, b| {
        let self_a = a.1.time_ns.saturating_sub(a.1.inner_time_ns);
        let self_b = b.1.time_ns.saturating_sub(b.1.inner_time_ns);
        self_b.cmp(&self_a)
    });

    eprintln!();
    eprintln!("  ── Flux Profiling Report ─────────────────────────────────────");
    eprintln!(
        "  {:<24} {:<16} {:>8} {:>8}",
        "COST CENTRE", "MODULE", "entries", "%time"
    );
    eprintln!(
        "  {:<24} {:<16} {:>8} {:>8}",
        "───────────", "──────", "───────", "─────"
    );

    for (_, cc) in &active {
        let self_time = cc.time_ns.saturating_sub(cc.inner_time_ns);
        let pct_time = (self_time as f64 / total_time_ns as f64) * 100.0;

        eprintln!(
            "  {:<24} {:<16} {:>8} {:>7.1}%",
            truncate(&cc.name, 24),
            truncate(&cc.module, 16),
            cc.entries,
            pct_time,
        );
    }
    eprintln!("  ──────────────────────────────────────────────────────────────");
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
