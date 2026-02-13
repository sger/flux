use crate::runtime::gc::heap_object::HeapObject;

/// Internal storage slot for a heap-managed object.
///
/// `marked` is the mark bit used by the sweep phase.
pub struct HeapEntry {
    /// Object payload allocated in the GC heap.
    pub object: HeapObject,
    /// Mark bit set during tracing in the current GC cycle.
    pub marked: bool,
}
