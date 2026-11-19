#!/bin/sh
# Usage: scripts/bundle.sh <binary> <out/Jiggle.app> <version>. Builds an unsigned app bundle.
set -eu
bin=$1 app=$2 version=$3
root=$(cd "$(dirname "$0")/.." && pwd)
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/jiggle"
cp "$root/assets/jiggle.icns" "$app/Contents/Resources/jiggle.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>local.jiggle</string>
  <key>CFBundleName</key><string>Jiggle</string>
  <key>CFBundleExecutable</key><string>jiggle</string>
  <key>CFBundleIconFile</key><string>jiggle</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
</dict></plist>
PLIST
