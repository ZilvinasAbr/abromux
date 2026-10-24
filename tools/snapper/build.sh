#!/usr/bin/env bash
# Builds tools/snapper/Snapper.app once. Rebuilding changes its signature and
# macOS will ask for Screen Recording permission again, so only build if missing.
set -euo pipefail
cd "$(dirname "$0")"
[ -d Snapper.app ] && { echo "Snapper.app exists"; exit 0; }
mkdir -p Snapper.app/Contents/MacOS
swiftc -O main.swift -o Snapper.app/Contents/MacOS/Snapper
cat > Snapper.app/Contents/Info.plist <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.abromux.snapper</string>
<key>CFBundleName</key><string>Snapper</string>
<key>CFBundleExecutable</key><string>Snapper</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
<key>LSBackgroundOnly</key><true/>
</dict></plist>
PLIST
codesign -s - --force --identifier dev.abromux.snapper Snapper.app
echo built
