### Added
- The interactive `flux repl` now has **line editing and command history**
  (proposal 0175). On an interactive terminal it runs on `rustyline`: arrow-key
  history, in-line editing (←/→, Ctrl-A/E, word motions), and Ctrl-R reverse
  search, with history persisted to `~/.flux_repl_history` across sessions.
  Multi-line forms still continue onto the `....>` prompt until their delimiters
  balance. Piped or redirected input (scripts, the integration tests) keeps the
  plain line-reader path, so stdout carries only evaluation output and scripted
  sessions stay deterministic.

### Internal
- New `rustyline` dependency. The REPL dispatch loop was factored into a shared
  `dispatch` step driven by two front-ends — an interactive `rustyline` reader
  and the existing piped reader — selected by `io::stdin().is_terminal()`. If the
  editor fails to initialise, the REPL falls back to the piped reader.
