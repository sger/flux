### Added
- `flux-lsp`: call hierarchy — `textDocument/prepareCallHierarchy`,
  `callHierarchy/incomingCalls`, and `callHierarchy/outgoingCalls`. Put the
  cursor on a function (its declaration or any call site) and the editor shows
  who calls it (incoming) and what it calls (outgoing), navigable across the
  module-graph component. Calls made inside an anonymous lambda are attributed
  to the nearest enclosing named function, the same way rust-analyzer folds
  closure bodies into their host. Both qualified calls (`Module.foo(..)`) and
  direct calls (`foo(..)`) are tracked; only calls that resolve to a function
  the workspace actually declares are listed as edges. `handlers/call_hierarchy.rs`
  shares one extraction pass across all three requests and follows the existing
  gather-on-main / compute-off-thread split, so the AST walks run on the worker.
