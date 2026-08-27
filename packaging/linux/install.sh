#!/usr/bin/env bash
# Puts an unpacked ntls where a desktop will find it, for the tarball's own
# README to point at. Nothing here needs root: it all goes under ~/.local.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${1:-$HOME/.local}"

install -Dm755 "$here/ntls" "$prefix/bin/ntls"
install -Dm644 "$here/ntls.desktop" "$prefix/share/applications/ntls.desktop"
install -Dm644 "$here/ntls.png" "$prefix/share/icons/hicolor/512x512/apps/ntls.png"

echo "ntls is in $prefix/bin — make sure that is on your PATH."
