#!/usr/bin/env bash
# Installs an unpacked ntls into ~/.local so a desktop can find it. No root
# needed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${1:-$HOME/.local}"

install -Dm755 "$here/ntls" "$prefix/bin/ntls"
install -Dm644 "$here/ntls.desktop" "$prefix/share/applications/ntls.desktop"
install -Dm644 "$here/ntls.png" "$prefix/share/icons/hicolor/512x512/apps/ntls.png"

echo "Installed to $prefix/bin. Make sure that is on your PATH."
