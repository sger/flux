### Fixed
- **LSP: `import Lib exposing (..)` of a sibling/subdir module no longer
  false-flags its members as UNDEFINED VARIABLE.** The unresolved-name pass
  brought `exposing (..)` members into scope only from the Flow-stdlib index
  (`Snapshot::module_members`), so a user module's exported names (e.g. a
  cross-module `async` helper `step`) used unqualified were reported as
  undefined even though inference and the module graph resolved them. The
  workspace now enumerates each `module`'s exported value members from its
  symbol index (`Workspace::workspace_module_members`, memoized alongside the
  module-name cache) and threads them into `Snapshot::build`; the name
  resolver consults them for both `exposing (..)` and qualified `Mod.member`
  membership checks. A genuine typo is still flagged (the fix does not
  blanket-suppress unknown names).
