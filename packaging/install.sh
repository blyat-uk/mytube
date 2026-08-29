#!/usr/bin/env bash
# Installs the MyTube binary and icon into the user's home. The .desktop file is
# left for you to copy to ~/.local/share/applications yourself.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_src="$root/src-tauri/target/release/mytube"

if [[ ! -x "$bin_src" ]]; then
  echo "Release binary not found at $bin_src" >&2
  echo "Build it first:  bun run tauri build" >&2
  exit 1
fi

install -Dm755 "$bin_src" "$HOME/.local/bin/mytube"
install -Dm644 "$root/src-tauri/icons/128x128.png" \
  "$HOME/.local/share/icons/hicolor/128x128/apps/mytube.png"

gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

echo "Installed:"
echo "  $HOME/.local/bin/mytube"
echo "  $HOME/.local/share/icons/hicolor/128x128/apps/mytube.png"
echo
echo "Now copy the launcher yourself:"
echo "  cp $root/packaging/mytube.desktop ~/.local/share/applications/"
