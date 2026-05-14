### Fixed
- LSP: hover on names imported from non-prelude `Flow.*` modules (e.g. `Flow.Async.sleep`, `Flow.Tcp.*`) now resolves to the correct type instead of returning a free type variable rendered as `_`. The LSP walks buffer-level `import Flow.X` statements after parse and lazily preloads each module's schemes into the shared compiler, caching by module name so repeated imports across keystrokes are free.
- LSP: prelude member schemes (functions declared inside `module Flow.X { ... }` blocks) are now read from `InferProgramResult::module_member_schemes` rather than the top-level-only `resolved_binding_schemes`, so non-intrinsic prelude functions land in `cached_member_schemes` and are visible to buffer inference.

### Changed
- `flux::lsp_support::preload_one` is now `pub` (renamed `preload_module_into_compiler`) and a new `pub fn flow_module_file_for(flow_dir, module_name) -> Option<PathBuf>` helper resolves dotted module names like `Flow.Async` to `lib/Flow/Async.flx`. The LSP uses both to load buffer-driven imports on demand.
- `flux::lsp_support::PreludeCompiler` gains `flow_dir: Option<PathBuf>` and `loaded_module_names: Vec<String>` so consumers can resume the prelude search and dedupe re-loads of modules already in the auto-prelude.
