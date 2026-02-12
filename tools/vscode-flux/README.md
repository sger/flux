# Flux VS Code Extension (Local)

This folder contains a minimal VS Code extension that adds Flux language support for `.flx` files.

## Important

Creating this folder does **not** auto-install the extension into VS Code.
You must install it locally once.

## Install Locally

From the repo root:

```bash
bash tools/vscode-flux/scripts/install-local.sh
```

Then restart VS Code.

## Build VSIX (recommended for WSL + Windows)

From the repo root:

```bash
python3 tools/vscode-flux/scripts/build-vsix.py
```

Generated file:

`tools/vscode-flux/dist/flux-language-0.0.1.vsix`

Install in VS Code:

1. Open Extensions view.
2. Click `...` (top-right).
3. `Install from VSIX...`
4. Choose `tools/vscode-flux/dist/flux-language-0.0.1.vsix`.
5. Reload window.

## Verify

1. Open any `.flx` file.
2. Bottom-right language mode should show `Flux`.
3. If it does not, run `Developer: Reload Window` and reopen the file.

## Alternative (Extension Dev Host)

1. Open `tools/vscode-flux` as the workspace folder in VS Code.
2. Press `F5` to launch Extension Development Host.
3. Open a `.flx` file in the new host window.
