#!/usr/bin/env bash
# Builds "Reign Hub.app" for Apple Silicon and Intel Macs in one universal
# app, and zips it as dist/reign-macos.zip: the file users download and the
# hub updates itself from. The hub starts the editor (Contents/MacOS/
# reign-engine) for whichever project you open.
#
# usage: scripts/bundle-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

targets=(aarch64-apple-darwin x86_64-apple-darwin)
rustup target add "${targets[@]}"
for target in "${targets[@]}"; do
  cargo build --release --target "$target" -p reign_engine -p reign_hub
done

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
app="dist/Reign Hub.app"
rm -rf "$app" dist/reign-macos.zip
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

for program in reign-hub reign-engine; do
  lipo -create -output "$app/Contents/MacOS/$program" \
    "target/aarch64-apple-darwin/release/$program" \
    "target/x86_64-apple-darwin/release/$program"
done
cp LICENSE hub/assets/fonts/*-OFL.txt "$app/Contents/Resources/"

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

# Apple Silicon only runs signed code; an ad-hoc signature is enough for an
# app that isn't notarised.
codesign --force --deep --sign - "$app"
ditto -c -k --keepParent "$app" dist/reign-macos.zip
echo "Built $app and dist/reign-macos.zip"
