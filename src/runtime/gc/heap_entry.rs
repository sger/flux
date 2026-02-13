use crate::runtime::gc::heap_object::HeapObject;

pub struct HeapEntry {
    pub object: HeapObject,
    pub marked: bool,
}
