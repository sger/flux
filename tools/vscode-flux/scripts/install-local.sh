#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT_ID="flux-local.flux-language-0.0.1"

install_to_dir() {
  local base_dir="$1"
  local target_dir="${base_dir}/${EXT_ID}"

  mkdir -p "${base_dir}"
  rm -rf "${target_dir}"
  cp -R "${ROOT_DIR}" "${target_dir}"
  echo "Installed to: ${target_dir}"
}

install_to_dir "${HOME}/.vscode/extensions"

if [ -d "${HOME}/.vscode-insiders" ]; then
  install_to_dir "${HOME}/.vscode-insiders/extensions"
fi

# Remote SSH / container VS Code server installs extensions here.
if [ -d "${HOME}/.vscode-server" ]; then
  install_to_dir "${HOME}/.vscode-server/extensions"
fi

if [ -d "${HOME}/.vscode-server-insiders" ]; then
  install_to_dir "${HOME}/.vscode-server-insiders/extensions"
fi

echo "Done. Restart VS Code and open a .flx file."
