### Fixed
- Native backend (aether reuse pass): an effectful call bound and returned bare
  in tail position after a dropped binding — `drop x in (let r = f(...); r)` —
  was lowered to **two** calls (the first result discarded), double-executing
  `f`. The reuse-token threader `rewrite_drop_body_with_env`
  ([src/aether/reuse_analysis.rs](../src/aether/reuse_analysis.rs)) recorded the
  let binding as an alias (`r → rhs`) and, on reaching the bare `Var(r)` tail,
  substituted the aliased rhs into the body position while the enclosing
  `let r = rhs` binding was retained — yielding `let r = rhs in rhs`. It now only
  returns the substituted alias when following it actually produces a reuse;
  otherwise the tail stays `Var(r)`, so the call is emitted once. The legitimate
  precompute-let-to-`Con` reuse path is unaffected. Resolves known issue KI-2.
