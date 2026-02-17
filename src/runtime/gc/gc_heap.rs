use std::rc::Rc;

use crate::runtime::{
    closure::Closure,
    frame::Frame,
    gc::{
        gc_handle::GcHandle, hamt_entry::HamtEntry, heap_entry::HeapEntry, heap_object::HeapObject,
    },
    leak_detector,
    value::Value,
};

const DEFAULT_GC_THRESHOLD: usize = 10_000;
const MIN_GC_THRESHOLD: usize = 1024;

enum WorkItem {
    Value(Value),
    Handle(GcHandle),
}
/// Stop-the-world mark-and-sweep garbage collector heap.
///
/// All persistent collection nodes (cons cells, HAMT nodes) are allocated here.
/// The VM triggers collection when the allocation count reaches the threshold.
pub struct GcHeap {
    entries: Vec<Option<HeapEntry>>,
    free_list: Vec<u32>,
    allocation_count: usize,
    gc_threshold: usize,
    gc_enabled: bool,
    total_collections: usize,
    total_allocations: usize,
    #[cfg(feature = "gc-telemetry")]
    telemetry: super::telemetry::GcTelemetry,
}

impl Default for GcHeap {
    fn default() -> Self {
        Self::new()
    }
}

impl GcHeap {
    /// Creates a new GC heap with default collection settings.
    ///
    /// Defaults:
    /// - threshold: `10_000` allocations
    /// - GC enabled: `true`
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free_list: Vec::new(),
            allocation_count: 0,
            gc_threshold: DEFAULT_GC_THRESHOLD,
            gc_enabled: true,
            total_collections: 0,
            total_allocations: 0,
            #[cfg(feature = "gc-telemetry")]
            telemetry: super::telemetry::GcTelemetry::new(),
        }
    }

    /// Creates a new heap with a custom GC allocation threshold.
    ///
    /// Unlike [`Self::set_threshold`], this does not clamp to `MIN_GC_THRESHOLD`.
    pub fn with_threshold(threshold: usize) -> Self {
        let mut heap = Self::new();
        heap.gc_threshold = threshold;
        heap
    }

    /// Enables or disables automatic collection checks.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.gc_enabled = enabled
    }

    /// Sets the allocation threshold that triggers collection.
    ///
    /// Values below `MIN_GC_THRESHOLD` are clamped upward.
    pub fn set_threshold(&mut self, threshhold: usize) {
        self.gc_threshold = threshhold.max(MIN_GC_THRESHOLD)
    }

    /// Returns `true` when GC is enabled and the threshold was reached.
    pub fn should_collect(&self) -> bool {
        self.gc_enabled && self.allocation_count >= self.gc_threshold
    }

    /// Allocates a new heap object and returns a stable handle to it.
    ///
    /// Freed slots are reused through the internal free-list before growing
    /// the storage vector.
    pub fn alloc(&mut self, object: HeapObject) -> GcHandle {
        leak_detector::record_gc_alloc();
        self.allocation_count += 1;
        self.total_allocations += 1;

        #[cfg(feature = "gc-telemetry")]
        let alloc_kind = super::telemetry::ObjectKind::from_object(&object);
        #[cfg(feature = "gc-telemetry")]
        let alloc_size = object.shallow_size_bytes();

        let entry = HeapEntry {
            object,
            marked: false,
        };

        let handle = if let Some(idx) = self.free_list.pop() {
            self.entries[idx as usize] = Some(entry);
            GcHandle(idx)
        } else {
            let idx = self.entries.len() as u32;
            self.entries.push(Some(entry));
            GcHandle(idx)
        };

        #[cfg(feature = "gc-telemetry")]
        self.telemetry.record_alloc(alloc_kind, alloc_size);

        handle
    }

    /// Returns an immutable reference to a live object by handle.
    ///
    /// Panics if the handle points to a free slot or is out of bounds.
    pub fn get(&self, handle: GcHandle) -> &HeapObject {
        &self.entries[handle.0 as usize]
            .as_ref()
            .expect("GcHeap::get: invalid or free handle")
            .object
    }

    /// Returns the number of currently live heap entries.
    pub fn live_count(&self) -> usize {
        let mut live = 0;
        let mut i = 0;
        let len = self.entries.len();

        while i < len {
            if self.entries[i].is_some() {
                live += 1;
            }
            i += 1;
        }

        live
    }

    /// Returns the total number of allocations performed by this heap.
    pub fn total_allocations(&self) -> usize {
        self.total_allocations
    }

    /// Returns the total number of completed GC cycles.
    pub fn total_collections(&self) -> usize {
        self.total_collections
    }

    /// Runs a full stop-the-world mark-and-sweep collection.
    ///
    /// The VM provides root sets from stack, globals, constants, the last popped
    /// value, and active frame closures.
    #[allow(clippy::too_many_arguments)]
    pub fn collect(
        &mut self,
        stack: &[Value],
        sp: usize,
        globals: &[Value],
        constants: &[Value],
        last_popped: &Value,
        frames: &[Frame],
        frame_index: usize,
    ) {
        #[cfg(feature = "gc-telemetry")]
        {
            let bytes_before = self.total_live_bytes();
            self.telemetry.begin_cycle(self.gc_threshold, bytes_before);
            let frame_count = if !frames.is_empty() {
                frame_index.min(frames.len() - 1) + 1
            } else {
                0
            };
            let roots_count = sp + globals.len() + constants.len() + 1 + frame_count;
            self.telemetry.set_roots_scanned(roots_count);
        }

        self.mark_slice(&stack[..sp]);
        self.mark_slice(globals);
        self.mark_slice(constants);

        self.mark_value(last_popped);

        if !frames.is_empty() {
            let end = frame_index.min(frames.len() - 1);
            let mut i = 0;
            while i <= end {
                self.mark_closure(&frames[i].closure);
                i += 1;
            }
        }

        let live_before = self.live_count();
        self.sweep();
        let live_after = self.live_count();
        let collected = live_before.saturating_sub(live_after);

        self.total_collections += 1;
        self.allocation_count = 0;

        self.adapt_threshold(collected, live_before);

        #[cfg(feature = "gc-telemetry")]
        {
            let bytes_after = self.total_live_bytes();
            self.telemetry.end_cycle(
                live_before,
                live_after,
                collected,
                bytes_after,
                self.gc_threshold,
            );
        }
    }

    fn mark_slice(&mut self, roots: &[Value]) {
        let mut i = 0;
        let len = roots.len();
        while i < len {
            self.mark_value(&roots[i]);
            i += 1;
        }
    }

    fn mark_value(&mut self, root: &Value) {
        let mut worklist = Vec::with_capacity(16);
        worklist.push(WorkItem::Value(root.clone()));

        while let Some(item) = worklist.pop() {
            #[cfg(feature = "gc-telemetry")]
            self.telemetry.update_peak_mark_stack(worklist.len() + 1);

            match item {
                WorkItem::Handle(handle) => self.mark_handle(handle, &mut worklist),
                WorkItem::Value(value) => match value {
                    Value::Gc(handle) => {
                        // Follow heap references lazily through dedicated handle items.
                        worklist.push(WorkItem::Handle(handle));
                    }
                    Value::Some(inner)
                    | Value::Left(inner)
                    | Value::Right(inner)
                    | Value::ReturnValue(inner) => {
                        worklist.push(WorkItem::Value(inner.as_ref().clone()));
                    }
                    Value::Array(elements) => {
                        let mut i = 0;
                        let len = elements.len();
                        while i < len {
                            worklist.push(WorkItem::Value(elements[i].clone()));
                            i += 1;
                        }
                    }
                    Value::Closure(closure) => {
                        let mut i = 0;
                        let len = closure.free.len();
                        while i < len {
                            worklist.push(WorkItem::Value(closure.free[i].clone()));
                            i += 1;
                        }
                    }
                    Value::JitClosure(closure) => {
                        let mut i = 0;
                        let len = closure.captures.len();
                        while i < len {
                            worklist.push(WorkItem::Value(closure.captures[i].clone()));
                            i += 1;
                        }
                    }
                    // Leaf types: no GC references
                    Value::Uninit
                    | Value::Integer(_)
                    | Value::Float(_)
                    | Value::Boolean(_)
                    | Value::String(_)
                    | Value::None
                    | Value::EmptyList
                    | Value::Function(_)
                    | Value::Builtin(_) => {}
                },
            }
        }
    }

    fn mark_handle(&mut self, handle: GcHandle, worklist: &mut Vec<WorkItem>) {
        let idx = handle.index() as usize;
        if idx >= self.entries.len() {
            return;
        }

        // Mark first so cycles/shared nodes are visited once.
        match self.entries[idx].as_mut() {
            Some(entry) => {
                if entry.marked {
                    return;
                }
                entry.marked = true;
            }
            None => return,
        }

        // Then enqueue children after releasing the mutable mark borrow.
        let object = match self.entries[idx].as_ref() {
            Some(entry) => &entry.object,
            None => return,
        };

        match object {
            HeapObject::Cons { head, tail } => {
                worklist.push(WorkItem::Value(head.clone()));
                worklist.push(WorkItem::Value(tail.clone()));
            }
            HeapObject::HamtNode { children, .. } => {
                let mut i = 0;
                let len = children.len();
                while i < len {
                    match &children[i] {
                        HamtEntry::Leaf(_, value) => {
                            worklist.push(WorkItem::Value(value.clone()));
                        }
                        HamtEntry::Node(child) | HamtEntry::Collision(child) => {
                            worklist.push(WorkItem::Handle(*child));
                        }
                    }
                    i += 1;
                }
            }
            HeapObject::HamtCollision { entries, .. } => {
                let mut i = 0;
                let len = entries.len();
                while i < len {
                    worklist.push(WorkItem::Value(entries[i].1.clone()));
                    i += 1;
                }
            }
        }
    }

    fn mark_closure(&mut self, closure: &Rc<Closure>) {
        let mut i = 0;
        let len = closure.free.len();
        while i < len {
            self.mark_value(&closure.free[i]);
            i += 1;
        }
    }

    fn sweep(&mut self) {
        let mut i = 0;
        let len = self.entries.len();
        while i < len {
            if let Some(entry) = &mut self.entries[i] {
                if entry.marked {
                    #[cfg(feature = "gc-telemetry")]
                    {
                        let kind = super::telemetry::ObjectKind::from_object(&entry.object);
                        let size = entry.object.shallow_size_bytes();
                        self.telemetry.record_survival(kind, size);
                    }
                    entry.marked = false;
                } else {
                    self.entries[i] = None;
                    self.free_list.push(i as u32);
                }
            }
            i += 1;
        }
    }

    /// Compute the total live bytes across all live heap entries.
    #[cfg(feature = "gc-telemetry")]
    fn total_live_bytes(&self) -> usize {
        let mut total = 0;
        for e in self.entries.iter().flatten() {
            total += e.object.shallow_size_bytes();
        }
        total
    }

    /// Returns a reference to the telemetry collector.
    #[cfg(feature = "gc-telemetry")]
    pub fn telemetry(&self) -> &super::telemetry::GcTelemetry {
        &self.telemetry
    }

    /// Produces a point-in-time heap snapshot.
    #[cfg(feature = "gc-telemetry")]
    pub fn heap_snapshot(&self, largest_n: usize) -> super::telemetry::HeapSnapshot {
        use super::telemetry::{HeapSnapshot, ObjectKind};

        let capacity = self.entries.len();
        let free_list_len = self.free_list.len();
        let live_count = self.live_count();

        let fragmentation = if capacity > 0 {
            1.0 - (live_count as f64 / capacity as f64)
        } else {
            0.0
        };
        let utilization = if capacity > 0 {
            live_count as f64 / capacity as f64
        } else {
            0.0
        };

        let mut counts = [0usize; 3];
        let mut bytes = [0usize; 3];
        let mut total_live_bytes = 0usize;
        let mut all_objects: Vec<(u32, ObjectKind, usize)> = Vec::new();

        for (i, entry) in self.entries.iter().enumerate() {
            if let Some(e) = entry {
                let kind = ObjectKind::from_object(&e.object);
                let size = e.object.shallow_size_bytes();
                let idx = kind as usize;
                counts[idx] += 1;
                bytes[idx] += size;
                total_live_bytes += size;
                all_objects.push((i as u32, kind, size));
            }
        }

        all_objects.sort_by(|a, b| b.2.cmp(&a.2));
        all_objects.truncate(largest_n);

        let kind_breakdown: Vec<(ObjectKind, usize, usize)> = ObjectKind::ALL
            .iter()
            .map(|&k| (k, counts[k as usize], bytes[k as usize]))
            .collect();

        HeapSnapshot {
            capacity,
            live_count,
            free_list_len,
            fragmentation,
            utilization,
            kind_breakdown,
            largest_objects: all_objects,
            total_live_bytes,
        }
    }

    /// Produce a full formatted telemetry report.
    #[cfg(feature = "gc-telemetry")]
    pub fn telemetry_report(&self) -> String {
        let snapshot = self.heap_snapshot(10);
        self.telemetry.report_full(&snapshot)
    }

    fn adapt_threshold(&mut self, collected: usize, total_before: usize) {
        if total_before == 0 {
            return;
        }

        let ratio = collected as f64 / total_before as f64;
        if ratio < 0.25 {
            self.gc_threshold = (self.gc_threshold * 2).min(1_000_000);
        } else if ratio > 0.75 {
            self.gc_threshold = (self.gc_threshold / 2).max(MIN_GC_THRESHOLD)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::runtime::{
        gc::{
            gc_heap::{GcHeap, MIN_GC_THRESHOLD},
            heap_object::HeapObject,
        },
        value::Value,
    };

    #[test]
    fn test_alloc_and_get() {
        let mut heap = GcHeap::new();
        let h = heap.alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::None,
        });
        match heap.get(h) {
            HeapObject::Cons { head, tail } => {
                assert_eq!(*head, Value::Integer(1));
                assert_eq!(*tail, Value::None);
            }
            _ => panic!("expected Cons"),
        }
        assert_eq!(heap.live_count(), 1);
    }

    #[test]
    fn test_collect_frees_unreachable() {
        let mut heap = GcHeap::new();
        // Allocate some cons cells with no roots
        for i in 0..100 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }
        assert_eq!(heap.live_count(), 100);

        // Collect with empty roots
        heap.collect(&[], 0, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.live_count(), 0);
        assert_eq!(heap.free_list.len(), 100);
    }

    #[test]
    fn test_collect_preserves_reachable() {
        let mut heap = GcHeap::new();
        let h = heap.alloc(HeapObject::Cons {
            head: Value::Integer(42),
            tail: Value::None,
        });

        // Allocate some unreachable objects
        for i in 0..50 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }
        assert_eq!(heap.live_count(), 51);

        // Collect with the first handle as a root on the stack
        let stack = vec![Value::Gc(h)];
        heap.collect(&stack, 1, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.live_count(), 1);

        // The reachable object is still valid
        match heap.get(h) {
            HeapObject::Cons { head, .. } => assert_eq!(*head, Value::Integer(42)),
            _ => panic!("expected Cons"),
        }
    }

    #[test]
    fn test_free_list_reuse() {
        let mut heap = GcHeap::new();
        let h1 = heap.alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::None,
        });
        let _h2 = heap.alloc(HeapObject::Cons {
            head: Value::Integer(2),
            tail: Value::None,
        });

        // Free everything
        heap.collect(&[], 0, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.live_count(), 0);
        assert_eq!(heap.free_list.len(), 2);

        // New alloc should reuse freed slots
        let h3 = heap.alloc(HeapObject::Cons {
            head: Value::Integer(3),
            tail: Value::None,
        });
        // Should reuse one of the freed slots
        assert!(h3.0 == h1.0 || h3.0 == 1);
        assert_eq!(heap.entries.len(), 2); // no new slots added
    }

    #[test]
    fn test_collect_traces_nested_cons() {
        let mut heap = GcHeap::new();

        // Build: [1 | [2 | None]]
        let inner = heap.alloc(HeapObject::Cons {
            head: Value::Integer(2),
            tail: Value::None,
        });
        let outer = heap.alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::Gc(inner),
        });

        // Add unreachable garbage
        for _ in 0..10 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(99),
                tail: Value::None,
            });
        }
        assert_eq!(heap.live_count(), 12);

        // Only the outer cons is a root, but inner should survive via tracing
        let stack = vec![Value::Gc(outer)];
        heap.collect(&stack, 1, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.live_count(), 2); // outer + inner

        // Both still valid
        match heap.get(outer) {
            HeapObject::Cons { head, tail } => {
                assert_eq!(*head, Value::Integer(1));
                assert_eq!(*tail, Value::Gc(inner));
            }
            _ => panic!("expected Cons"),
        }
        match heap.get(inner) {
            HeapObject::Cons { head, tail } => {
                assert_eq!(*head, Value::Integer(2));
                assert_eq!(*tail, Value::None);
            }
            _ => panic!("expected Cons"),
        }
    }

    #[test]
    fn test_should_collect_respects_threshold() {
        let mut heap = GcHeap::with_threshold(5);
        assert!(!heap.should_collect());
        for _ in 0..5 {
            heap.alloc(HeapObject::Cons {
                head: Value::None,
                tail: Value::None,
            });
        }
        assert!(heap.should_collect());
    }

    #[test]
    fn test_should_collect_respects_enabled() {
        let mut heap = GcHeap::with_threshold(2);
        for _ in 0..5 {
            heap.alloc(HeapObject::Cons {
                head: Value::None,
                tail: Value::None,
            });
        }
        assert!(heap.should_collect());

        heap.set_enabled(false);
        assert!(!heap.should_collect());
    }

    #[test]
    fn test_adaptive_threshold_doubles_on_low_collection() {
        let mut heap = GcHeap::with_threshold(MIN_GC_THRESHOLD);
        let initial = heap.gc_threshold;

        // Allocate some objects, keep them all alive
        let mut roots = Vec::new();
        for i in 0..10 {
            let h = heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
            roots.push(Value::Gc(h));
        }

        // Collect with all roots alive — nothing freed => ratio = 0
        heap.collect(&roots, roots.len(), &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.gc_threshold, initial * 2);
    }

    #[test]
    fn test_adaptive_threshold_halves_on_high_collection() {
        let mut heap = GcHeap::with_threshold(100_000);
        let initial = heap.gc_threshold;

        // Allocate lots of garbage
        for i in 0..100 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }

        // Collect with no roots — all freed => ratio = 1.0
        heap.collect(&[], 0, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.gc_threshold, initial / 2);
    }

    #[test]
    fn test_stress_100k_allocations() {
        let mut heap = GcHeap::with_threshold(1024);

        // Keep only a small set of live roots; the rest is garbage.
        let mut live = heap.alloc(HeapObject::Cons {
            head: Value::Integer(0),
            tail: Value::None,
        });

        for i in 1..100_000i64 {
            // Allocate garbage (not rooted)
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });

            // Periodically collect, keeping only `live`
            if heap.should_collect() {
                let stack = vec![Value::Gc(live)];
                heap.collect(&stack, 1, &[], &[], &Value::None, &[], 0);
            }

            // Every 10K iterations, replace the live root
            if i % 10_000 == 0 {
                live = heap.alloc(HeapObject::Cons {
                    head: Value::Integer(i),
                    tail: Value::None,
                });
            }
        }

        // Final collection
        let stack = vec![Value::Gc(live)];
        heap.collect(&stack, 1, &[], &[], &Value::None, &[], 0);
        // GC should have freed >99% of objects
        assert!(
            heap.live_count() <= 5,
            "Expected <= 5 live objects, got {}",
            heap.live_count()
        );
        assert!(heap.total_collections() > 0);
    }

    #[test]
    fn test_collect_traces_value_in_some_wrapper() {
        let mut heap = GcHeap::new();
        let inner = heap.alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::None,
        });
        // The GcHandle is wrapped in Some
        let root = Value::Some(std::rc::Rc::new(Value::Gc(inner)));

        heap.alloc(HeapObject::Cons {
            head: Value::Integer(99),
            tail: Value::None,
        }); // garbage

        let stack = vec![root];
        heap.collect(&stack, 1, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.live_count(), 1);

        match heap.get(inner) {
            HeapObject::Cons { head, .. } => assert_eq!(*head, Value::Integer(1)),
            _ => panic!("expected Cons"),
        }
    }

    #[test]
    fn test_collect_traces_value_in_array() {
        let mut heap = GcHeap::new();
        let h = heap.alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::None,
        });
        let arr = Value::Array(Rc::new(vec![Value::Gc(h), Value::Integer(2)]));

        // garbage
        heap.alloc(HeapObject::Cons {
            head: Value::Integer(99),
            tail: Value::None,
        });

        let stack = vec![arr];
        heap.collect(&stack, 1, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.live_count(), 1);
    }

    #[cfg(feature = "gc-telemetry")]
    #[test]
    fn test_telemetry_alloc_tracking() {
        let mut heap = GcHeap::new();
        for i in 0..10 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }
        assert_eq!(heap.telemetry().total_alloc_count(), 10);
        assert!(
            heap.telemetry()
                .kind_stats(crate::runtime::gc::telemetry::ObjectKind::Cons)
                .alloc_bytes
                > 0
        );
    }

    #[cfg(feature = "gc-telemetry")]
    #[test]
    fn test_telemetry_collection_cycle() {
        let mut heap = GcHeap::new();
        for i in 0..100 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }
        heap.collect(&[], 0, &[], &[], &Value::None, &[], 0);
        assert_eq!(heap.telemetry().cycles().len(), 1);
        let cycle = &heap.telemetry().cycles()[0];
        assert_eq!(cycle.live_before, 100);
        assert_eq!(cycle.live_after, 0);
        assert_eq!(cycle.collected_count, 100);
    }

    #[cfg(feature = "gc-telemetry")]
    #[test]
    fn test_telemetry_survival_tracking() {
        let mut heap = GcHeap::new();
        let h = heap.alloc(HeapObject::Cons {
            head: Value::Integer(42),
            tail: Value::None,
        });
        for i in 0..50 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }
        let stack = vec![Value::Gc(h)];
        heap.collect(&stack, 1, &[], &[], &Value::None, &[], 0);
        let cons_stats = heap
            .telemetry()
            .kind_stats(crate::runtime::gc::telemetry::ObjectKind::Cons);
        assert_eq!(cons_stats.survival_count, 1);
        assert!(cons_stats.survival_bytes > 0);
    }

    #[cfg(feature = "gc-telemetry")]
    #[test]
    fn test_heap_snapshot() {
        let mut heap = GcHeap::new();
        for i in 0..20 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }
        let snap = heap.heap_snapshot(5);
        assert_eq!(snap.live_count, 20);
        assert_eq!(snap.capacity, 20);
        assert_eq!(snap.free_list_len, 0);
        assert!(snap.utilization > 0.99);
        assert_eq!(snap.largest_objects.len(), 5);
        assert!(snap.total_live_bytes > 0);
    }

    #[cfg(feature = "gc-telemetry")]
    #[test]
    fn test_telemetry_report_does_not_panic() {
        let mut heap = GcHeap::new();
        for i in 0..10 {
            heap.alloc(HeapObject::Cons {
                head: Value::Integer(i),
                tail: Value::None,
            });
        }
        heap.collect(&[], 0, &[], &[], &Value::None, &[], 0);
        let report = heap.telemetry_report();
        assert!(report.contains("GC Allocation Stats"));
        assert!(report.contains("GC Cycles"));
        assert!(report.contains("Heap Snapshot"));
    }
}
