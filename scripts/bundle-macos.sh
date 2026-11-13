#!/usr/bin/env bash
# Builds the release hub and editor and wraps them in "Reign Hub.app" under
# dist/. The hub is the app's entry point; it starts the editor
# (Contents/MacOS/reign-engine) for whichever project you open.
#
# usage: scripts/bundle-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p reign_engine -p reign_hub
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
app="dist/Reign Hub.app"

rm -rf "$app" "dist/Reign Engine.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/reign-hub "$app/Contents/MacOS/reign-hub"
cp target/release/reign-engine "$app/Contents/MacOS/reign-engine"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Reign Hub</string>
  <key>CFBundleDisplayName</key><string>Reign Hub</string>
  <key>CFBundleIdentifier</key><string>com.lunarstudios.reign-hub</string>
  <key>CFBundleExecutable</key><string>reign-hub</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
cp target/release/reign-engine target/release/reign-hub dist/
echo "Bundled $app, dist/reign-hub and dist/reign-engine"
