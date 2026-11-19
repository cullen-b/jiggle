#!/bin/sh
# Build Jiggle.app into ~/Applications and start it at login via a LaunchAgent.
set -eu
cd "$(dirname "$0")"
taskpolicy -b cargo build --release

app="$HOME/Applications/Jiggle.app"
agent="$HOME/Library/LaunchAgents/local.jiggle.plist"
launchctl bootout "gui/$(id -u)" "$agent" 2>/dev/null || true

mkdir -p "$app/Contents/MacOS" "$HOME/Library/LaunchAgents"
cp target/release/jiggle "$app/Contents/MacOS/jiggle"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>local.jiggle</string>
  <key>CFBundleName</key><string>Jiggle</string>
  <key>CFBundleExecutable</key><string>jiggle</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
</dict></plist>
PLIST
codesign --force --sign - --identifier local.jiggle "$app"

cat > "$agent" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>local.jiggle</string>
  <key>ProgramArguments</key><array><string>$app/Contents/MacOS/jiggle</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
</dict></plist>
PLIST
launchctl bootstrap "gui/$(id -u)" "$agent"
echo "Jiggle installed. Allow it under System Settings > Privacy & Security > Accessibility."
