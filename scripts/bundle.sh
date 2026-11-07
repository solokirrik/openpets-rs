#!/usr/bin/env bash
# Builds a release binary and wraps it in an ad-hoc signed OpenPets.app.
# Usage: scripts/bundle.sh [output-dir]   (default: target/bundle)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/target/bundle}"
app="$out/OpenPets.app"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

cargo build --release --manifest-path "$root/Cargo.toml"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$root/target/release/openpets-rs" "$app/Contents/MacOS/openpets-rs"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>OpenPets</string>
  <key>CFBundleDisplayName</key><string>OpenPets</string>
  <key>CFBundleIdentifier</key><string>dev.openpets.rs</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleExecutable</key><string>openpets-rs</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --sign - "$app"
echo "Built $app"
