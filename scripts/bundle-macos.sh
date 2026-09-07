#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
bundle="target/Pasteface.app/Contents"
mkdir -p "$bundle/MacOS"
cp target/release/pasteface "$bundle/MacOS/pasteface"
cat > "$bundle/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Pasteface</string>
<key>CFBundleDisplayName</key><string>Pasteface</string>
<key>CFBundleIdentifier</key><string>dev.blairjordan.pasteface</string>
<key>CFBundleExecutable</key><string>pasteface</string>
<key>CFBundleVersion</key><string>0.1.0</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSMicrophoneUsageDescription</key><string>Pasteface records your voice for local transcription when you press Record.</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --deep --sign - target/Pasteface.app
printf 'Created target/Pasteface.app. Open it to grant microphone permission.\n'
