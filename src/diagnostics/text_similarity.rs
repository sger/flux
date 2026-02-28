//! Text-similarity utilities shared by diagnostics and suggestion paths.

/// Compute Levenshtein edit distance between two strings.
///
/// Returns the minimum number of single-character edits (insertions,
/// deletions, substitutions) required to transform `a` into `b`.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_len = a.chars().count();
    let b_len = b.chars().count();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0; b_len + 1]; a_len + 1];

    // Initialize first column and row.
    for (i, row) in matrix.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in matrix[0].iter_mut().enumerate() {
        *cell = j;
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    for (i, &a_char) in a_chars.iter().enumerate() {
        for (j, &b_char) in b_chars.iter().enumerate() {
            let cost = if a_char == b_char { 0 } else { 1 };
            matrix[i + 1][j + 1] = *[
                matrix[i][j + 1] + 1,
                matrix[i + 1][j] + 1,
                matrix[i][j] + cost,
            ]
            .iter()
            .min()
            .expect("distance candidates are non-empty");
        }
    }

    matrix[a_len][b_len]
}

#[cfg(test)]
mod tests {
    use super::levenshtein_distance;

    #[test]
    fn distance_empty_strings() {
        assert_eq!(levenshtein_distance("", ""), 0);
    }

    #[test]
    fn distance_exact_match() {
        assert_eq!(levenshtein_distance("cat", "cat"), 0);
    }

    #[test]
    fn distance_single_edit() {
        assert_eq!(levenshtein_distance("cat", "cats"), 1);
        assert_eq!(levenshtein_distance("cat", "cut"), 1);
    }

    #[test]
    fn distance_known_case() {
        assert_eq!(levenshtein_distance("saturday", "sunday"), 3);
    }
}
