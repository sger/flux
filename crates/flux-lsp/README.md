# flux-lsp

Language Server Protocol implementation for the [Flux](../..) language.

A standalone binary that speaks LSP over stdio. Any LSP-capable editor can talk to it. The VS Code client lives at [editors/vscode/](../../editors/vscode/), but the server is editor-agnostic.

## What it does

| Capability | Status |
|---|---|
| `publishDiagnostics` (parser + single-file inference) | Yes |
| `textDocument/hover` (literals, expressions, decl sites, effect names, type names) | Yes |
| `textDocument/definition` (intra-file) | Yes |
| `textDocument/completion` (top-level symbols + keywords) | Yes |
| `textDocument/documentSymbol` | Yes |
| `textDocument/formatting` | Yes |
| Cross-file go-to-definition, semantic tokens, inlay hints, rename, code actions | Not yet |
| `Flow.Primops` prelude types (`print`, `println`, …) on hover | Yes, when `lib/Flow/Primops.flx` is reachable from the buffer |
| Full prelude (`Flow.List`, `Flow.Option`, …) | Not yet — needs multi-module loading |

## Install

```sh
cargo install --path crates/flux-lsp
```

Drops `flux-lsp` (`flux-lsp.exe` on Windows) into your Cargo bin directory (usually `~/.cargo/bin` or `%USERPROFILE%\.cargo\bin`). That directory is on `PATH` by default if Rust was installed via rustup.

Verify:

```sh
flux-lsp --help
```

The process will block waiting for LSP traffic on stdin — that's correct. Ctrl-C to exit.

To rebuild after pulling new changes:

```sh
cargo install --path crates/flux-lsp --force
```

## Run from your editor

### VS Code

Use the extension at [editors/vscode/](../../editors/vscode/). Install Node 18+, then:

```sh
cd editors/vscode
npm install
npm run compile
code --extensionDevelopmentPath=$PWD <path-to-your-flux-workspace>
```

Settings:
- `flux.serverPath` — absolute path to the `flux-lsp` binary if it's not on `PATH`.
- `flux.trace.server` — set to `messages` or `verbose` to log LSP traffic to **View → Output → "Flux Language Server"**.

### Neovim (`nvim-lspconfig`)

```lua
vim.lsp.start({
  name = "flux-lsp",
  cmd = { "flux-lsp" },
  root_dir = vim.fn.getcwd(),
  filetypes = { "flux" },
})
```

### Helix (`languages.toml`)

```toml
[[language]]
name = "flux"
file-types = ["flx"]
language-servers = ["flux-lsp"]

[language-server.flux-lsp]
command = "flux-lsp"
```

### Zed

Add a custom language server in your Zed config pointing at `flux-lsp`.

## Run from the command line (for debugging)

The server speaks LSP framing over stdin/stdout. You normally won't talk to it by hand, but to verify it starts:

```sh
flux-lsp
```

You should see no output. Send `Content-Length: 0\r\n\r\n` and a JSON-RPC message to interact. Most of the time you just want the editor to drive it.

Set `FLUX_LSP_LOG` to control logging (env-filter syntax):

```sh
FLUX_LSP_LOG=debug flux-lsp
FLUX_LSP_LOG=flux_lsp=trace,lsp_server=warn flux-lsp
```

Logs go to stderr so they don't interfere with the LSP protocol on stdout.

## Prelude discovery

On the first `didOpen`, the server walks up from the buffer's parent directory (up to 8 levels) looking for `lib/Flow/Primops.flx`. If found, the file is parsed and inferred, and its user-facing schemes (`print`, `println`, `read_file`, `write_file`, `read_stdin`, `clock_now`, `now_ms`, `idiv`, `imod`, `index`, `array_get`, `panic`) are seeded into every subsequent inference.

If `lib/Flow/Primops.flx` isn't found, the server logs a warning and continues with an empty prelude. Hover on prelude names will fall back to fresh type variables.

This means: open the Flux repo (or any Flux project that has a `lib/Flow/` next to it) and prelude hover works automatically. Open a one-off `.flx` file in `/tmp` and it doesn't.

## Run the integration tests

```sh
cargo test -p flux-lsp
```

The tests use `lsp_server::Connection::memory()` to drive the server in-process — no subprocesses, no stdio. URIs are synthetic (`file:///*.flx`) so the prelude walk-up returns nothing, keeping tests hermetic.

## Architecture

- [src/main.rs](src/main.rs) — stdio transport setup, initialize handshake, tracing.
- [src/server.rs](src/server.rs) — event loop and request dispatcher.
- [src/document.rs](src/document.rs) — `DocumentStore` keyed by `Uri`; holds the lazy `Prelude`.
- [src/snapshot.rs](src/snapshot.rs) — per-buffer parsed AST + inference result, rebuilt synchronously on every change.
- [src/hover_index.rs](src/hover_index.rs) — position → innermost `HoverTarget` (expression / decl / effect / type).
- [src/symbol_index.rs](src/symbol_index.rs) — top-level identifier → definition span, for go-to-definition and completion.
- [src/prelude.rs](src/prelude.rs) — lazy `Flow.Primops` loader.
- [src/convert.rs](src/convert.rs) — Flux `Span`/`Position`/`Severity` ↔ `lsp_types` equivalents.
- [src/handlers/](src/handlers/) — one file per LSP request type.

The server is single-threaded for now. Full reparse + reinference on every `didChange`; no incremental engine yet. Fine for typical Flux files; revisit if latency becomes a problem.

## Contributing

See the [implementation plan](../../../.claude/plans/this-is-a-new-buzzing-stream.md) for the M1/M2 roadmap and the deferred follow-ups.
