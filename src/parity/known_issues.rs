//! Which KIs in `docs/known_issues.md` are open, so a skip can be held to an
//! open one (proposal 0190 §8).
//!
//! `xtask/src/census/known_issues.rs` reads the same file with the same rule;
//! it lives in another crate, so the rule is repeated here.

use std::collections::BTreeSet;

use super::fixture::FixtureMeta;

/// The ids of the open entries. An entry is resolved when it sits under
/// `## Resolved`, or when its heading says `FIXED` or `NOT REPRODUCIBLE`,
/// which some entries under `## Open` do.
#[must_use]
pub fn open_ids(text: &str) -> BTreeSet<String> {
    let mut open = BTreeSet::new();
    let mut in_resolved = false;
    for line in text.lines() {
        if let Some(section) = line.strip_prefix("## ") {
            in_resolved = section.trim().starts_with("Resolved");
        } else if let Some(heading) = line.strip_prefix("### ")
            && let Some(id) = cited(heading).into_iter().next()
            && heading.starts_with(&id)
        {
            let upper = heading.to_uppercase();
            if !in_resolved && !upper.contains("FIXED") && !upper.contains("NOT REPRODUCIBLE") {
                open.insert(id);
            }
        }
    }
    open
}

/// Every `KI-nnn` in `text`, in order.
pub(crate) fn cited(text: &str) -> Vec<String> {
    text.match_indices("KI-")
        .filter_map(|(at, _)| {
            let digits: String = text[at + 3..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (!digits.is_empty()).then(|| format!("KI-{digits}"))
        })
        .collect()
}

/// One error for each KI that `meta`'s `skip:`, `skip-way:` or `expect-leak:`
/// reasons cite and `open` does not hold. A reason that cites no KI is not
/// checked here.
#[must_use]
pub fn stale_kis(meta: &FixtureMeta, open: &BTreeSet<String>) -> Vec<String> {
    let reasons = meta
        .skip
        .iter()
        .chain(meta.skip_ways.iter().map(|skip| &skip.reason))
        .chain(meta.expected_leaks.iter().map(|leak| &leak.reason));
    let mut stale: Vec<String> = reasons
        .flat_map(|reason| cited(reason))
        .filter(|id| !open.contains(id))
        .collect();
    stale.sort();
    stale.dedup();
    stale
        .into_iter()
        .map(|id| format!("`{id}` is not open in docs/known_issues.md (resolved or missing)"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::{
        Way,
        fixture::{ExpectLeak, SkipWay},
    };

    const DOC: &str = "\
# Known issues
## Open
### KI-102 — Effects leak under --no-cache
See KI-011.
### KI-011 — Old bug — FIXED 2026-01-02
### KI-050 — Gone — NOT REPRODUCIBLE
## Resolved
### KI-038 — Resolved without a marker
";

    #[test]
    fn only_open_entries_count() {
        let open: Vec<String> = open_ids(DOC).into_iter().collect();
        assert_eq!(open, ["KI-102"]);
    }

    #[test]
    fn every_ki_in_a_reason_is_cited() {
        assert_eq!(cited("KI-076 and KI-103; KI- no"), ["KI-076", "KI-103"]);
    }

    #[test]
    fn a_skip_must_cite_an_open_ki() {
        let open = open_ids(DOC);
        let mut meta = FixtureMeta {
            skip: Some("support file".to_string()),
            ..FixtureMeta::default()
        };
        meta.skip_ways.push(SkipWay {
            way: Way::Llvm,
            reason: "KI-102 leak".to_string(),
        });
        assert!(stale_kis(&meta, &open).is_empty());

        meta.skip = Some("KI-038 was here".to_string());
        meta.skip_ways.push(SkipWay {
            way: Way::Vm,
            reason: "KI-999 and KI-011".to_string(),
        });
        let stale = stale_kis(&meta, &open);
        assert_eq!(stale.len(), 3, "{stale:?}");
        assert!(stale[0].starts_with("`KI-011`"));
        assert!(stale[2].starts_with("`KI-999`"));
    }

    #[test]
    fn an_expect_leak_must_cite_an_open_ki() {
        let open = open_ids(DOC);
        let leak = |reason: &str| ExpectLeak {
            way: Way::LlvmHeap,
            at_most: 1,
            reason: reason.to_string(),
            folder_wide: true,
        };
        let mut meta = FixtureMeta::default();
        meta.expected_leaks.push(leak("KI-102 a tuple"));
        assert!(stale_kis(&meta, &open).is_empty());

        meta.expected_leaks.push(leak("KI-011 fixed since"));
        let stale = stale_kis(&meta, &open);
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert!(stale[0].starts_with("`KI-011`"));
    }
}
