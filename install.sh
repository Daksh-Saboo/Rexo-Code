#!/usr/bin/env bash
#
# install.sh — Linux/macOS counterpart to install.ps1.
#
# Builds Rexo Code and puts `rexo` on your PATH, so it runs from any
# directory instead of failing with "rexo: command not found".
#
# Works two ways:
#   - From a downloaded release archive (a flat folder containing `rexo`
#     right next to this script) — just copies that binary, never builds.
#   - From a source checkout (this script next to Cargo.toml) — builds
#     with `cargo build --release` first (skipped with --skip-build if
#     target/release/rexo already exists).
#
# Usage:
#   ./install.sh                 # release archive: install; source tree: build + install
#   ./install.sh --skip-build    # source tree only: reuse target/release/rexo
#
# Install directory, in order of preference: $REXO_INSTALL_DIR if you set
# it, else ~/.local/bin (created if needed — XDG convention, already on
# PATH by default on most modern Linux distros), else ~/.rexo/bin as a
# last resort. If it's not already on PATH, exactly one
# `export PATH="...":$PATH` line gets appended to whichever shell rc file
# matches your current $SHELL — nothing else in that file is touched.
#
# Uninstall with ./uninstall.sh.

set -euo pipefail

SKIP_BUILD=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1 ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg (see --help)" >&2
      exit 1
      ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# A release archive ships the binary flat, right next to this script —
# see release.yml's packaging step. If it's there, use it directly and
# skip the whole cargo/source-tree path entirely; there's no Cargo.toml
# to build from inside a downloaded archive, so even --skip-build can't
# fall through to "try building anyway" the way it used to.
if [ -f "$REPO_ROOT/rexo" ]; then
  RELEASE_BIN="$REPO_ROOT/rexo"
else
  RELEASE_BIN="$REPO_ROOT/target/release/rexo"

  if [ "$SKIP_BUILD" -eq 0 ] || [ ! -f "$RELEASE_BIN" ]; then
    if ! command -v cargo >/dev/null 2>&1; then
      echo "cargo wasn't found on PATH. Install Rust from https://rustup.rs first," >&2
      echo "then re-run this script from a new shell." >&2
      exit 1
    fi

    echo "Building Rexo Code (release)..."
    (cd "$REPO_ROOT" && cargo build --release)
  fi
fi

if [ ! -f "$RELEASE_BIN" ]; then
  echo "$RELEASE_BIN wasn't found. Something's off — check the cargo output above," >&2
  echo "or confirm this script is sitting next to the 'rexo' binary if you downloaded a release." >&2
  exit 1
fi

# --- Pick an install directory -----------------------------------------
if [ -n "${REXO_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$REXO_INSTALL_DIR"
elif mkdir -p "$HOME/.local/bin" 2>/dev/null; then
  INSTALL_DIR="$HOME/.local/bin"
else
  INSTALL_DIR="$HOME/.rexo/bin"
  mkdir -p "$INSTALL_DIR"
fi

TARGET="$INSTALL_DIR/rexo"
cp -f "$RELEASE_BIN" "$TARGET"
chmod +x "$TARGET"
echo "Installed: $TARGET"

# --- Make sure $INSTALL_DIR is on PATH ----------------------------------
case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    echo "Already on your PATH: $INSTALL_DIR"
    ;;
  *)
    SHELL_NAME="$(basename "${SHELL:-bash}")"
    LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
    case "$SHELL_NAME" in
      fish)
        RC_FILE="$HOME/.config/fish/config.fish"
        LINE="set -gx PATH $INSTALL_DIR \$PATH"
        ;;
      zsh)
        RC_FILE="$HOME/.zshrc"
        ;;
      *)
        RC_FILE="$HOME/.bashrc"
        ;;
    esac
    mkdir -p "$(dirname "$RC_FILE")"
    if [ -f "$RC_FILE" ] && grep -qxF "$LINE" "$RC_FILE"; then
      echo "PATH line already present in $RC_FILE"
    else
      {
        echo ""
        echo "# Added by Rexo Code's install.sh"
        echo "$LINE"
      } >> "$RC_FILE"
      echo "Added to PATH via $RC_FILE: $INSTALL_DIR"
    fi
    echo "(only this one line was added — nothing else in $RC_FILE was touched)"
    ;;
esac

echo ""
echo "Done. Open a NEW terminal window (or run 'source $RC_FILE') and run:"
echo ""
echo "    rexo"
echo ""
echo "To remove it later, run ./uninstall.sh from this same folder."
