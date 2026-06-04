#!/usr/bin/env bash
# Rebuild flux-lsp, repackage the VS Code extension, and reinstall it.
#
# Usage:
#   ./scripts/rebuild-vsix.sh              # build + package + install
#   ./scripts/rebuild-vsix.sh --no-install # build + package only

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VSCODE_DIR="$REPO_ROOT/editors/vscode"

# ── platform detection ────────────────────────────────────────────────────────
# Windows builds produce flux-lsp.exe; macOS/Linux produce flux-lsp.
# Matches the per-platform staging shown in editors/vscode/README.md.
EXE_SUFFIX=""
case "$(uname -s 2>/dev/null || echo unknown)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) EXE_SUFFIX=".exe" ;;
esac

# ── options ───────────────────────────────────────────────────────────────────
NO_INSTALL=false
for arg in "$@"; do
  case "$arg" in
    --no-install) NO_INSTALL=true ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

# ── step 1: build the server binary ───────────────────────────────────────────
echo "==> Building flux-lsp (release)..."
cargo build --release -p flux-lsp --manifest-path "$REPO_ROOT/Cargo.toml"

# ── step 2: stage the binary ──────────────────────────────────────────────────
echo "==> Staging binary..."
mkdir -p "$VSCODE_DIR/server"
cp "$REPO_ROOT/target/release/flux-lsp$EXE_SUFFIX" "$VSCODE_DIR/server/flux-lsp$EXE_SUFFIX"

# ── step 3: install JS deps (no-op if already up to date) ────────────────────
echo "==> Installing JS dependencies..."
npm install --prefix "$VSCODE_DIR" --silent

# ── step 4: compile TypeScript ────────────────────────────────────────────────
echo "==> Compiling TypeScript..."
npm run compile --prefix "$VSCODE_DIR"

# ── step 5: package ───────────────────────────────────────────────────────────
echo "==> Packaging extension..."
VSIX_PATH=$(cd "$VSCODE_DIR" && npx --yes @vscode/vsce package --no-git-tag-version 2>/dev/null \
  | grep "Packaged:" | sed 's/.*Packaged: //' | sed 's/ (.*//')

# Fallback: find the most recently created .vsix if the grep didn't match.
if [[ -z "$VSIX_PATH" ]]; then
  VSIX_PATH=$(ls -t "$VSCODE_DIR"/*.vsix 2>/dev/null | head -1)
fi

if [[ -z "$VSIX_PATH" ]]; then
  echo "ERROR: Could not locate the packaged .vsix file." >&2
  exit 1
fi

echo "==> Packaged: $VSIX_PATH"

# ── step 6: install into VS Code ──────────────────────────────────────────────
if [[ "$NO_INSTALL" == "true" ]]; then
  echo "==> Skipping install (--no-install passed)."
  echo ""
  echo "To install manually:"
  echo "  code --uninstall-extension flux.flux-language"
  echo "  code --install-extension \"$VSIX_PATH\""
  echo "Then fully restart VS Code (Cmd+Q, not Reload Window)."
  exit 0
fi

if ! command -v code &>/dev/null; then
  echo "WARNING: 'code' CLI not found on PATH — skipping install."
  echo "Install manually: code --install-extension \"$VSIX_PATH\""
  exit 0
fi

echo "==> Uninstalling previous version (if any)..."
code --uninstall-extension flux.flux-language 2>/dev/null || true

echo "==> Installing $VSIX_PATH..."
code --install-extension "$VSIX_PATH"

echo ""
echo "Done. Fully restart VS Code (Cmd+Q) to activate the new version."
