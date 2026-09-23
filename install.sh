#!/usr/bin/env bash

# ============================================================
# Rexo Code Installer
# ============================================================
#
# Release archive:
#   ./install.sh
#
# Source checkout:
#   ./install.sh
#
# Source checkout without rebuilding:
#   ./install.sh --skip-build
#
# Optional custom installation directory:
#   REXO_INSTALL_DIR="$HOME/bin" ./install.sh
#
# Supported:
#   Linux x86_64
#   macOS ARM64
#
# ============================================================

set -euo pipefail

SKIP_BUILD=0

# ============================================================
# Arguments
# ============================================================

for arg in "$@"; do
    case "$arg" in
        --skip-build)
            SKIP_BUILD=1
            ;;

        -h|--help)
            cat <<'EOF'
Rexo Code Installer

Usage:
  ./install.sh
      Install Rexo Code.

  ./install.sh --skip-build
      Install an existing source-build binary without rebuilding.

Environment:
  REXO_INSTALL_DIR
      Optional installation directory.

Examples:
  ./install.sh
  ./install.sh --skip-build
  REXO_INSTALL_DIR="$HOME/bin" ./install.sh

EOF
            exit 0
            ;;

        *)
            echo "Error: unknown argument '$arg'" >&2
            echo "Run './install.sh --help' for usage." >&2
            exit 1
            ;;
    esac
done

# ============================================================
# Locate installer directory
# ============================================================

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# ============================================================
# Find Rexo binary
# ============================================================
#
# Release archives contain:
#
#   rexo
#   install.sh
#   uninstall.sh
#
# Source repositories contain:
#
#   Cargo.toml
#   install.sh
#   target/release/rexo
#
# Release binaries always take priority.
# ============================================================

RELEASE_BIN="$SCRIPT_DIR/rexo"
SOURCE_BIN="$SCRIPT_DIR/target/release/rexo"

if [[ -f "$RELEASE_BIN" ]]; then

    # --------------------------------------------------------
    # Downloaded release archive
    # --------------------------------------------------------

    REXO_BIN="$RELEASE_BIN"

    echo "Rexo Code release binary detected."
    echo "Skipping source build."

else

    # --------------------------------------------------------
    # Source checkout
    # --------------------------------------------------------

    REXO_BIN="$SOURCE_BIN"

    if [[ ! -f "$SCRIPT_DIR/Cargo.toml" ]]; then
        echo "Error: Rexo binary was not found." >&2
        echo "" >&2
        echo "Expected either:" >&2
        echo "  $RELEASE_BIN" >&2
        echo "or a Rexo source checkout containing Cargo.toml." >&2
        exit 1
    fi

    if [[ "$SKIP_BUILD" -eq 0 ]]; then

        if ! command -v cargo >/dev/null 2>&1; then
            echo "Error: Cargo was not found on PATH." >&2
            echo "" >&2
            echo "Install Rust from https://rustup.rs and run this installer again." >&2
            exit 1
        fi

        echo "Building Rexo Code in release mode..."
        echo ""

        (
            cd "$SCRIPT_DIR"
            cargo build --release
        )

    elif [[ ! -f "$REXO_BIN" ]]; then

        echo "Error: --skip-build was specified, but the release binary does not exist:" >&2
        echo "  $REXO_BIN" >&2
        echo "" >&2
        echo "Run './install.sh' without --skip-build first." >&2
        exit 1

    fi

fi

# ============================================================
# Validate binary
# ============================================================

if [[ ! -f "$REXO_BIN" ]]; then
    echo "Error: Rexo binary was not found:" >&2
    echo "  $REXO_BIN" >&2
    exit 1
fi

if [[ ! -x "$REXO_BIN" ]]; then
    chmod +x "$REXO_BIN"
fi

# ============================================================
# Select installation directory
# ============================================================

if [[ -n "${REXO_INSTALL_DIR:-}" ]]; then

    INSTALL_DIR="$REXO_INSTALL_DIR"

elif [[ -d "$HOME/.local/bin" || "$OSTYPE" == "darwin"* || "$OSTYPE" == "linux-gnu"* ]]; then

    INSTALL_DIR="$HOME/.local/bin"

else

    INSTALL_DIR="$HOME/.rexo/bin"

fi

mkdir -p "$INSTALL_DIR"

TARGET="$INSTALL_DIR/rexo"

# ============================================================
# Install binary
# ============================================================

echo ""
echo "Installing Rexo Code..."
echo "  Source: $REXO_BIN"
echo "  Target: $TARGET"

cp -f "$REXO_BIN" "$TARGET"
chmod +x "$TARGET"

echo ""
echo "Installed successfully:"
echo "  $TARGET"

# ============================================================
# PATH helper
# ============================================================

path_contains() {
    case ":${PATH:-}:" in
        *":$1:"*)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

# ============================================================
# Configure PATH
# ============================================================

RC_FILE=""
PATH_UPDATED=0

if path_contains "$INSTALL_DIR"; then

    echo ""
    echo "Already on PATH:"
    echo "  $INSTALL_DIR"

else

    SHELL_NAME="$(basename "${SHELL:-bash}")"

    case "$SHELL_NAME" in

        zsh)
            RC_FILE="$HOME/.zshrc"
            PATH_LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;

        fish)
            RC_FILE="$HOME/.config/fish/config.fish"
            PATH_LINE="fish_add_path \"$INSTALL_DIR\""
            ;;

        bash)
            RC_FILE="$HOME/.bashrc"
            PATH_LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;

        *)
            RC_FILE="$HOME/.profile"
            PATH_LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;

    esac

    mkdir -p "$(dirname "$RC_FILE")"

    if [[ -f "$RC_FILE" ]] && grep -Fqx "$PATH_LINE" "$RC_FILE"; then

        echo ""
        echo "PATH configuration already exists:"
        echo "  $RC_FILE"

    else

        {
            echo ""
            echo "# Rexo Code"
            echo "$PATH_LINE"
        } >> "$RC_FILE"

        PATH_UPDATED=1

        echo ""
        echo "Added Rexo Code to PATH:"
        echo "  $RC_FILE"

    fi

fi

# ============================================================
# Verify installation
# ============================================================

echo ""

if path_contains "$INSTALL_DIR"; then

    if command -v rexo >/dev/null 2>&1; then
        echo "Rexo Code is available:"
        echo "  $(command -v rexo)"

    else
        echo "Rexo Code was installed successfully."
    fi

else

    echo "Rexo Code was installed successfully."
    echo ""
    echo "Your current shell does not have the new PATH yet."

    if [[ -n "$RC_FILE" ]]; then
        echo "Run:"
        echo ""
        echo "  source \"$RC_FILE\""
    fi

fi

# ============================================================
# Done
# ============================================================

echo ""
echo "----------------------------------------"
echo "Rexo Code installation complete."
echo "----------------------------------------"
echo ""

if [[ "$PATH_UPDATED" -eq 1 ]]; then
    echo "Open a new terminal or reload your shell:"
    echo ""

    if [[ -n "$RC_FILE" ]]; then
        echo "  source \"$RC_FILE\""
    fi

    echo ""
fi

echo "Then run:"
echo ""
echo "  rexo"
echo ""
echo "Enjoy Rexo Code."