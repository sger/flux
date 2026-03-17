use std::fmt;

use crate::runtime::{
    BaseFn, BorrowedBaseFn, RuntimeContext, base::BaseHmSignatureId, value::Value,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseFunctionArgMode {
    OwnedOnly,
    BorrowedOnly,
    BorrowedPreferredWithOwnedFallback,
}

#[derive(Clone, Copy)]
enum BaseFunctionImpl {
    Owned(BaseFn),
    Borrowed(BorrowedBaseFn),
    BorrowedPreferred {
        borrowed: BorrowedBaseFn,
        owned: BaseFn,
    },
}

#[derive(Clone)]
pub struct BaseFunction {
    pub name: &'static str,
    implementation: BaseFunctionImpl,
    pub arg_mode: BaseFunctionArgMode,
    pub hm_signature: BaseHmSignatureId,
}

impl BaseFunction {
    pub const fn owned(name: &'static str, hm_signature: BaseHmSignatureId, func: BaseFn) -> Self {
        Self {
            name,
            implementation: BaseFunctionImpl::Owned(func),
            arg_mode: BaseFunctionArgMode::OwnedOnly,
            hm_signature,
        }
    }

    pub const fn borrowed(
        name: &'static str,
        hm_signature: BaseHmSignatureId,
        func: BorrowedBaseFn,
    ) -> Self {
        Self {
            name,
            implementation: BaseFunctionImpl::Borrowed(func),
            arg_mode: BaseFunctionArgMode::BorrowedOnly,
            hm_signature,
        }
    }

    pub const fn preferred(
        name: &'static str,
        hm_signature: BaseHmSignatureId,
        borrowed: BorrowedBaseFn,
        owned: BaseFn,
    ) -> Self {
        Self {
            name,
            implementation: BaseFunctionImpl::BorrowedPreferred { borrowed, owned },
            arg_mode: BaseFunctionArgMode::BorrowedPreferredWithOwnedFallback,
            hm_signature,
        }
    }

    pub fn call_owned(
        &self,
        ctx: &mut dyn RuntimeContext,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        match self.implementation {
            BaseFunctionImpl::Owned(func) => func(ctx, args),
            BaseFunctionImpl::Borrowed(func) => {
                let borrowed: Vec<&Value> = args.iter().collect();
                func(ctx, &borrowed)
            }
            BaseFunctionImpl::BorrowedPreferred { owned, .. } => owned(ctx, args),
        }
    }

    /// Call this base function with NaN-boxed arguments, returning a NaN-boxed result.
    ///
    /// This is the Phase 4 entry point used by the VM when the `nan-boxing` feature
    /// is enabled. The default bridge decodes args to [`Value`] and re-encodes the
    /// result; individual hot functions can be migrated to operate on [`NanBox`]
    /// directly by overriding this via a dedicated `NanBoxedBaseFn` pointer in future.
    ///
    /// Keeping the bridge here means the call site in `function_call.rs` is always
    /// allocation-free for the arg-collection step (raw slots, no decode), while the
    /// per-function decode is deferred until the function actually needs the value.
    #[cfg(feature = "nan-boxing")]
    pub fn call_owned_nanboxed(
        &self,
        ctx: &mut dyn RuntimeContext,
        args: Vec<crate::runtime::nanbox::NanBox>,
    ) -> Result<crate::runtime::nanbox::NanBox, String> {
        use crate::runtime::nanbox::NanBox;
        let value_args: Vec<Value> = args.into_iter().map(NanBox::to_value).collect();
        let result = self.call_owned(ctx, value_args)?;
        Ok(NanBox::from_value(result))
    }

    pub fn call_borrowed(
        &self,
        ctx: &mut dyn RuntimeContext,
        args: &[&Value],
    ) -> Result<Value, String> {
        match self.implementation {
            BaseFunctionImpl::Borrowed(func) => func(ctx, args),
            BaseFunctionImpl::Owned(func) => {
                let owned: Vec<Value> = args.iter().map(|value| (*value).clone()).collect();
                func(ctx, owned)
            }
            BaseFunctionImpl::BorrowedPreferred { borrowed, .. } => borrowed(ctx, args),
        }
    }
}

impl fmt::Debug for BaseFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BaseFunction({})", self.name)
    }
}

impl PartialEq for BaseFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::runtime::{RuntimeContext, gc::GcHeap, value::Value};

    use super::{BaseFunction, BaseFunctionArgMode};

    static OWNED_CALLS: AtomicUsize = AtomicUsize::new(0);
    static BORROWED_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn owned_stub(_ctx: &mut dyn RuntimeContext, _args: Vec<Value>) -> Result<Value, String> {
        OWNED_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Integer(1))
    }

    fn borrowed_stub(_ctx: &mut dyn RuntimeContext, _args: &[&Value]) -> Result<Value, String> {
        BORROWED_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Integer(2))
    }

    struct TestContext {
        heap: GcHeap,
    }

    impl TestContext {
        fn new() -> Self {
            Self {
                heap: GcHeap::new(),
            }
        }
    }

    impl RuntimeContext for TestContext {
        fn invoke_value(&mut self, _callee: Value, _args: Vec<Value>) -> Result<Value, String> {
            Err("unused".to_string())
        }

        fn invoke_base_function_borrowed(
            &mut self,
            _base_fn_index: usize,
            _args: &[&Value],
        ) -> Result<Value, String> {
            Err("unused".to_string())
        }

        fn gc_heap(&self) -> &GcHeap {
            &self.heap
        }

        fn gc_heap_mut(&mut self) -> &mut GcHeap {
            &mut self.heap
        }
    }

    #[test]
    fn borrowed_registry_entries_dispatch_borrowed_path() {
        BORROWED_CALLS.store(0, Ordering::SeqCst);
        let base = BaseFunction::borrowed(
            "borrowed",
            crate::runtime::base::BaseHmSignatureId::Len,
            borrowed_stub,
        );
        let mut ctx = TestContext::new();
        let arg = Value::Integer(7);
        let args = [&arg];
        let result = base.call_borrowed(&mut ctx, &args).unwrap();
        assert_eq!(result, Value::Integer(2));
        assert_eq!(base.arg_mode, BaseFunctionArgMode::BorrowedOnly);
        assert_eq!(BORROWED_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn owned_registry_entries_fallback_clone_for_borrowed_dispatch() {
        OWNED_CALLS.store(0, Ordering::SeqCst);
        let base = BaseFunction::owned(
            "owned",
            crate::runtime::base::BaseHmSignatureId::Len,
            owned_stub,
        );
        let mut ctx = TestContext::new();
        let arg = Value::Integer(7);
        let args = [&arg];
        let result = base.call_borrowed(&mut ctx, &args).unwrap();
        assert_eq!(result, Value::Integer(1));
        assert_eq!(base.arg_mode, BaseFunctionArgMode::OwnedOnly);
        assert_eq!(OWNED_CALLS.load(Ordering::SeqCst), 1);
    }
}
