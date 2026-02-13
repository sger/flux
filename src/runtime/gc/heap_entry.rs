use crate::runtime::gc::heap_object::HeapObject;

pub struct HeapEntry {
    object: HeapObject,
    marked: bool,
}
