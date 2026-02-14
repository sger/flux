//! GC telemetry and heap analysis.
//!
//! Gated behind `#[cfg(feature = "gc-telemetry")]`. When the feature is
//! disabled this module is not compiled and all instrumentation compiles
//! to nothing.

use std::fmt;
use std::time::{Duration, Instant};

use super::heap_object::HeapObject;

// ---------------------------------------------------------------------------
// ObjectKind — lightweight classification of heap objects
// ---------------------------------------------------------------------------

/// Classification of heap object variants for telemetry bucketing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Cons = 0,
    HamtNode = 1,
    HamtCollision = 2,
}

impl ObjectKind {
    pub fn from_object(obj: &HeapObject) -> Self {
        match obj {
            HeapObject::Cons { .. } => ObjectKind::Cons,
            HeapObject::HamtNode { .. } => ObjectKind::HamtNode,
            HeapObject::HamtCollision { .. } => ObjectKind::HamtCollision,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ObjectKind::Cons => "Cons",
            ObjectKind::HamtNode => "HamtNode",
            ObjectKind::HamtCollision => "HamtCollision",
        }
    }

    /// All variants for iteration.
    pub const ALL: [ObjectKind; 3] = [
        ObjectKind::Cons,
        ObjectKind::HamtNode,
        ObjectKind::HamtCollision,
    ];
}

impl fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

// ---------------------------------------------------------------------------
// Per-kind allocation stats
// ---------------------------------------------------------------------------

/// Cumulative allocation statistics for one object kind.
#[derive(Debug, Clone, Default)]
pub struct KindStats {
    pub alloc_count: usize,
    pub alloc_bytes: usize,
    pub survival_count: usize,
    pub survival_bytes: usize,
}

// ---------------------------------------------------------------------------
// GC cycle metrics
// ---------------------------------------------------------------------------

/// Metrics captured for a single GC collection cycle.
#[derive(Debug, Clone)]
pub struct CycleMetrics {
    pub cycle_index: usize,
    pub duration: Duration,
    pub live_before: usize,
    pub live_after: usize,
    pub collected_count: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub bytes_collected: usize,
    pub roots_scanned: usize,
    pub peak_mark_stack: usize,
    pub threshold_before: usize,
    pub threshold_after: usize,
}

// ---------------------------------------------------------------------------
// Heap snapshot
// ---------------------------------------------------------------------------

/// Point-in-time summary of heap state.
#[derive(Debug, Clone)]
pub struct HeapSnapshot {
    pub capacity: usize,
    pub live_count: usize,
    pub free_list_len: usize,
    pub fragmentation: f64,
    pub utilization: f64,
    pub kind_breakdown: Vec<(ObjectKind, usize, usize)>,
    pub largest_objects: Vec<(u32, ObjectKind, usize)>,
    pub total_live_bytes: usize,
}

// ---------------------------------------------------------------------------
// GcTelemetry — the central telemetry collector
// ---------------------------------------------------------------------------

/// Telemetry collector for GC heap analysis.
///
/// Tracks per-kind allocation statistics, per-cycle collection metrics,
/// and provides heap snapshot inspection.
pub struct GcTelemetry {
    kind_stats: [KindStats; 3],
    cycles: Vec<CycleMetrics>,
    cycle_start: Option<Instant>,
    threshold_before: usize,
    bytes_before: usize,
    peak_mark_stack: usize,
    roots_scanned: usize,
}

impl GcTelemetry {
    pub fn new() -> Self {
        Self {
            kind_stats: [
                KindStats::default(),
                KindStats::default(),
                KindStats::default(),
            ],
            cycles: Vec::new(),
            cycle_start: None,
            threshold_before: 0,
            bytes_before: 0,
            peak_mark_stack: 0,
            roots_scanned: 0,
        }
    }

    // -- Allocation tracking --

    /// Record a new allocation of the given kind and size.
    #[inline]
    pub fn record_alloc(&mut self, kind: ObjectKind, size_bytes: usize) {
        let idx = kind as usize;
        self.kind_stats[idx].alloc_count += 1;
        self.kind_stats[idx].alloc_bytes += size_bytes;
    }

    // -- Survival tracking (called during sweep) --

    /// Record that an object survived a GC cycle.
    #[inline]
    pub fn record_survival(&mut self, kind: ObjectKind, size_bytes: usize) {
        let idx = kind as usize;
        self.kind_stats[idx].survival_count += 1;
        self.kind_stats[idx].survival_bytes += size_bytes;
    }

    // -- Collection cycle tracking --

    /// Call before starting a collection cycle.
    pub fn begin_cycle(&mut self, threshold: usize, bytes_before: usize) {
        self.cycle_start = Some(Instant::now());
        self.threshold_before = threshold;
        self.bytes_before = bytes_before;
        self.peak_mark_stack = 0;
        self.roots_scanned = 0;
    }

    /// Record the number of root values scanned.
    pub fn set_roots_scanned(&mut self, count: usize) {
        self.roots_scanned = count;
    }

    /// Update peak mark stack depth if current depth exceeds it.
    #[inline]
    pub fn update_peak_mark_stack(&mut self, depth: usize) {
        if depth > self.peak_mark_stack {
            self.peak_mark_stack = depth;
        }
    }

    /// Call after a collection cycle completes.
    pub fn end_cycle(
        &mut self,
        live_before: usize,
        live_after: usize,
        collected: usize,
        bytes_after: usize,
        threshold_after: usize,
    ) {
        let duration = self
            .cycle_start
            .map(|start| start.elapsed())
            .unwrap_or_default();
        let cycle_index = self.cycles.len();
        self.cycles.push(CycleMetrics {
            cycle_index,
            duration,
            live_before,
            live_after,
            collected_count: collected,
            bytes_before: self.bytes_before,
            bytes_after,
            bytes_collected: self.bytes_before.saturating_sub(bytes_after),
            roots_scanned: self.roots_scanned,
            peak_mark_stack: self.peak_mark_stack,
            threshold_before: self.threshold_before,
            threshold_after,
        });
        self.cycle_start = None;
    }

    // -- Queries --

    pub fn kind_stats(&self, kind: ObjectKind) -> &KindStats {
        &self.kind_stats[kind as usize]
    }

    pub fn cycles(&self) -> &[CycleMetrics] {
        &self.cycles
    }

    pub fn total_alloc_bytes(&self) -> usize {
        self.kind_stats.iter().map(|s| s.alloc_bytes).sum()
    }

    pub fn total_alloc_count(&self) -> usize {
        self.kind_stats.iter().map(|s| s.alloc_count).sum()
    }

    // -- Reporting --

    /// Formatted report of per-kind allocation statistics.
    pub fn report_allocation_stats(&self) -> String {
        let mut out = String::from("=== GC Allocation Stats ===\n");
        out.push_str(&format!(
            "{:<18} {:>10} {:>12} {:>10} {:>12}\n",
            "Kind", "Allocs", "AllocBytes", "Survived", "SurvBytes"
        ));
        out.push_str(&"-".repeat(64));
        out.push('\n');
        for kind in ObjectKind::ALL {
            let s = self.kind_stats(kind);
            out.push_str(&format!(
                "{:<18} {:>10} {:>12} {:>10} {:>12}\n",
                kind.label(),
                s.alloc_count,
                s.alloc_bytes,
                s.survival_count,
                s.survival_bytes,
            ));
        }
        out.push_str(&"-".repeat(64));
        out.push('\n');
        out.push_str(&format!(
            "{:<18} {:>10} {:>12}\n",
            "TOTAL",
            self.total_alloc_count(),
            self.total_alloc_bytes(),
        ));
        out
    }

    /// Formatted report of GC cycle history.
    pub fn report_cycles(&self) -> String {
        if self.cycles.is_empty() {
            return "=== GC Cycles ===\nNo collections performed.\n".to_string();
        }
        let mut out = String::from("=== GC Cycles ===\n");
        out.push_str(&format!(
            "{:>5} {:>10} {:>8} {:>8} {:>9} {:>10} {:>10} {:>10}\n",
            "Cycle",
            "Duration",
            "Before",
            "After",
            "Collected",
            "BytesBef",
            "BytesAft",
            "Threshold"
        ));
        out.push_str(&"-".repeat(80));
        out.push('\n');
        for c in &self.cycles {
            out.push_str(&format!(
                "{:>5} {:>8}us {:>8} {:>8} {:>9} {:>10} {:>10} {:>10}\n",
                c.cycle_index,
                c.duration.as_micros(),
                c.live_before,
                c.live_after,
                c.collected_count,
                c.bytes_before,
                c.bytes_after,
                c.threshold_after,
            ));
        }
        out
    }

    /// Full telemetry report combining all sections.
    pub fn report_full(&self, snapshot: &HeapSnapshot) -> String {
        let mut out = self.report_allocation_stats();
        out.push('\n');
        out.push_str(&self.report_cycles());
        out.push('\n');
        out.push_str(&format_heap_snapshot(snapshot));
        out
    }
}

impl Default for GcTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Heap snapshot formatting
// ---------------------------------------------------------------------------

pub fn format_heap_snapshot(snap: &HeapSnapshot) -> String {
    let mut out = String::from("=== Heap Snapshot ===\n");
    out.push_str(&format!("Capacity (slots):   {}\n", snap.capacity));
    out.push_str(&format!("Live objects:       {}\n", snap.live_count));
    out.push_str(&format!("Free list length:   {}\n", snap.free_list_len));
    out.push_str(&format!("Total live bytes:   {}\n", snap.total_live_bytes));
    out.push_str(&format!(
        "Fragmentation:      {:.2}%\n",
        snap.fragmentation * 100.0
    ));
    out.push_str(&format!(
        "Utilization:        {:.2}%\n",
        snap.utilization * 100.0
    ));
    out.push_str("\nBreakdown by kind:\n");
    out.push_str(&format!("{:<18} {:>8} {:>12}\n", "Kind", "Count", "Bytes"));
    out.push_str(&"-".repeat(40));
    out.push('\n');
    for (kind, count, bytes) in &snap.kind_breakdown {
        out.push_str(&format!(
            "{:<18} {:>8} {:>12}\n",
            kind.label(),
            count,
            bytes
        ));
    }
    if !snap.largest_objects.is_empty() {
        out.push_str(&format!(
            "\nLargest {} objects:\n",
            snap.largest_objects.len()
        ));
        for (slot, kind, bytes) in &snap.largest_objects {
            out.push_str(&format!(
                "  slot {:>6}  {:<18} {} bytes\n",
                slot,
                kind.label(),
                bytes
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_kind_from_object() {
        use crate::runtime::value::Value;

        let cons = HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::None,
        };
        assert_eq!(ObjectKind::from_object(&cons), ObjectKind::Cons);

        let node = HeapObject::HamtNode {
            bitmap: 0,
            children: vec![],
        };
        assert_eq!(ObjectKind::from_object(&node), ObjectKind::HamtNode);

        let collision = HeapObject::HamtCollision {
            hash: 0,
            entries: vec![],
        };
        assert_eq!(
            ObjectKind::from_object(&collision),
            ObjectKind::HamtCollision
        );
    }

    #[test]
    fn test_record_alloc_increments_stats() {
        let mut t = GcTelemetry::new();
        t.record_alloc(ObjectKind::Cons, 64);
        t.record_alloc(ObjectKind::Cons, 64);
        t.record_alloc(ObjectKind::HamtNode, 128);

        assert_eq!(t.kind_stats(ObjectKind::Cons).alloc_count, 2);
        assert_eq!(t.kind_stats(ObjectKind::Cons).alloc_bytes, 128);
        assert_eq!(t.kind_stats(ObjectKind::HamtNode).alloc_count, 1);
        assert_eq!(t.kind_stats(ObjectKind::HamtNode).alloc_bytes, 128);
        assert_eq!(t.kind_stats(ObjectKind::HamtCollision).alloc_count, 0);
        assert_eq!(t.total_alloc_count(), 3);
        assert_eq!(t.total_alloc_bytes(), 256);
    }

    #[test]
    fn test_record_survival() {
        let mut t = GcTelemetry::new();
        t.record_survival(ObjectKind::Cons, 64);
        t.record_survival(ObjectKind::Cons, 64);

        assert_eq!(t.kind_stats(ObjectKind::Cons).survival_count, 2);
        assert_eq!(t.kind_stats(ObjectKind::Cons).survival_bytes, 128);
    }

    #[test]
    fn test_cycle_metrics_recorded() {
        let mut t = GcTelemetry::new();
        t.begin_cycle(10_000, 5000);
        t.set_roots_scanned(42);
        t.update_peak_mark_stack(8);
        t.update_peak_mark_stack(3); // lower, should not update
        t.end_cycle(100, 50, 50, 2500, 20_000);

        assert_eq!(t.cycles().len(), 1);
        let c = &t.cycles()[0];
        assert_eq!(c.cycle_index, 0);
        assert_eq!(c.live_before, 100);
        assert_eq!(c.live_after, 50);
        assert_eq!(c.collected_count, 50);
        assert_eq!(c.bytes_before, 5000);
        assert_eq!(c.bytes_after, 2500);
        assert_eq!(c.bytes_collected, 2500);
        assert_eq!(c.roots_scanned, 42);
        assert_eq!(c.peak_mark_stack, 8);
        assert_eq!(c.threshold_before, 10_000);
        assert_eq!(c.threshold_after, 20_000);
    }

    #[test]
    fn test_report_allocation_stats_format() {
        let mut t = GcTelemetry::new();
        t.record_alloc(ObjectKind::Cons, 64);
        t.record_alloc(ObjectKind::HamtNode, 128);

        let report = t.report_allocation_stats();
        assert!(report.contains("GC Allocation Stats"));
        assert!(report.contains("Cons"));
        assert!(report.contains("HamtNode"));
        assert!(report.contains("TOTAL"));
    }

    #[test]
    fn test_report_cycles_empty() {
        let t = GcTelemetry::new();
        let report = t.report_cycles();
        assert!(report.contains("No collections performed"));
    }

    #[test]
    fn test_report_cycles_with_data() {
        let mut t = GcTelemetry::new();
        t.begin_cycle(10_000, 5000);
        t.end_cycle(100, 50, 50, 2500, 20_000);

        let report = t.report_cycles();
        assert!(report.contains("GC Cycles"));
        assert!(report.contains("Cycle"));
        assert!(!report.contains("No collections performed"));
    }

    #[test]
    fn test_heap_snapshot_format() {
        let snap = HeapSnapshot {
            capacity: 100,
            live_count: 60,
            free_list_len: 40,
            fragmentation: 0.4,
            utilization: 0.6,
            kind_breakdown: vec![
                (ObjectKind::Cons, 50, 3200),
                (ObjectKind::HamtNode, 10, 1280),
                (ObjectKind::HamtCollision, 0, 0),
            ],
            largest_objects: vec![(5, ObjectKind::HamtNode, 256)],
            total_live_bytes: 4480,
        };
        let report = format_heap_snapshot(&snap);
        assert!(report.contains("Capacity (slots):   100"));
        assert!(report.contains("Live objects:       60"));
        assert!(report.contains("40.00%"));
        assert!(report.contains("60.00%"));
        assert!(report.contains("Largest 1 objects"));
    }

    #[test]
    fn test_multiple_cycles() {
        let mut t = GcTelemetry::new();
        for i in 0..5 {
            t.begin_cycle(10_000, 1000 * (i + 1));
            t.end_cycle(100, 50, 50, 500 * (i + 1), 10_000);
        }
        assert_eq!(t.cycles().len(), 5);
        assert_eq!(t.cycles()[0].cycle_index, 0);
        assert_eq!(t.cycles()[4].cycle_index, 4);
    }

    #[test]
    fn test_object_kind_display() {
        assert_eq!(format!("{}", ObjectKind::Cons), "Cons");
        assert_eq!(format!("{}", ObjectKind::HamtNode), "HamtNode");
        assert_eq!(format!("{}", ObjectKind::HamtCollision), "HamtCollision");
    }
}
