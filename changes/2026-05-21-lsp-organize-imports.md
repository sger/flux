### Added
- `flux-lsp`: `source.organizeImports` code action. The leading block of
  top-level `import`s is sorted by module name, exact-duplicate lines are
  dropped, and unused imports are removed. Removal is conservative: only an
  import with no `exposing` clause (so its only binding is the module/alias
  name) and flagged unused by the linter's W003 check is dropped — `exposing`
  imports are always kept, since their unqualified bindings aren't tracked by
  that check. The action is offered only when the client requests it
  (`source.organizeImports` / `source`), so it doesn't clutter the cursor
  lightbulb, and the server now advertises `codeActionKinds` so editors expose
  an "Organize Imports" command and can run it on save.
