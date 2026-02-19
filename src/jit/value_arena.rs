use crate::runtime::value::Value;

const CHUNK_SIZE: usize = 1024;

/// Bump allocator for JIT-allocated Values.
///
/// Values allocated here have stable pointers (never moved) because each chunk
/// is a `Box<[Value]>`. The arena can be reset between top-level calls to
/// reclaim memory without per-value deallocation.
pub struct ValueArena {
    chunks: Vec<Box<[Value]>>,
    offset: usize,
}

impl ValueArena {
    pub fn new() -> Self {
        Self {
            chunks: vec![Self::new_chunk()],
            offset: 0,
        }
    }

    /// Allocate a Value in the arena, returning a stable pointer.
    ///
    /// # Safety
    /// The returned pointer is valid until `reset()` is called.
    pub fn alloc(&mut self, value: Value) -> *mut Value {
        if self.offset >= CHUNK_SIZE {
            self.chunks.push(Self::new_chunk());
            self.offset = 0;
        }
        let chunk = self.chunks.last_mut().unwrap();
        chunk[self.offset] = value;
        let ptr = &mut chunk[self.offset] as *mut Value;
        self.offset += 1;
        ptr
    }

    /// Reset the arena, allowing all memory to be reused.
    /// Existing pointers become invalid after this call.
    pub fn reset(&mut self) {
        // Keep the first chunk, drop the rest
        self.chunks.truncate(1);
        self.offset = 0;
    }

    fn new_chunk() -> Box<[Value]> {
        let mut v = Vec::with_capacity(CHUNK_SIZE);
        v.resize(CHUNK_SIZE, Value::None);
        v.into_boxed_slice()
    }
}

impl Default for ValueArena {
    fn default() -> Self {
        Self::new()
    }
}
