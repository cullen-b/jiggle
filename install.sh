#!/bin/sh
# Install the latest signed Jiggle.app release into ~/Applications and start it at login.
# --source builds this checkout instead (ad-hoc signed; auto-update swaps in the next signed release).
set -eu
cd "$(dirname "$0")"
app="$HOME/Applications/Jiggle.app"
agent="$HOME/Library/LaunchAgents/local.jiggle.plist"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ "${1:-}" = "--source" ]; then
  taskpolicy -b cargo build --release
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
  scripts/bundle.sh target/release/jiggle "$tmp/Jiggle.app" "$version"
  codesign --force --sign - --identifier local.jiggle "$tmp/Jiggle.app"
else
  curl -fsSL -o "$tmp/Jiggle.zip" https://github.com/cullen-b/jiggle/releases/latest/download/Jiggle.zip
  ditto -x -k "$tmp/Jiggle.zip" "$tmp"
  codesign --verify --deep --strict -R="$(cat scripts/requirement.txt)" "$tmp/Jiggle.app"
fi

launchctl bootout "gui/$(id -u)" "$agent" 2>/dev/null || true
mkdir -p "$HOME/Applications" "$HOME/Library/LaunchAgents"
rm -rf "$app"
ditto "$tmp/Jiggle.app" "$app"

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
