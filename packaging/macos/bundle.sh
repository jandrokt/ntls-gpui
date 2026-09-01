#!/usr/bin/env bash
# Wraps an already-built binary in ntls.app.
#
#   packaging/macos/bundle.sh <binary> <output-dir> [build]
#
# Writes <output-dir>/ntls.app. `build` is the build number that ntls calls
# itself by. macOS wants two version strings; both get that number.
set -euo pipefail

binary="${1:?usage: bundle.sh <binary> <output-dir> [build]}"
out="${2:?usage: bundle.sh <binary> <output-dir> [build]}"
build="${3:-0}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
app="$out/ntls.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$binary" "$app/Contents/MacOS/ntls"
chmod +x "$app/Contents/MacOS/ntls"
cp "$root/packaging/icon/ntls.icns" "$app/Contents/Resources/ntls.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>ntls</string>
    <key>CFBundleDisplayName</key>
    <string>ntls</string>
    <key>CFBundleIdentifier</key>
    <string>net.ntls.app</string>
    <key>CFBundleExecutable</key>
    <string>ntls</string>
    <key>CFBundleIconFile</key>
    <string>ntls</string>
    <key>CFBundleVersion</key>
    <string>$build</string>
    <key>CFBundleShortVersionString</key>
    <string>$build</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <!-- The window is the whole of the interface; there is no document to
         restore and nothing to show before one opens. -->
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>NSLocalNetworkUsageDescription</key>
    <string>ntls sweeps and pings the network you point it at.</string>
</dict>
</plist>
PLIST

# Ad-hoc signature. Enough to run once quarantine is cleared, and free when
# there's no real signing identity around.
if command -v codesign >/dev/null 2>&1; then
    codesign --force --deep --sign - "$app" >/dev/null 2>&1 || \
        echo "note: could not ad-hoc sign the bundle" >&2
fi

echo "$app"
