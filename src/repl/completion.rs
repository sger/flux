//! Tab completion for the interactive REPL (a rustyline [`Helper`]).
//!
//! Mirrors GHCi's two-stage dispatch: a line starting with `:` completes the
//! command name (or, past the command word, that command's argument — file paths
//! for `:load`, an identifier for `:type`); anything else completes an identifier.
//! Identifier candidates come from [`ReplEngine::completion_names`] — the bare
//! names a user can actually type (exposed library members, builtin primops, ADT
//! constructors, session bindings, `it`, keywords) plus fully-qualified names for
//! `.`-prefixed completion. The set is shared with the loop via an
//! `Rc<RefCell<…>>` and refreshed after each entered line.

use std::cell::RefCell;
use std::rc::Rc;

use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

/// The `:`-commands offered for command-name completion (mirrors `handle_command`).
const COMMANDS: &[&str] = &["quit", "reset", "help", "list", "type", "load", "reload"];

/// Word-break characters for identifier extraction. Whitespace, brackets, and
/// operators break a word; **`.` does not**, so a module-qualified name like
/// `Flow.Array.map` is treated as a single word and completes against the
/// qualified candidates.
const BREAK_CHARS: &[char] = &[
    ' ', '\t', '\n', '(', ')', '[', ']', '{', '}', ',', ';', '+', '-', '*', '/', '%', '<', '>',
    '=', '!', '&', '|', ':', '"', '\'', '@', '#', '?', '~', '^',
];

/// The start byte and text of the identifier word ending at `pos`. Scans back to
/// the last break char (UTF-8 safe). `.` is not a break char (qualified names).
fn current_word(line: &str, pos: usize) -> (usize, &str) {
    let start = line[..pos]
        .char_indices()
        .rev()
        .find(|(_, c)| BREAK_CHARS.contains(c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    (start, &line[start..pos])
}

/// rustyline completion helper. Holds the shared, loop-updated in-scope name set
/// and a filename completer for path arguments.
pub(super) struct ReplHelper {
    names: Rc<RefCell<Vec<String>>>,
    filenames: FilenameCompleter,
}

impl ReplHelper {
    pub(super) fn new(names: Rc<RefCell<Vec<String>>>) -> Self {
        Self {
            names,
            filenames: FilenameCompleter::new(),
        }
    }

    /// Complete the identifier word ending at `pos` against the in-scope names.
    /// An empty word yields no candidates (never dump the whole namespace). The
    /// names are pre-sorted/deduped by [`ReplEngine::completion_names`].
    fn complete_identifier(&self, line: &str, pos: usize) -> (usize, Vec<Pair>) {
        let (start, word) = current_word(line, pos);
        if word.is_empty() {
            return (start, Vec::new());
        }
        let candidates = self
            .names
            .borrow()
            .iter()
            .filter(|name| name.starts_with(word))
            .map(|name| Pair {
                display: name.clone(),
                replacement: name.clone(),
            })
            .collect();
        (start, candidates)
    }
}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let prefix = &line[..pos];
        let trimmed = prefix.trim_start();
        if let Some(after_colon) = trimmed.strip_prefix(':') {
            // Still typing the command word (no whitespace yet): complete commands.
            if !after_colon.contains(char::is_whitespace) {
                let candidates = COMMANDS
                    .iter()
                    .filter(|cmd| cmd.starts_with(after_colon))
                    .map(|cmd| Pair {
                        display: (*cmd).to_string(),
                        replacement: (*cmd).to_string(),
                    })
                    .collect();
                // Replace the text after the leading ':'.
                let start = prefix.len() - after_colon.len();
                return Ok((start, candidates));
            }
            // Command + argument: a path for `:load`, an identifier for `:type`.
            let command = after_colon.split_whitespace().next().unwrap_or("");
            return match command {
                "load" => self.filenames.complete(line, pos, ctx),
                "type" | "t" => Ok(self.complete_identifier(line, pos)),
                _ => Ok((pos, Vec::new())),
            };
        }
        Ok(self.complete_identifier(line, pos))
    }
}

// The REPL uses neither inline hints, syntax highlighting, nor input validation
// (multi-line continuation is handled externally via `needs_more_input`), so the
// default no-op impls suffice — but `Helper` requires all four supertraits.
impl Hinter for ReplHelper {
    type Hint = String;
}
impl Highlighter for ReplHelper {}
impl Validator for ReplHelper {}
impl Helper for ReplHelper {}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::history::MemHistory;

    fn helper(names: &[&str]) -> ReplHelper {
        ReplHelper::new(Rc::new(RefCell::new(
            names.iter().map(|s| s.to_string()).collect(),
        )))
    }

    fn displays(pairs: &[Pair]) -> Vec<String> {
        pairs.iter().map(|p| p.display.clone()).collect()
    }

    /// Run `complete` with a throwaway in-memory history context.
    fn complete(h: &ReplHelper, line: &str, pos: usize) -> (usize, Vec<Pair>) {
        let history = MemHistory::new();
        let ctx = Context::new(&history);
        h.complete(line, pos, &ctx).expect("complete")
    }

    #[test]
    fn completes_command_name_after_colon() {
        let (start, out) = complete(&helper(&[]), ":lo", 3);
        assert_eq!(start, 1, "replacement starts right after the ':'");
        assert_eq!(displays(&out), ["load"]);
    }

    #[test]
    fn bare_colon_offers_all_commands() {
        let (start, out) = complete(&helper(&[]), ":", 1);
        assert_eq!(start, 1);
        assert_eq!(out.len(), COMMANDS.len());
    }

    #[test]
    fn completes_identifier_by_prefix() {
        let (start, out) = complete(&helper(&["map", "max", "filter"]), "ma", 2);
        assert_eq!(start, 0);
        assert_eq!(displays(&out), ["map", "max"]);
    }

    #[test]
    fn extracts_word_mid_expression() {
        // `xs |> ma` — the word starts after the space at index 6.
        let (start, out) = complete(&helper(&["map"]), "xs |> ma", 8);
        assert_eq!(start, 6);
        assert_eq!(displays(&out), ["map"]);
    }

    #[test]
    fn dot_is_not_a_break_char_for_qualified_names() {
        let (start, out) = complete(&helper(&["Flow.Array.map", "map"]), "Flow.Array.ma", 13);
        assert_eq!(start, 0, "the whole qualified word, dots included");
        assert_eq!(displays(&out), ["Flow.Array.map"]);
    }

    #[test]
    fn completes_keyword() {
        let (_, out) = complete(&helper(&["match", "max"]), "mat", 3);
        assert_eq!(displays(&out), ["match"]);
    }

    #[test]
    fn empty_word_yields_no_candidates() {
        let (start, out) = complete(&helper(&["map"]), "1 + ", 4);
        assert_eq!(start, 4);
        assert!(out.is_empty(), "must not dump the whole namespace");
    }

    #[test]
    fn type_command_completes_an_identifier() {
        // Names arrive pre-sorted from `completion_names`; the completer preserves order.
        let (start, out) = complete(&helper(&["len", "length"]), ":type len", 9);
        assert_eq!(start, 6, "the identifier starts after `:type `");
        assert_eq!(displays(&out), ["len", "length"]);
    }

    #[test]
    fn load_command_does_not_offer_identifiers() {
        // `:load` dispatches to filename completion, so identifier names never appear.
        let (_, out) = complete(&helper(&["map", "length"]), ":load map", 9);
        assert!(
            !displays(&out).iter().any(|d| d == "map" || d == "length"),
            "load must complete files, not identifiers: {:?}",
            displays(&out)
        );
    }

    #[test]
    fn current_word_is_utf8_safe() {
        // A multibyte char before the word must not split a UTF-8 boundary.
        let line = "λ + ma"; // 'λ' is 2 bytes
        let pos = line.len();
        let (start, word) = current_word(line, pos);
        assert_eq!(word, "ma");
        assert_eq!(&line[start..pos], "ma");
    }
}
