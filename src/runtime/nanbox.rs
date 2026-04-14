//! NaN-boxing runtime value representation.
//!
//! Encodes all Flux [`Value`] variants in exactly 8 bytes by exploiting the
//! unused quiet-NaN bit patterns of IEEE 754 doubles.
//!
//! # Bit layout
//!
//! ```text
//! IEEE 754 double (64 bits):
//!   [63]     sign
//!   [62:52]  exponent (11 bits)
//!   [51:0]   mantissa (52 bits)
//!
//! A double is NaN when exponent = all-1s and mantissa ≠ 0.
//! A quiet NaN additionally has bit 51 = 1.
//!
//! NaN-box sentinel (marks "this is not a float"):
//!   bits [63:50] = 0x7FFD_  (sign=0, exp=0x7FF, quiet=1, box=1)
//!   i.e.  0x7FFD_0000_0000_0000
//!
//! Any double whose top 14 bits are NOT 0x7FFD is stored as-is (raw f64 bits).
//! The canonical IEEE NaN (0x7FF8_0000_0000_0000) is therefore a valid float,
//! not a NaN-box.
//!
//! When bits [63:50] == 0x7FFD:
//!   bits [49:46]  = NanTag (4 bits, 16 possible tags)
//!   bits [45:0]   = payload (46 bits)
//! ```
//!
//! # Integer range
//!
//! Inline integers use a 46-bit signed two's-complement payload, covering
//! ±35_184_372_088_831. Integers outside this range are heap-allocated via
//! [`NanTag::BoxedValue`] exactly like any other heap Value.
//!
//! # Heap values
//!
//! All [`Value`] variants containing `Rc<T>` fields are encoded as
//! [`NanTag::BoxedValue`]: the full `Value` is placed behind an `Rc<Value>`,
//! and the raw pointer (shifted right 3 bits for 8-byte alignment) is stored
//! in the payload. [`NanBox`]'s `Clone` and `Drop` impls manually manage the
//! `Rc` strong count via unsafe pointer operations.
//!
//! This approach is deliberately conservative for Phase 1: correctness and
//! round-trip fidelity are the primary goals. Dedicated per-type pointer tags
//! (to avoid the double-indirection for common heap types) are deferred to a
//! later phase once the encoding is proven stable.

mod inner {
    use std::mem::ManuallyDrop;
    use std::rc::Rc;

    use crate::runtime::value::Value;

    // ── Bit-layout constants ──────────────────────────────────────────────────

    /// Bits [63:50] that mark a NaN-box (not a real float).
    ///
    /// = sign=0, exp=0x7FF (bits 62:52 all-1), quiet-NaN=1 (bit 51), box=1 (bit 50)
    /// = 0b 0_11111111111_11_00…00 = 0x7FFC_0000_0000_0000
    ///
    /// `NANBOX_SENTINEL` has bits set ONLY in positions [63:50]. This is required
    /// so that `(NANBOX_SENTINEL & SENTINEL_MASK) == NANBOX_SENTINEL` (self-consistent).
    ///
    /// The canonical quiet NaN (0x7FF8_…) has bit 50 = 0 → it does NOT match
    /// the sentinel → floats that are NaN are stored as-is (canonicalized in
    /// `from_float`).
    const NANBOX_SENTINEL: u64 = 0x7FFC_0000_0000_0000;

    /// Mask for bits [63:50]: the fixed region of every NaN-box.
    ///
    /// The mask covers exactly the bits that are fixed in `NANBOX_SENTINEL`.
    /// Tag bits [49:46] and payload bits [45:0] are intentionally excluded.
    const SENTINEL_MASK: u64 = 0xFFFC_0000_0000_0000;

    /// Bit position of the lowest tag bit.
    const TAG_SHIFT: u32 = 46;

    /// 4-bit tag mask (applied after shifting).
    const TAG_MASK: u64 = 0xF;

    /// 46-bit payload mask.
    const PAYLOAD_MASK: u64 = (1u64 << 46) - 1;

    /// Minimum value representable as an inline 46-bit signed integer.
    pub const MIN_INLINE_INT: i64 = -(1i64 << 45); // -35_184_372_088_832

    /// Maximum value representable as an inline 46-bit signed integer.
    pub const MAX_INLINE_INT: i64 = (1i64 << 45) - 1; // 35_184_372_088_831

    // ── Tag enum ─────────────────────────────────────────────────────────────

    /// 4-bit discriminant embedded in a NaN-box.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum NanTag {
        /// 46-bit signed integer (inline).
        Integer = 0x0,
        /// Boolean: payload 0 = false, 1 = true.
        Boolean = 0x1,
        /// `Value::None`.
        None = 0x2,
        /// `Value::Uninit` — VM-internal stack sentinel.
        Uninit = 0x3,
        /// `Value::EmptyList`.
        EmptyList = 0x4,
        // 0x5 was BaseFunction (removed). Tag value reserved.
        /// Trampoline thunk for mutual tail-call optimization (llvm only).
        /// Payload holds a heap pointer (>> 3) to `{i8 fn_index, i8[3] _pad, i32 nargs, i64 args[]}`.
        Thunk = 0x6,
        /// Heap-allocated `Rc<Value>` for any Value variant not encoded inline.
        /// Payload holds the `Rc<Value>` raw pointer shifted right 3 bits.
        BoxedValue = 0x8,
        // Tags 0x9–0xF reserved for future dedicated pointer types.
    }

    impl NanTag {
        #[inline(always)]
        fn from_u8(v: u8) -> Self {
            match v {
                0x0 => NanTag::Integer,
                0x1 => NanTag::Boolean,
                0x2 => NanTag::None,
                0x3 => NanTag::Uninit,
                0x4 => NanTag::EmptyList,
                0x5 => NanTag::BoxedValue, // was BaseFunction (removed)
                0x6 => NanTag::Thunk,
                0x7 | 0x8 => NanTag::BoxedValue,
                _ => NanTag::BoxedValue, // reserved tags fall back to boxed
            }
        }
    }

    // ── NanBox ────────────────────────────────────────────────────────────────

    /// An 8-byte NaN-boxed runtime value.
    ///
    /// Every Flux [`Value`] variant can be round-tripped through `NanBox` via
    /// [`NanBox::from_value`] / [`NanBox::to_value`] without observable change.
    ///
    /// ## Safety invariant
    ///
    /// When the tag is [`NanTag::BoxedValue`], the payload encodes a raw
    /// `*const Value` pointer shifted right 3 bits. The `NanBox` holds a
    /// strong reference to that `Rc<Value>`. `Clone` increments the count;
    /// `Drop` decrements it. No other code may create or destroy `Rc` counts
    /// for a NaN-box pointer except through these impls.
    #[derive(Debug)]
    #[repr(transparent)]
    pub struct NanBox(u64);

    // ── Internal constructors ─────────────────────────────────────────────────

    impl NanBox {
        /// Encode a tag + 46-bit payload as a NaN-box.
        #[inline(always)]
        fn from_tag_payload(tag: NanTag, payload: u64) -> Self {
            debug_assert!(payload <= PAYLOAD_MASK, "payload exceeds 46 bits");
            NanBox(NANBOX_SENTINEL | ((tag as u64) << TAG_SHIFT) | payload)
        }

        /// Check whether this NaN-box encodes a heap pointer (BoxedValue).
        #[inline(always)]
        fn is_heap_pointer(&self) -> bool {
            !self.is_float() && self.tag() == NanTag::BoxedValue
        }

        /// Recover the raw `*const Value` pointer from a BoxedValue payload.
        ///
        /// # Safety
        /// Must only be called when `tag() == NanTag::BoxedValue`.
        #[inline(always)]
        unsafe fn as_raw_ptr(&self) -> *const Value {
            debug_assert_eq!(self.tag(), NanTag::BoxedValue);
            ((self.0 & PAYLOAD_MASK) << 3) as *const Value
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    impl NanBox {
        /// Returns `true` if this value is a raw IEEE 754 double (not a NaN-box).
        #[inline(always)]
        pub fn is_float(&self) -> bool {
            (self.0 & SENTINEL_MASK) != NANBOX_SENTINEL
        }

        /// Returns the 4-bit [`NanTag`] discriminant.
        ///
        /// # Panics (debug)
        /// Panics if called on a float NaN-box (i.e., `is_float()` is true).
        #[inline(always)]
        pub fn tag(&self) -> NanTag {
            debug_assert!(!self.is_float(), "tag() called on a float NanBox");
            NanTag::from_u8(((self.0 >> TAG_SHIFT) & TAG_MASK) as u8)
        }

        /// Returns the 46-bit payload.
        #[inline(always)]
        pub fn payload(&self) -> u64 {
            self.0 & PAYLOAD_MASK
        }

        /// Encode a `f64`.
        ///
        /// All NaN values are canonicalized to `f64::NAN` (0x7FF8_0000_0000_0000)
        /// because some quiet-NaN bit patterns with bit 50 = 1 would collide with
        /// the NaN-box sentinel (0x7FFC_…). Flux semantics do not distinguish
        /// between NaN bit patterns, so canonicalization is lossless.
        ///
        /// Valid non-NaN floats (including ±infinity) are stored as-is.
        #[inline(always)]
        pub fn from_float(f: f64) -> Self {
            if f.is_nan() {
                // Canonical quiet NaN: 0x7FF8_0000_0000_0000.
                // This has bit50 = 0, so it never matches the NaN-box sentinel.
                NanBox(f64::NAN.to_bits())
            } else {
                NanBox(f.to_bits())
            }
        }

        /// Encode a 64-bit signed integer.
        ///
        /// Integers in `[MIN_INLINE_INT, MAX_INLINE_INT]` are stored inline.
        /// Integers outside that range are heap-boxed as `Rc<Value>`.
        #[inline]
        pub fn from_int(n: i64) -> Self {
            if (MIN_INLINE_INT..=MAX_INLINE_INT).contains(&n) {
                // Sign-extend into 46 bits: mask to 46 bits.
                let payload = (n as u64) & PAYLOAD_MASK;
                NanBox::from_tag_payload(NanTag::Integer, payload)
            } else {
                NanBox::box_value(Value::Integer(n))
            }
        }

        /// Encode a boolean.
        #[inline(always)]
        pub fn from_bool(b: bool) -> Self {
            NanBox::from_tag_payload(NanTag::Boolean, b as u64)
        }

        /// Encode `Value::None`.
        #[inline(always)]
        pub fn from_none() -> Self {
            NanBox::from_tag_payload(NanTag::None, 0)
        }

        /// Encode `Value::Uninit`.
        #[inline(always)]
        pub fn from_uninit() -> Self {
            NanBox::from_tag_payload(NanTag::Uninit, 0)
        }

        /// Encode `Value::EmptyList`.
        #[inline(always)]
        pub fn from_empty_list() -> Self {
            NanBox::from_tag_payload(NanTag::EmptyList, 0)
        }

        /// Box any [`Value`] behind an `Rc<Value>` and encode the pointer.
        ///
        /// This is the fallback path for all heap-allocated Value variants
        /// (String, Array, Closure, Adt, etc.) and for integers outside the
        /// 46-bit inline range.
        fn box_value(v: Value) -> Self {
            let rc = Rc::new(v);
            let raw = Rc::into_raw(rc); // Rc strong count = 1; we "own" it via the NanBox
            let addr = raw as u64;
            // Pointers are 8-byte aligned → bottom 3 bits are 0 → shift right 3
            debug_assert_eq!(addr & 0x7, 0, "Rc pointer is not 8-byte aligned");
            let shifted = addr >> 3;
            debug_assert!(
                shifted <= PAYLOAD_MASK,
                "pointer exceeds 46-bit payload range"
            );
            NanBox::from_tag_payload(NanTag::BoxedValue, shifted)
        }

        // ── Decoding ─────────────────────────────────────────────────────────

        /// Decode as `f64`. Only valid when `is_float()` is true.
        #[inline(always)]
        pub fn as_float(&self) -> f64 {
            debug_assert!(self.is_float());
            f64::from_bits(self.0)
        }

        /// Decode as a signed integer.
        ///
        /// Sign-extends the 46-bit payload to 64 bits.
        #[inline(always)]
        pub fn as_int(&self) -> i64 {
            debug_assert_eq!(self.tag(), NanTag::Integer);
            // Sign-extend: shift left to put sign bit at top, then arithmetic shift right
            let payload = (self.0 & PAYLOAD_MASK) as i64;
            (payload << (64 - 46)) >> (64 - 46)
        }

        /// Decode as a boolean.
        #[inline(always)]
        pub fn as_bool(&self) -> bool {
            debug_assert_eq!(self.tag(), NanTag::Boolean);
            self.payload() != 0
        }

        // ── Value conversion ──────────────────────────────────────────────────

        /// Encode a [`Value`] as a [`NanBox`].
        ///
        /// All Value variants are handled. Immediate types (Int, Float, Bool,
        /// None, Uninit, EmptyList, GcHandle) are encoded inline. All other
        /// variants are heap-boxed via [`NanBox::box_value`].
        pub fn from_value(v: Value) -> Self {
            match v {
                Value::Float(f) => NanBox::from_float(f),
                Value::Integer(n) => NanBox::from_int(n),
                Value::Boolean(b) => NanBox::from_bool(b),
                Value::None => NanBox::from_none(),
                Value::Uninit => NanBox::from_uninit(),
                Value::EmptyList => NanBox::from_empty_list(),
                // Everything else goes through the boxed path.
                other => NanBox::box_value(other),
            }
        }

        /// Decode a [`NanBox`] back into a [`Value`], consuming the NanBox.
        ///
        /// The returned Value is a full clone of the original (for heap variants
        /// this means incrementing the inner Rc count and then dropping the
        /// NanBox's reference).
        pub fn to_value(self) -> Value {
            // Use ManuallyDrop to prevent NanBox's Drop from running — we will
            // manually handle the Rc lifetime below.
            let nb = ManuallyDrop::new(self);
            unsafe { nb.decode() }
        }

        /// Internal decode: must be called on a `ManuallyDrop<NanBox>` or when
        /// the caller guarantees the Rc lifetime is handled externally.
        ///
        /// # Safety
        /// Caller must ensure that either:
        /// - `self` is wrapped in `ManuallyDrop` (so Drop won't run), or
        /// - The BoxedValue Rc count has been pre-incremented.
        unsafe fn decode(&self) -> Value {
            if self.is_float() {
                return Value::Float(self.as_float());
            }
            match self.tag() {
                NanTag::Integer => Value::Integer(self.as_int()),
                NanTag::Boolean => Value::Boolean(self.as_bool()),
                NanTag::None => Value::None,
                NanTag::Uninit => Value::Uninit,
                NanTag::EmptyList => Value::EmptyList,
                NanTag::Thunk => {
                    // Thunks are llvm-only trampoline values. They should
                    // never appear in the VM. Treat as None if encountered.
                    Value::None
                }
                NanTag::BoxedValue => {
                    // Reconstruct the Rc. `from_raw` takes ownership, which will
                    // drop the Rc (decrementing count) at end of this block.
                    // We clone the inner Value first, so the caller gets a proper
                    // owned Value with its own Rc references.
                    unsafe {
                        let ptr = self.as_raw_ptr();
                        let rc = Rc::from_raw(ptr);
                        (*rc).clone()
                        // rc drops here → decrements Rc count (this is the NanBox's "drop")
                    }
                }
            }
        }
    }

    // ── Clone ─────────────────────────────────────────────────────────────────

    impl Clone for NanBox {
        fn clone(&self) -> Self {
            if self.is_heap_pointer() {
                // Increment the Rc strong count without constructing an Rc handle.
                unsafe {
                    Rc::increment_strong_count(self.as_raw_ptr());
                }
            }
            NanBox(self.0)
        }
    }

    // ── Drop ──────────────────────────────────────────────────────────────────

    impl Drop for NanBox {
        fn drop(&mut self) {
            if self.is_heap_pointer() {
                // Decrement the Rc strong count. If it reaches zero, the inner
                // Value (and all its own Rc fields) will be dropped automatically.
                unsafe {
                    Rc::decrement_strong_count(self.as_raw_ptr());
                }
            }
        }
    }

    // ── PartialEq ─────────────────────────────────────────────────────────────

    impl PartialEq for NanBox {
        fn eq(&self, other: &Self) -> bool {
            // Bit-identical NaN-boxes are equal, *except* that NaN != NaN per
            // IEEE 754. For floats we delegate to f64's PartialEq.
            if self.is_float() && other.is_float() {
                return self.as_float() == other.as_float();
            }
            // For NaN-boxed values, bit identity suffices: two NanBoxes with
            // the same bits encode the same value (same tag + same payload, or
            // same Rc pointer for BoxedValue).
            self.0 == other.0
        }
    }

    // ── Display ───────────────────────────────────────────────────────────────

    impl std::fmt::Display for NanBox {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // Convert to Value for display.
            write!(f, "{}", self.clone().to_value())
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[cfg(test)]
    mod tests {
        use std::rc::Rc;

        use crate::runtime::value::{AdtFields, AdtValue, Value};

        use super::*;

        fn roundtrip(v: Value) -> Value {
            NanBox::from_value(v).to_value()
        }

        #[test]
        fn nanbox_size_is_8_bytes() {
            assert_eq!(std::mem::size_of::<NanBox>(), 8);
        }

        #[test]
        fn float_roundtrip() {
            assert_eq!(
                roundtrip(Value::Float(std::f64::consts::PI)),
                Value::Float(std::f64::consts::PI)
            );
            assert_eq!(roundtrip(Value::Float(0.0)), Value::Float(0.0));
            assert_eq!(roundtrip(Value::Float(-1.5)), Value::Float(-1.5));
            // NaN round-trips as NaN (PartialEq returns false for NaN, so check bits)
            let nan_in = Value::Float(f64::NAN);
            let nan_out = roundtrip(nan_in);
            assert!(matches!(nan_out, Value::Float(f) if f.is_nan()));
        }

        #[test]
        fn float_is_not_nanbox() {
            let nb = NanBox::from_float(1.0);
            assert!(nb.is_float());
            let nb_nan = NanBox::from_float(f64::NAN);
            assert!(nb_nan.is_float());
        }

        #[test]
        fn integer_inline_roundtrip() {
            for n in [0i64, 1, -1, 42, -42, MIN_INLINE_INT, MAX_INLINE_INT] {
                let nb = NanBox::from_int(n);
                assert!(!nb.is_float());
                assert_eq!(nb.tag(), NanTag::Integer);
                assert_eq!(nb.as_int(), n, "failed for n={}", n);
                assert_eq!(roundtrip(Value::Integer(n)), Value::Integer(n));
            }
        }

        #[test]
        fn integer_overflow_roundtrip() {
            // Integers outside the 46-bit range go through BoxedValue.
            let big = MAX_INLINE_INT + 1;
            let small = MIN_INLINE_INT - 1;
            assert_eq!(roundtrip(Value::Integer(big)), Value::Integer(big));
            assert_eq!(roundtrip(Value::Integer(small)), Value::Integer(small));
            assert_eq!(
                roundtrip(Value::Integer(i64::MAX)),
                Value::Integer(i64::MAX)
            );
            assert_eq!(
                roundtrip(Value::Integer(i64::MIN)),
                Value::Integer(i64::MIN)
            );
        }

        #[test]
        fn boolean_roundtrip() {
            assert_eq!(roundtrip(Value::Boolean(true)), Value::Boolean(true));
            assert_eq!(roundtrip(Value::Boolean(false)), Value::Boolean(false));
        }

        #[test]
        fn immediate_variants_roundtrip() {
            assert_eq!(roundtrip(Value::None), Value::None);
            assert_eq!(roundtrip(Value::Uninit), Value::Uninit);
            assert_eq!(roundtrip(Value::EmptyList), Value::EmptyList);
        }

        #[test]
        fn string_roundtrip() {
            let v = Value::String(Rc::new("hello".to_string()));
            assert_eq!(roundtrip(v.clone()), v);
        }

        #[test]
        fn array_roundtrip() {
            let v = Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)]));
            assert_eq!(roundtrip(v.clone()), v);
        }

        #[test]
        fn adt_unit_roundtrip() {
            let v = Value::AdtUnit(Rc::new("Red".to_string()));
            assert_eq!(roundtrip(v.clone()), v);
        }

        #[test]
        fn adt_roundtrip() {
            let v = Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new("Node".to_string()),
                fields: AdtFields::from_vec(vec![Value::Integer(1), Value::Integer(2)]),
            }));
            assert_eq!(roundtrip(v.clone()), v);
        }

        #[test]
        fn some_left_right_roundtrip() {
            let v = Value::Some(Rc::new(Value::Integer(42)));
            assert_eq!(roundtrip(v.clone()), v);
            let l = Value::Left(Rc::new(Value::Boolean(true)));
            assert_eq!(roundtrip(l.clone()), l);
            let r = Value::Right(Rc::new(Value::None));
            assert_eq!(roundtrip(r.clone()), r);
        }

        #[test]
        fn rc_count_after_clone() {
            let original = NanBox::from_value(Value::String(Rc::new("x".to_string())));
            let cloned = original.clone();
            // Both NanBoxes reference the same Rc<Value>; strong count should be 2.
            // Decode to check: each to_value() creates a clone of the inner value
            // and drops the Rc, so after both are decoded the count is back to 0.
            let v1 = original.to_value();
            let v2 = cloned.to_value();
            assert_eq!(v1, v2);
        }

        #[test]
        fn rc_count_after_drop() {
            // Create a boxed value, extract the inner Rc, verify count reaches 1
            // after the NanBox is dropped.
            let inner = Rc::new("sentinel".to_string());
            let v = Value::String(Rc::clone(&inner));
            let nb = NanBox::from_value(v);
            // inner (1) + the Rc<Value> inside the NanBox holds another Rc<String>
            // The NanBox holds an Rc<Value>, not directly the Rc<String>.
            // After dropping nb, the Rc<Value> should be freed, which drops the
            // inner Rc<String> count back to 1 (only `inner` holds a reference).
            drop(nb);
            assert_eq!(Rc::strong_count(&inner), 1);
        }

        #[test]
        fn payload_sign_extension() {
            // -1 should round-trip correctly (all-ones in 46 bits, sign-extended to i64)
            assert_eq!(roundtrip(Value::Integer(-1)), Value::Integer(-1));
            assert_eq!(roundtrip(Value::Integer(-100)), Value::Integer(-100));
            assert_eq!(
                roundtrip(Value::Integer(MIN_INLINE_INT)),
                Value::Integer(MIN_INLINE_INT)
            );
        }
    }
}

pub use inner::{MAX_INLINE_INT, MIN_INLINE_INT, NanBox, NanTag};
