# Flux Language for Zed (Dev Extension)

This is a starter Zed extension that registers `.flx` files as `Flux`.

## Current status

- File association: `.flx` -> `Flux`
- Comment/bracket config for Flux files
- Starter syntax highlighting (best effort)

Note: This currently reuses Zed's built-in JavaScript grammar to provide basic highlighting.
For accurate parsing/highlighting, add a dedicated `tree-sitter-flux` grammar later.

## Install in Zed

Recommended (works on all platforms):

1. Open Zed.
2. Command Palette -> `Extensions: Install Dev Extension`.
3. Select this folder: `tools/zed-flux`.
4. Open a `.flx` file and set language mode to `Flux` if needed.

## Platform notes

- macOS/Linux local: install once in your local Zed app.
- Windows + WSL: if editing files through WSL, install in the Zed instance that owns that workspace.

## Files

- `extension.toml`
- `languages/flux/config.toml`
- `languages/flux/highlights.scm`
