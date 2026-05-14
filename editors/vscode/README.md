# Flux Language Support for VS Code

Syntax highlighting and language-server integration for [Flux](../..).

## Features

- Diagnostics on open/change/save
- Hover types
- Go to definition (intra-file)
- Top-level identifier completion + keyword completion
- Document symbols (outline view)
- Formatting

## Setup

1. Build and install the language server:

   ```sh
   cargo install --path crates/flux-lsp
   ```

   This installs the `flux-lsp` binary into your Cargo bin directory. Make sure
   that directory is on your `PATH`, or set `flux.serverPath` to the absolute
   path of the binary.

2. Build the extension:

   ```sh
   cd editors/vscode
   npm install
   npm run compile
   ```

3. Launch VS Code with the extension loaded (from `editors/vscode/`):

   ```sh
   code --extensionDevelopmentPath=$PWD ..
   ```

## Settings

- `flux.serverPath` — path to the `flux-lsp` binary. Defaults to `flux-lsp` on `PATH`.
- `flux.trace.server` — set to `messages` or `verbose` to log LSP traffic to the output panel.

## Roadmap

Future improvements (see [the implementation plan](../../../../.claude/plans/) and project roadmaps):

- Cross-file go-to-definition
- Semantic tokens / inlay hints
- Code actions, rename, references
- Bundled binary distribution
