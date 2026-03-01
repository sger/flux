//! Suggestion system for compiler diagnostics
//!
//! Provides fuzzy matching and "did you mean?" suggestions for undefined identifiers.

use crate::diagnostics::text_similarity::levenshtein_distance;

/// Find similar strings from a list of candidates
///
/// Returns up to `max_suggestions` strings that are similar to `target`,
/// sorted by similarity (most similar first).
///
/// # Algorithm
/// - Exact match (case-insensitive): distance 0
/// - Very close match (1-2 char difference): included
/// - Moderate match (≤30% of length): included if short enough
/// - Prefix match: bonus points
pub fn find_similar_names(
    target: &str,
    candidates: &[String],
    max_suggestions: usize,
) -> Vec<String> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let target_lower = target.to_lowercase();
    let target_len = target.chars().count();

    // Calculate similarity scores for all candidates
    let mut scored: Vec<(String, usize, bool)> = candidates
        .iter()
        .filter_map(|candidate| {
            let candidate_lower = candidate.to_lowercase();

            // Exact match (case-insensitive) - skip it
            if target_lower == candidate_lower {
                return None;
            }

            let distance = levenshtein_distance(&target_lower, &candidate_lower);
            let is_prefix = candidate_lower.starts_with(&target_lower)
                || target_lower.starts_with(&candidate_lower);

            // Filter out very different strings
            let max_distance = if target_len <= 3 {
                1 // For short names, only allow distance of 1
            } else if target_len <= 6 {
                2 // For medium names, allow distance of 2
            } else {
                3.max(target_len / 3) // For longer names, allow up to 30% difference
            };

            if distance <= max_distance || is_prefix {
                Some((candidate.clone(), distance, is_prefix))
            } else {
                None
            }
        })
        .collect();

    // Sort by:
    // 1. Prefix matches first
    // 2. Then by edit distance (ascending)
    // 3. Then alphabetically
    scored.sort_by(|a, b| {
        b.2.cmp(&a.2) // Prefix matches first (true > false)
            .then_with(|| a.1.cmp(&b.1)) // Then by distance
            .then_with(|| a.0.cmp(&b.0)) // Then alphabetically
    });

    // Return top suggestions
    scored
        .into_iter()
        .take(max_suggestions)
        .map(|(name, _, _)| name)
        .collect()
}

const KNOWN_EFFECT_NAMES: &[&str] = &["IO", "Time", "State"];

/// Suggest a known built-in effect name for unknown-effect diagnostics.
pub fn suggest_effect_name(name: &str) -> Option<String> {
    if KNOWN_EFFECT_NAMES.contains(&name) || name.is_empty() {
        return None;
    }

    let candidates: Vec<String> = KNOWN_EFFECT_NAMES
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let suggestion = find_similar_names(name, &candidates, 1)
        .into_iter()
        .next()?;
    Some(format!("did you mean `{}`?", suggestion))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_similar_names_typo() {
        let candidates = vec![
            "count".to_string(),
            "amount".to_string(),
            "discount".to_string(),
        ];
        let suggestions = find_similar_names("cound", &candidates, 3);

        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0], "count"); // Closest match
    }

    #[test]
    fn test_find_similar_names_prefix() {
        let candidates = vec![
            "variable".to_string(),
            "value".to_string(),
            "var".to_string(),
        ];
        let suggestions = find_similar_names("val", &candidates, 3);

        assert!(!suggestions.is_empty());
        // Should prioritize prefix matches
        assert!(
            suggestions.contains(&"value".to_string())
                || suggestions.contains(&"variable".to_string())
        );
    }

    #[test]
    fn test_find_similar_names_no_exact_match() {
        let candidates = vec!["test".to_string(), "testing".to_string()];
        let suggestions = find_similar_names("test", &candidates, 3);

        // Should not include exact matches
        assert!(!suggestions.contains(&"test".to_string()));
    }

    #[test]
    fn test_find_similar_names_limit() {
        let candidates = vec![
            "alpha".to_string(),
            "aleph".to_string(),
            "alps".to_string(),
            "alt".to_string(),
        ];
        let suggestions = find_similar_names("alp", &candidates, 2);

        assert!(suggestions.len() <= 2);
    }

    #[test]
    fn test_suggest_effect_name_prefix() {
        let suggestion = suggest_effect_name("I");
        assert_eq!(suggestion.as_deref(), Some("did you mean `IO`?"));
    }
}
