/// handle into the GC heap.
///
/// A `GcHandle` is a lightweight, copyable index that refers to a heap-allocated
/// object managed by the garbage collector. It is the runtime representation
/// used inside `Value::Gc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GcHandle(pub(crate) u32);

impl GcHandle {
    /// Returns the raw heap slot index backing this handle.
    pub fn index(self) -> u32 {
        self.0
    }

    #[cfg(test)]
    pub fn new_for_test(index: u32) -> Self {
        Self(index)
    }
}
