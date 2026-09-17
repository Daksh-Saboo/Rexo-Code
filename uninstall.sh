#!/usr/bin/env bash
#
# uninstall.sh — removes what install.sh did.
#
# Deletes the installed `rexo` binary and the PATH line install.sh added
# to your shell rc file. Doesn't touch this checkout, your rexo.toml, or
# anything else on PATH.
#
# Usage: ./uninstall.sh

set -euo pipefail

if [ -n "${REXO_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$REXO_INSTALL_DIR"
elif [ -f "$HOME/.local/bin/rexo" ]; then
  INSTALL_DIR="$HOME/.local/bin"
elif [ -f "$HOME/.rexo/bin/rexo" ]; then
  INSTALL_DIR="$HOME/.rexo/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
fi

TARGET="$INSTALL_DIR/rexo"

if [ -f "$TARGET" ]; then
  rm -f "$TARGET"
  echo "Removed: $TARGET"
else
  echo "Nothing installed at $TARGET (already removed?)"
fi

if [ -d "$INSTALL_DIR" ] && [ "$INSTALL_DIR" = "$HOME/.rexo/bin" ] && [ -z "$(ls -A "$INSTALL_DIR" 2>/dev/null)" ]; then
  rmdir "$INSTALL_DIR" 2>/dev/null || true
fi

LINE_MATCH="export PATH=\"$INSTALL_DIR:\$PATH\""
FISH_LINE_MATCH="set -gx PATH $INSTALL_DIR \$PATH"
for RC_FILE in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.config/fish/config.fish"; do
  [ -f "$RC_FILE" ] || continue
  if grep -qxF "$LINE_MATCH" "$RC_FILE" 2>/dev/null || grep -qxF "$FISH_LINE_MATCH" "$RC_FILE" 2>/dev/null; then
    # Remove the marker comment and the line right after it, in place.
    tmp="$(mktemp)"
    awk -v line="$LINE_MATCH" -v fline="$FISH_LINE_MATCH" '
      $0 == "# Added by Rexo Code'\''s install.sh" { skip_next=1; next }
      skip_next { skip_next=0; if ($0 == line || $0 == fline) next }
      { print }
    ' "$RC_FILE" > "$tmp"
    mv "$tmp" "$RC_FILE"
    echo "Removed PATH entry from $RC_FILE"
  fi
done

echo ""
echo "Done. Open a new terminal window for the PATH change to take effect."
