#!/usr/bin/env bash
# Install LLVM 18 development libraries for the Flux LLVM backend.
#
# Supports: macOS (Homebrew), Ubuntu/Debian/WSL (apt), Fedora/RHEL (dnf)
#
# Usage:
#   bash scripts/ci/install_llvm.sh
#
# After running, the LLVM_SYS_180_PREFIX environment variable is printed.
# Add it to your shell profile (~/.zshrc or ~/.bashrc) to persist it.

set -euo pipefail

LLVM_VERSION=18

red()   { printf '\033[1;31m%s\033[0m\n' "$*"; }
green() { printf '\033[1;32m%s\033[0m\n' "$*"; }
info()  { printf '\033[1;34m%s\033[0m\n' "$*"; }

detect_os() {
    case "$(uname -s)" in
        Darwin) echo "macos" ;;
        Linux)
            if [ -f /etc/os-release ]; then
                . /etc/os-release
                case "$ID" in
                    ubuntu|debian|pop|linuxmint) echo "debian" ;;
                    fedora|rhel|centos|rocky|alma) echo "fedora" ;;
                    *) echo "unknown-linux" ;;
                esac
            else
                echo "unknown-linux"
            fi
            ;;
        *) echo "unsupported" ;;
    esac
}

install_macos() {
    info "Installing LLVM $LLVM_VERSION via Homebrew..."

    if ! command -v brew &>/dev/null; then
        red "Homebrew is not installed. Install it from https://brew.sh"
        exit 1
    fi

    if brew list "llvm@$LLVM_VERSION" &>/dev/null; then
        info "llvm@$LLVM_VERSION is already installed."
    else
        brew install "llvm@$LLVM_VERSION"
    fi

    # zstd is required by LLVM's linker dependencies
    if ! brew list zstd &>/dev/null; then
        brew install zstd
    fi

    LLVM_PREFIX=$(brew --prefix "llvm@$LLVM_VERSION")
    ZSTD_LIB=$(brew --prefix zstd)/lib
    echo ""
    green "LLVM $LLVM_VERSION installed at $LLVM_PREFIX"
    echo ""
    info "Add this to your ~/.zshrc (or ~/.bash_profile):"
    echo ""
    echo "  export LLVM_SYS_180_PREFIX=$LLVM_PREFIX"
    echo "  export LIBRARY_PATH=\"$ZSTD_LIB:\${LIBRARY_PATH:-}\""
    echo ""
}

install_debian() {
    info "Installing LLVM $LLVM_VERSION via apt..."

    sudo apt-get update -qq
    sudo apt-get install -y \
        "llvm-$LLVM_VERSION-dev" \
        "libclang-$LLVM_VERSION-dev" \
        "libpolly-$LLVM_VERSION-dev" \
        "lld-$LLVM_VERSION" \
        libzstd-dev

    LLVM_PREFIX="/usr/lib/llvm-$LLVM_VERSION"
    echo ""
    green "LLVM $LLVM_VERSION installed at $LLVM_PREFIX"
    echo ""
    info "Add this to your ~/.bashrc:"
    echo ""
    echo "  export LLVM_SYS_180_PREFIX=$LLVM_PREFIX"
    echo ""
}

install_fedora() {
    info "Installing LLVM $LLVM_VERSION via dnf..."

    sudo dnf install -y \
        "llvm${LLVM_VERSION}-devel" \
        "clang${LLVM_VERSION}-devel" \
        "lld${LLVM_VERSION}"

    LLVM_PREFIX="/usr/lib64/llvm$LLVM_VERSION"
    echo ""
    green "LLVM $LLVM_VERSION installed at $LLVM_PREFIX"
    echo ""
    info "Add this to your ~/.bashrc:"
    echo ""
    echo "  export LLVM_SYS_180_PREFIX=$LLVM_PREFIX"
    echo ""
}

verify() {
    local llvm_config="$LLVM_PREFIX/bin/llvm-config"
    if [ -x "$llvm_config" ]; then
        local version
        version=$("$llvm_config" --version)
        green "Verified: llvm-config --version = $version"
    else
        red "Warning: $llvm_config not found or not executable."
        red "You may need to set LLVM_SYS_180_PREFIX manually."
    fi
}

# ── Main ──────────────────────────────────────────────────────────────────────

OS=$(detect_os)
info "Detected OS: $OS"

case "$OS" in
    macos)         install_macos ;;
    debian)        install_debian ;;
    fedora)        install_fedora ;;
    unknown-linux)
        red "Unknown Linux distribution."
        red "Please install LLVM $LLVM_VERSION manually and set:"
        red "  export LLVM_SYS_180_PREFIX=/path/to/llvm-$LLVM_VERSION"
        exit 1
        ;;
    *)
        red "Unsupported OS: $(uname -s)"
        exit 1
        ;;
esac

verify

echo ""
info "Next steps:"
echo "  1. Add the export line above to your shell profile"
echo "  2. Run: source ~/.zshrc  (or source ~/.bashrc)"
echo "  3. Build Flux with LLVM: cargo build --features llvm"
