# Flux Language Support for VS Code

Syntax highlighting and language-server integration for [Flux](../..). The extension bundles the `flux-lsp` binary, so end users do **not** need to install anything else.

## Features

- Diagnostics on open / change / save
- Hover types (literals, expressions, declarations, effect names, type names)
- Go to definition (intra-file)
- Top-level identifier + keyword completion
- Document symbols (outline view)
- Formatting

## Install (end users)

1. Download the latest `flux-language-<version>.vsix` from the [GitHub Releases](../../../../releases) page (or the file produced by `vsce package` in this directory).
2. Install it:

   ```powershell
   code --install-extension flux-language-<version>.vsix
   ```
3. Fully close and reopen VS Code (not just reload window).
4. Open any `.flx` file — the bundled language server starts automatically. Confirm with **View → Output → "Flux Language Server"**.

To upgrade later, repeat steps 1–3 with a newer `.vsix`. VS Code replaces the previous version in place.

To remove:

```powershell
code --uninstall-extension flux.flux-language
```

## Settings

- `flux.serverPath` — absolute path override for the `flux-lsp` binary. Leave empty (default) to use the binary bundled in the `.vsix`. Only set this if you want to point at a custom local build, e.g. `cargo install --path crates/flux-lsp` output.
- `flux.trace.server` — `off` / `messages` / `verbose`. Logs LSP traffic to the **"Flux Language Server"** Output channel.

---

## Develop and rebuild the extension

You only need this if you're modifying the language server, the TypeScript client, or the TextMate grammar.

### One-time setup

Install Node.js 18+ and npm (Windows: `winget install OpenJS.NodeJS.LTS`). Then:

```powershell
cd editors\vscode
npm install
```

This populates `node_modules/` (gitignored).

### Iteration loop while developing

Pick whichever workflow you prefer. **Workflow A** is faster for tight edits; **B** verifies the packaging path.

**A. Run the extension from source (no packaging).**

```powershell
# 1. Build the server in release mode and copy it next to the extension.
cargo build --release -p flux-lsp
Copy-Item ..\..\target\release\flux-lsp.exe .\server\flux-lsp.exe -Force

# 2. Compile the TypeScript client.
npm run compile

# 3. Launch a VS Code window with this directory loaded as a dev extension.
code --extensionDevelopmentPath=$PWD ..\..
```

The second VS Code window runs your in-tree extension code without touching your installed extensions. Use `npm run watch` in a separate terminal to recompile on save.

**B. Repackage and reinstall (verifies the shipped artifact).**

This is the loop the next section ("Build a new `.vsix`") describes.

### Build a new `.vsix`

1. **Bump the version** in [package.json](package.json) (`"version": "0.0.X"`). VS Code refuses to re-install the same version on top of itself; bumping forces an upgrade.

2. **Build the server binary** in release mode for your platform:

   ```powershell
   cargo build --release -p flux-lsp
   ```

3. **Copy the binary into `server/`** (the path the extension expects at runtime):

   ```powershell
   # Windows:
   Copy-Item ..\..\target\release\flux-lsp.exe .\server\flux-lsp.exe -Force
   # macOS / Linux:
   cp ../../target/release/flux-lsp ./server/flux-lsp
   ```

   The `server/` directory is gitignored; the binary is built fresh per release.

4. **Compile the TypeScript client:**

   ```powershell
   npm run compile
   ```

5. **Package the extension:**

   ```powershell
   npx --yes @vscode/vsce package
   ```

   Output: `flux-language-<version>.vsix` (~1.6 MB).

6. **Verify the contents.** Optional sanity check:

   ```powershell
   npx --yes @vscode/vsce ls --tree
   ```

   Confirm `server/flux-lsp{.exe}`, `out/extension.js`, and `node_modules/vscode-languageserver-protocol/` all appear in the listing. Missing the protocol package is the gotcha that broke 0.0.2 — see the troubleshooting section below.

### Install the rebuilt `.vsix`

For local testing on the same machine:

```powershell
code --uninstall-extension flux.flux-language
code --install-extension .\flux-language-<version>.vsix
```

Then **fully close all VS Code windows and reopen** — a reload-window is not enough when the extension's `package.json` changed.

### Verify the upgrade

1. **Check the version is the new one:** `Ctrl+Shift+X` → search "Flux" → confirm the version in the extension's row matches what you packaged.
2. **Open an `.flx` file.** Bottom-right status bar should read "Flux".
3. **`View → Output`** → dropdown lists "Flux Language Server" with no errors at the top.
4. **Hover on a literal** (e.g. `42`) → inferred type appears.

If "Flux Language Server" never shows up in the Output dropdown, the extension failed to activate — see the next section.

### Troubleshooting

**`Ctrl+Shift+P → "Developer: Show Logs..." → "Extension Host"`** is the source of truth. Search the log for `flux.flux-language`. Common failures:

- `Cannot find module 'vscode-languageserver-protocol'` (or any other npm package) — the package was excluded from the `.vsix` by [.vscodeignore](.vscodeignore). Make sure the ignore file only filters out test/markdown/typescript-source files inside `node_modules/`, not the modules themselves.
- `ENOENT` for `flux-lsp{.exe}` — the binary isn't in `server/`, or the path resolution in [src/extension.ts](src/extension.ts) is wrong. Confirm `editors/vscode/server/flux-lsp.exe` exists before packaging.
- Server starts but immediately exits — run it directly: `& "$env:USERPROFILE\.vscode\extensions\flux.flux-language-<version>\server\flux-lsp.exe"`. It should hang waiting for stdin. If it errors out, the binary itself has a problem (rebuild it).
- Extension is "Activating" forever — `flux.trace.server: verbose` in your settings, reload, look at the **Output → "Flux"** channel to see what the server is sending.

If you have multiple `flux.*` extensions installed (e.g. from older experiments), uninstall the stragglers:

```powershell
code --list-extensions | Select-String flux
code --uninstall-extension <publisher>.<name>
```

## Layout

```
editors/vscode/
  package.json                       Extension manifest
  package-lock.json                  Pinned npm dep versions (commit this)
  tsconfig.json                      TypeScript config
  language-configuration.json        Brackets / comments
  syntaxes/flux.tmLanguage.json      TextMate grammar
  src/extension.ts                   Client wiring
  out/                               Compiled JS (gitignored)
  node_modules/                      npm deps (gitignored)
  server/                            Bundled flux-lsp binary (gitignored)
  *.vsix                             Built packages (gitignored)
```

`node_modules` and `out` are reproduced by `npm install` + `npm run compile`. The server binary is built by `cargo build --release -p flux-lsp`. Don't commit any of them.

## Roadmap

- Cross-platform `.vsix` build matrix in CI (Windows / macOS-x64 / macOS-arm64 / Linux-x64 / Linux-arm64)
- Cross-file go-to-definition
- Semantic tokens / inlay hints
- Code actions, rename, references
