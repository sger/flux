//! Evidence vector mirroring the C runtime's `EvvArray` (Proposal 0162 Phase 3).
//!
//! The evidence vector tracks installed handlers so `perform` can locate the
//! correct target without walking a separate handler stack. Each entry bundles
//! an effect tag, a fresh marker, the operation-clause closures, and the
//! parent-vector pointer that must be restored when the handler unwinds.
//!
//! This is the VM-side mirror of `runtime/c/effects.c`'s `EvvArray`. The shape
//! matches the C layout entry-for-entry so both backends dispatch through the
//! same algorithm, but entries are stored using VM-native `Value` types rather
//! than NaN-boxed `i64`s.
//!
//! Entries are appended (not sorted); lookup scans from the end so the most
//! recently installed handler wins — matching the C runtime's `evv_lookup`.

use std::rc::Rc;

use crate::runtime::handler_arm::HandlerArm;
use crate::syntax::Identifier;

/// One evidence entry — the VM mirror of `EvvArray`'s 4-word entry.
///
/// Field order matches the C runtime (`EVV_HTAG_OFF`, `EVV_MARKER_OFF`,
/// `EVV_HANDLER_OFF`, `EVV_PARENT_OFF`). The C layout uses tagged `i64`s;
/// here we use VM-native representations since the VM doesn't NaN-box.
#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    /// Effect identifier (mirrors C `htag`).
    pub effect: Identifier,
    /// Fresh monotonically-increasing id uniquely identifying this handler
    /// instance. Mirrors C `marker` (C stores as tagged int).
    pub marker: u32,
    /// Operation clauses for this handler. Mirrors C `handler` (C stores a
    /// single clause closure; the VM keeps the per-op arm list because the VM
    /// dispatches by op name).
    pub arms: Rc<Vec<HandlerArm>>,
    /// Parent evidence vector (what to restore on unwind). `None` when this
    /// was installed into the empty root vector. Mirrors C `parent_evv`.
    pub parent: Option<EvidenceVector>,
}

/// Evidence vector — the VM mirror of `evv_box(EvvArray *)`.
///
/// Stored as `Rc<Vec<Evidence>>` so installation produces a fresh vector
/// without mutating the parent (the C runtime `memcpy`s the old entries).
/// Cloning an `EvidenceVector` is cheap (single `Rc` bump).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EvidenceVector {
    entries: Rc<Vec<Evidence>>,
}

impl EvidenceVector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append a new entry, returning a fresh vector. Mirrors C `flux_evv_insert`:
    /// copies existing entries and appends the new one at the end.
    pub fn insert(
        &self,
        effect: Identifier,
        marker: u32,
        arms: Rc<Vec<HandlerArm>>,
    ) -> EvidenceVector {
        let parent = if self.is_empty() {
            None
        } else {
            Some(self.clone())
        };
        let mut new_entries = Vec::with_capacity(self.entries.len() + 1);
        new_entries.extend_from_slice(&self.entries);
        new_entries.push(Evidence {
            effect,
            marker,
            arms,
            parent,
        });
        EvidenceVector {
            entries: Rc::new(new_entries),
        }
    }

    /// Find the most recently installed evidence entry for `effect`, scanning
    /// from the end. Returns the entry index or `None` if absent.
    ///
    /// Mirrors C `evv_lookup`.
    pub fn lookup(&self, effect: Identifier) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|entry| entry.effect == effect)
    }

    pub fn get(&self, index: usize) -> Option<&Evidence> {
        self.entries.get(index)
    }
}

/// Allocate a fresh marker. Mirrors C `flux_fresh_marker`. Single-threaded;
/// matches the rest of the VM's state model.
pub fn fresh_marker(counter: &mut u32) -> u32 {
    *counter = counter.wrapping_add(1);
    *counter
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::interner::Interner;

    fn effect_id(interner: &mut Interner, name: &str) -> Identifier {
        interner.intern(name)
    }

    #[test]
    fn insert_appends_and_lookup_finds_most_recent() {
        let mut interner = Interner::new();
        let console = effect_id(&mut interner, "Console");
        let clock = effect_id(&mut interner, "Clock");

        let arms = Rc::new(Vec::new());
        let evv = EvidenceVector::new();
        assert!(evv.is_empty());

        let evv = evv.insert(console, 1, arms.clone());
        let evv = evv.insert(clock, 2, arms.clone());
        let evv = evv.insert(console, 3, arms.clone());

        assert_eq!(evv.len(), 3);
        let idx = evv.lookup(console).expect("console installed");
        assert_eq!(evv.get(idx).unwrap().marker, 3, "most recent wins");
        let idx = evv.lookup(clock).expect("clock installed");
        assert_eq!(evv.get(idx).unwrap().marker, 2);
    }

    #[test]
    fn fresh_marker_monotonic() {
        let mut counter = 0u32;
        assert_eq!(fresh_marker(&mut counter), 1);
        assert_eq!(fresh_marker(&mut counter), 2);
        assert_eq!(fresh_marker(&mut counter), 3);
    }
}
