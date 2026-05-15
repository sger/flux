# Flux Language Support for VS Code

Syntax highlighting and language-server integration for [Flux](../..).

The published `.vsix` bundles a pre-built `flux-lsp.exe` for **Windows x64**. macOS and Linux users build `flux-lsp` from source once and the extension finds it via `PATH` or the `flux.serverPath` setting. A cross-platform CI build matrix is on the roadmap; until then non-Windows users follow the local-build path.

## Features

- Diagnostics on open / change / save
- Hover types, keyword docs, module/field labels
- Go to definition (intra-file)
- Context-aware completion (module members, record fields, `with` clauses, named-constructor fields)
- Document symbols (outline view)
- Formatting

## Quick start

Pick the path that matches your situation. Each box links to the detailed section below.

> **I'm on Windows and I have a `flux-language-<version>.vsix` file** (downloaded from Releases or shared by someone).
> Jump to [Install on Windows](#install-on-windows). One command, then restart VS Code. ~1 minute.

> **I'm on macOS / Linux and I have a `flux-language-<version>.vsix`.**
> The bundled binary inside the `.vsix` is Windows-only — you still need to build `flux-lsp` locally. Follow [Install on macOS](#install-on-macos-intel-or-apple-silicon) or [Install on Linux](#install-on-linux-x64-or-arm64). ~10 minutes including the initial Rust build.

> **I don't have a `.vsix` (any platform).**
> Build one from a clean checkout — see [Build the `.vsix` yourself](#build-the-vsix-yourself-any-platform). Walkthrough is nine steps and works the same way on Windows, macOS, and Linux. ~15 minutes the first time (mostly waiting on `cargo build`).

> **I'm hacking on the LSP or the extension itself.**
> Use the dev iteration loop in [Develop and rebuild](#develop-and-rebuild-the-extension) — no packaging needed; you launch a second VS Code window that runs the in-tree extension code directly.

If you get stuck at any step, see [Troubleshooting](#troubleshooting) at the bottom.

## Platform support

| Platform | Bundled binary | Local build needed |
|---|---|---|
| Windows x64 | `flux-lsp.exe` (in `.vsix`) | No |
| macOS Intel (`x86_64-apple-darwin`) | none | Yes |
| macOS Apple Silicon (`aarch64-apple-darwin`) | none | Yes |
| Linux x64 (`x86_64-unknown-linux-gnu`) | none | Yes |
| Linux arm64 (`aarch64-unknown-linux-gnu`) | none | Yes |

---

## Build the `.vsix` yourself (any platform)

If you don't have a prebuilt `flux-language-<version>.vsix`, this section walks you through producing one from a clean checkout. It's identical for Windows, macOS, and Linux modulo shell flavor — Windows samples use PowerShell, Unix samples use bash/zsh.

### Prerequisites

| Tool | Why | Install |
|---|---|---|
| Git | Clone the repo | Windows: `winget install Git.Git` · macOS: `xcode-select --install` · Linux: distro package |
| Rust toolchain | Build `flux-lsp` (Edition 2024, MSRV pinned to 1.93.0 — see CI) | All platforms: [rustup.rs](https://rustup.rs) — paste the one-line installer, restart your shell |
| Node.js 18+ and npm | Compile the TS client and run `vsce package` | Windows: `winget install OpenJS.NodeJS.LTS` · macOS: `brew install node` · Linux: distro package or [nodejs.org](https://nodejs.org/) |
| VS Code | Required to install/test the `.vsix`. Make sure the `code` CLI is on `PATH` | [code.visualstudio.com/download](https://code.visualstudio.com/download). On macOS run `Cmd+Shift+P` → "Shell Command: Install 'code' command in PATH" once after installing |

Verify each is on `PATH`:

```bash
git --version
cargo --version
node --version
npm --version
code --version
```

### 1. Clone the repo

```bash
git clone https://github.com/sger/flux.git
cd flux
```

(Or use your existing clone — `cd` into the repo root.)

### 2. Build the server binary

```bash
cargo build --release -p flux-lsp
```

This produces:

- Windows: `target\release\flux-lsp.exe`
- macOS / Linux: `target/release/flux-lsp`

First build is slow (compiles the whole `flux` crate plus deps); subsequent builds are seconds-scale.

### 3. Stage the binary next to the extension

The extension expects the binary at `editors/vscode/server/flux-lsp{.exe}` at packaging time.

Windows (PowerShell):

```powershell
cd editors\vscode
Copy-Item ..\..\target\release\flux-lsp.exe .\server\flux-lsp.exe -Force
```

macOS / Linux:

```bash
cd editors/vscode
mkdir -p server
cp ../../target/release/flux-lsp ./server/flux-lsp
```

(The `server/` directory is gitignored; the binary is built fresh per release.)

### 4. Install the JS deps and compile the TypeScript client

```bash
npm install            # first time only, or after package-lock.json changes
npm run compile        # transpiles src/extension.ts -> out/extension.js
```

### 5. (Optional) Bump the version

If you're upgrading an already-installed `.vsix`, edit [package.json](package.json) and increment `"version"` (e.g. `0.0.5` → `0.0.6`). VS Code silently refuses to reinstall the same version on top of itself.

### 6. Package the `.vsix`

```bash
npx --yes @vscode/vsce package
```

Output: `flux-language-<version>.vsix` in the current directory (`editors/vscode/`). Size is roughly 1.7 MB on Windows (includes the `.exe`) and ~150 KB on macOS/Linux (no bundled binary).

Sanity-check the archive contents:

```bash
npx --yes @vscode/vsce ls --tree
```

Confirm you see `out/extension.js`, `syntaxes/flux.tmLanguage.json`, `node_modules/vscode-languageserver-protocol/`, and (on Windows) `server/flux-lsp.exe`.

### 7. Install into VS Code

```bash
# Remove the previous version first (no-op if nothing installed):
code --uninstall-extension flux.flux-language

# Install the new one:
code --install-extension ./flux-language-<version>.vsix
```

Windows users: replace `./` with `.\` if you prefer, or pass an absolute path.

### 8. Restart VS Code

**Fully close all VS Code windows** (Cmd+Q on macOS, File → Exit on Windows/Linux). A Reload Window is **not** enough when the extension's `package.json` changed.

### 9. Verify

1. Open any `.flx` file (e.g. `examples/guide/01_getting_started.flx`).
2. Bottom-right status bar reads **Flux**.
3. **View → Output** → dropdown lists **Flux Language Server**.
4. Hover on a literal (e.g. `42`) → tooltip shows the inferred type.

If hover stays empty, jump to **Troubleshooting** at the bottom.

### macOS / Linux only — find the server binary

On Windows, step 3 staged the binary inside the `.vsix`, so it ships with the extension. On macOS/Linux, the `.vsix` doesn't contain a binary that matches your platform; you must either put `flux-lsp` on `PATH` or point at it via the `flux.serverPath` setting:

```bash
# Option A: install globally (recommended).
cargo install --path crates/flux-lsp --force
which flux-lsp           # confirm ~/.cargo/bin/flux-lsp

# Option B: symlink.
sudo ln -sf "$(pwd)/target/release/flux-lsp" /usr/local/bin/flux-lsp

# Option C: set an absolute path in VS Code settings (Cmd/Ctrl+,):
#   "flux.serverPath": "/absolute/path/to/target/release/flux-lsp"
```

Without one of A/B/C the extension activates but its `flux-lsp` spawn fails with `ENOENT` and you'll see no diagnostics, hover, etc. — `Output → "Flux Language Server"` will show the error.

---

## Install on Windows

### Prerequisites

- **Visual Studio Code** ([download](https://code.visualstudio.com/download)) with the `code` CLI on `PATH`. The installer offers a checkbox during setup; if you missed it, open VS Code → `Ctrl+Shift+P` → "Shell Command: Install 'code' command in PATH".
- A copy of `flux-language-<version>.vsix`. Either download it from [GitHub Releases](../../../../releases) or build one yourself (see "Develop and rebuild" below).

### Install

Open PowerShell or Windows Terminal:

```powershell
# If upgrading from a previous version:
code --uninstall-extension flux.flux-language

# Install the new .vsix (use forward or back slashes — both work):
code --install-extension "E:\Github\flux\editors\vscode\flux-language-0.0.5.vsix"
```

### Activate

1. **Fully close all VS Code windows** (not just reload window — extension activation is cached until full restart).
2. Re-open VS Code.
3. Open any `.flx` file. The Flux language server starts automatically.

### Verify

1. Bottom-right of the status bar reads **Flux**.
2. **View → Output** → dropdown lists **Flux Language Server**.
3. Hover on a literal (e.g. `42`) — an inferred-type tooltip appears.

### Uninstall

```powershell
code --uninstall-extension flux.flux-language
```

---

## Install on macOS (Intel or Apple Silicon)

### Prerequisites

- **Visual Studio Code** ([download](https://code.visualstudio.com/download)). After install, open VS Code → `Cmd+Shift+P` → "Shell Command: Install 'code' command in PATH" so `code` is callable from Terminal.
- **Rust toolchain** — install via [rustup](https://rustup.rs):

  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  source "$HOME/.cargo/env"
  ```
- **Git** (Apple's Command Line Tools provide one: `xcode-select --install`).
- A clone of this repo somewhere local.

### Build `flux-lsp`

```bash
cd /path/to/flux
cargo build --release -p flux-lsp
```

Output: `target/release/flux-lsp` (about 4 MB).

### Put it on `PATH` (pick one)

Option A — `cargo install` (recommended, drops into `~/.cargo/bin` which is already on `PATH`):

```bash
cargo install --path crates/flux-lsp --force
which flux-lsp   # should print ~/.cargo/bin/flux-lsp
```

Option B — symlink into `/usr/local/bin`:

```bash
sudo ln -sf "$(pwd)/target/release/flux-lsp" /usr/local/bin/flux-lsp
```

Option C — leave the binary in place and configure the extension to point at it. Skip to "Configure VS Code (Option C only)" after installing the extension.

### Install the extension

```bash
# If upgrading from a previous version:
code --uninstall-extension flux.flux-language

# Install the new .vsix:
code --install-extension /path/to/flux/editors/vscode/flux-language-0.0.5.vsix
```

### Configure VS Code (Option C only)

If you skipped `cargo install` / `ln`, open `Cmd+,` → search "flux.serverPath" → set to the absolute path of the binary:

```jsonc
// settings.json
"flux.serverPath": "/Users/you/path/to/flux/target/release/flux-lsp"
```

### Activate

1. **Quit VS Code completely** (Cmd+Q — a Reload Window is not enough).
2. Re-open VS Code.
3. Open any `.flx` file.

### Verify

1. Status bar reads **Flux**.
2. **View → Output** → "Flux Language Server" appears with no errors at the top.
3. Hover on a literal — type tooltip appears.

### Apple Silicon note

`cargo build --release -p flux-lsp` on an M1/M2/M3 Mac produces a native arm64 binary. There's no Rosetta translation involved, and you don't need to specify `--target`. If you're on Intel macOS, you get x86_64 by default — same command.

### Uninstall

```bash
code --uninstall-extension flux.flux-language
# Optional cleanup if you used `cargo install`:
cargo uninstall flux-lsp
```

---

## Install on Linux (x64 or arm64)

### Prerequisites

- **Visual Studio Code** — install via your distro's package manager or from [code.visualstudio.com/download](https://code.visualstudio.com/download). The `code` CLI ships with it on most distros; verify with `which code`.
- **Rust toolchain** via [rustup](https://rustup.rs):

  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  source "$HOME/.cargo/env"
  ```
- **Build essentials**:
  - Debian/Ubuntu: `sudo apt install build-essential pkg-config libssl-dev git`
  - Fedora/RHEL: `sudo dnf install gcc gcc-c++ make pkgconfig openssl-devel git`
  - Arch: `sudo pacman -S base-devel git`
- A clone of this repo.

### Build `flux-lsp`

```bash
cd /path/to/flux
cargo build --release -p flux-lsp
```

Output: `target/release/flux-lsp`.

### Put it on `PATH` (pick one)

Option A — `cargo install` (recommended):

```bash
cargo install --path crates/flux-lsp --force
which flux-lsp   # should print ~/.cargo/bin/flux-lsp
```

If `~/.cargo/bin` isn't on your `PATH`, add it to `~/.bashrc` / `~/.zshrc`:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

Option B — symlink into `/usr/local/bin`:

```bash
sudo ln -sf "$(pwd)/target/release/flux-lsp" /usr/local/bin/flux-lsp
```

Option C — leave the binary in place and point the extension at it via the setting `flux.serverPath` (see macOS section).

### Install the extension

```bash
# If upgrading:
code --uninstall-extension flux.flux-language

# Install the new .vsix:
code --install-extension /path/to/flux/editors/vscode/flux-language-0.0.5.vsix
```

### Activate

1. Close all VS Code windows.
2. Re-open VS Code.
3. Open any `.flx` file.

### Verify

Same checklist as macOS:

1. Status bar shows **Flux**.
2. **View → Output** → **Flux Language Server** present with no errors.
3. Hover on a literal renders the inferred type.

### Wayland / X11 note

The extension is GUI-toolkit-agnostic — runs identically on GNOME, KDE, Wayland sessions, X11. The `code` CLI behavior is the same.

### Uninstall

```bash
code --uninstall-extension flux.flux-language
cargo uninstall flux-lsp   # optional
```

---

## Settings (all platforms)

| Setting | Type | Default | Purpose |
|---|---|---|---|
| `flux.serverPath` | string | `""` | Absolute path override for the `flux-lsp` binary. Empty = use the binary bundled in the `.vsix`, else search `PATH`. |
| `flux.trace.server` | enum | `"off"` | `off` / `messages` / `verbose`. Logs LSP traffic to the **Flux Language Server** Output channel. Enable `verbose` to debug a misbehaving hover/completion. |

---

## Smoke test (post-install, any platform)

Open a sample Flux file and confirm each capability:

| Capability | Action | Expected |
|---|---|---|
| Diagnostics | Save a file with a parse error | Red squiggle at the error site |
| Hover (type) | Hover on an integer literal `42` | Tooltip showing `Int` |
| Hover (keyword) | Hover on `let` or `fn` | Markdown keyword doc with code example |
| Hover (module) | Hover on `String` in `String.join(...)` | `module: String` |
| Hover (field) | Hover on `.name` in `alice.name` | `name: String` (or actual field type) |
| Go to def | F12 on a `let` binding's use site | Cursor jumps to declaration |
| Go to def (data) | F12 on `User` in `User { ... }` | Jumps to `data User { ... }` decl |
| Completion (module) | Type `String.` | List of `Flow.String` exports |
| Completion (with) | Type `with ` inside `fn ... ` | List of effect labels (`IO`, `Async`, ...) |
| Completion (fields) | Type `alice.` or `Person { ` | Field names of the record |
| Document symbols | `Ctrl/Cmd+Shift+O` | Outline of top-level decls |
| Formatting | `Shift+Alt+F` (Win/Linux) or `Shift+Opt+F` (macOS) | Buffer reformats |

---

## Upgrade (all platforms)

1. Bump-versioned `.vsix` available (either new release or you packaged one — see "Develop and rebuild" below).
2. `code --uninstall-extension flux.flux-language`
3. `code --install-extension <new>.vsix`
4. **Fully restart VS Code**, not Reload Window.

VS Code refuses to install a `.vsix` whose version matches what's already installed; bump the version in [package.json](package.json) before repackaging, or the install becomes a silent no-op.

---

## Develop and rebuild the extension

You only need this section if you're modifying the language server, the TypeScript client, or the TextMate grammar.

### One-time setup

Install **Node.js 18+** and **npm**:

- Windows: `winget install OpenJS.NodeJS.LTS`
- macOS: `brew install node` (or [nodejs.org](https://nodejs.org/))
- Linux: distro package or [nodejs.org](https://nodejs.org/)

Then install the JS dependencies:

```bash
cd editors/vscode
npm install
```

Populates `node_modules/` (gitignored).

### Iteration loop while developing

**Workflow A — Run the extension from source (fast).**

Windows (PowerShell):

```powershell
cargo build --release -p flux-lsp
Copy-Item ..\..\target\release\flux-lsp.exe .\server\flux-lsp.exe -Force
npm run compile
code --extensionDevelopmentPath=$PWD ..\..
```

macOS / Linux (bash / zsh):

```bash
cargo build --release -p flux-lsp
cp ../../target/release/flux-lsp ./server/flux-lsp
npm run compile
code --extensionDevelopmentPath="$PWD" ../..
```

The second VS Code window runs the in-tree extension without touching your installed one. Run `npm run watch` in another terminal for auto-recompile on save.

**Workflow B — Repackage and reinstall (verifies the shipped artifact).** Follow the next section.

### Build a new `.vsix`

1. **Bump the version** in [package.json](package.json) (`"version": "0.0.X"`). VS Code refuses to re-install the same version on top of itself; bumping forces an upgrade.

2. **Build the server binary** in release mode for your platform:

   ```bash
   cargo build --release -p flux-lsp
   ```

3. **Copy the binary into `server/`**:

   ```powershell
   # Windows
   Copy-Item ..\..\target\release\flux-lsp.exe .\server\flux-lsp.exe -Force
   ```
   ```bash
   # macOS / Linux
   cp ../../target/release/flux-lsp ./server/flux-lsp
   ```

4. **Compile the TypeScript client:**

   ```bash
   npm run compile
   ```

5. **Package the extension:**

   ```bash
   npx --yes @vscode/vsce package
   ```

   Output: `flux-language-<version>.vsix` (~1.7 MB).

6. **Verify the contents** (optional sanity check):

   ```bash
   npx --yes @vscode/vsce ls --tree
   ```

   Confirm `server/flux-lsp{.exe}`, `out/extension.js`, and `node_modules/vscode-languageserver-protocol/` all appear.

### Install the rebuilt `.vsix`

Windows:

```powershell
code --uninstall-extension flux.flux-language
code --install-extension .\flux-language-<version>.vsix
```

macOS / Linux:

```bash
code --uninstall-extension flux.flux-language
code --install-extension ./flux-language-<version>.vsix
```

Then **fully close and reopen VS Code** — Reload Window is not enough when `package.json` changed.

---

## Troubleshooting

The Extension Host log is the source of truth:

- **`Ctrl+Shift+P`** (Windows/Linux) or **`Cmd+Shift+P`** (macOS) → **"Developer: Show Logs..." → "Extension Host"**.
- Search the log for `flux.flux-language`.

### "Flux Language Server" doesn't appear in the Output dropdown

The extension failed to activate. Look at the Extension Host log for the actual error message.

### `Cannot find module 'vscode-languageserver-protocol'`

The package was excluded from the `.vsix` by [.vscodeignore](.vscodeignore). Make sure the ignore file only filters out test/markdown/typescript-source files inside `node_modules/`, not the modules themselves.

### `ENOENT` for `flux-lsp` / `flux-lsp.exe`

- **On Windows**: the binary isn't in `server/` of the installed extension. Confirm `editors/vscode/server/flux-lsp.exe` existed before packaging, repackage, reinstall.
- **On macOS / Linux**: the bundled binary doesn't exist (by design — only Windows ships one). The extension fell back to `flux-lsp` on `PATH`. Either:
  - Add `~/.cargo/bin` to `PATH` and re-run `cargo install --path crates/flux-lsp`, OR
  - Set `flux.serverPath` to the absolute path of your built binary.

### Server starts but immediately exits

Run it directly to surface the underlying error:

```powershell
# Windows
& "$env:USERPROFILE\.vscode\extensions\flux.flux-language-<version>\server\flux-lsp.exe"
```
```bash
# macOS
/Users/you/.cargo/bin/flux-lsp        # or wherever you installed it
# Linux
~/.cargo/bin/flux-lsp
```

It should hang waiting for stdin. If it errors out (panic, missing shared lib, etc.), the binary itself is broken — rebuild it.

### Extension is "Activating" forever

Set `flux.trace.server` to `verbose` in settings, restart VS Code, and watch **Output → "Flux Language Server"** for what the server is sending (or failing to send).

### Multiple `flux.*` extensions installed

```bash
code --list-extensions | grep -i flux       # macOS / Linux
code --list-extensions | Select-String flux # Windows PowerShell
code --uninstall-extension <publisher>.<name>
```

### macOS: "cannot be opened because the developer cannot be verified"

If macOS Gatekeeper flags your locally-built `flux-lsp` binary:

```bash
xattr -d com.apple.quarantine ~/.cargo/bin/flux-lsp
```

This only happens if the binary somehow inherited a quarantine attribute (rare — usually only when downloaded from the internet, not built locally).

### Linux: `error while loading shared libraries: ...`

You're missing a runtime dep. The most common one on minimal containers:

```bash
sudo apt install libssl3      # Debian/Ubuntu
sudo dnf install openssl      # Fedora/RHEL
```

If `cargo build` succeeded, the binary should already declare its runtime requirements — `ldd target/release/flux-lsp` lists them.

---

## Layout

```
editors/vscode/
  package.json                       Extension manifest
  package-lock.json                  Pinned npm dep versions (commit this)
  tsconfig.json                      TypeScript config
  language-configuration.json        Brackets / comments / indentation
  syntaxes/flux.tmLanguage.json      TextMate grammar
  src/extension.ts                   LSP client wiring
  out/                               Compiled JS (gitignored)
  node_modules/                      npm deps (gitignored)
  server/                            Bundled flux-lsp binary (gitignored)
  *.vsix                             Built packages (gitignored)
```

`node_modules` and `out` are reproduced by `npm install` + `npm run compile`. The server binary is built by `cargo build --release -p flux-lsp`. None of them are committed.

## Roadmap

- Cross-platform `.vsix` build matrix in CI (Windows / macOS x64 / macOS arm64 / Linux x64 / Linux arm64)
- Cross-file go-to-definition (VFS layer)
- Semantic tokens / inlay hints
- Code actions, rename, find references
- Scope-aware locals in completion
