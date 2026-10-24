#!/usr/bin/env bash
# Build a release binary and wrap it in AbroMux.app.
#
#   scripts/bundle-app.sh                 # installs /Applications/AbroMux.app
#   scripts/bundle-app.sh ~/Applications  # or another folder
#
# The `abromux` CLI on PATH can point at the bundled binary:
#   ln -sf /Applications/AbroMux.app/Contents/MacOS/abromux ~/.cargo/bin/abromux
set -euo pipefail

cd "$(dirname "$0")/.."
dest="${1:-/Applications}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

cargo build --release

app="$(mktemp -d)/AbroMux.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/abromux "$app/Contents/MacOS/abromux"
cp assets/app/abromux.icns "$app/Contents/Resources/abromux.icns"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>AbroMux</string>
  <key>CFBundleDisplayName</key><string>AbroMux</string>
  <key>CFBundleIdentifier</key><string>dev.abromux.app</string>
  <key>CFBundleExecutable</key><string>abromux</string>
  <key>CFBundleIconFile</key><string>abromux</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
EOF

# Ad-hoc sign so Gatekeeper and TCC treat it as one stable app.
codesign --force --deep --sign - "$app"

rm -rf "$dest/AbroMux.app"
mv "$app" "$dest/AbroMux.app"
# Make Finder and the Dock pick up the new icon instead of a cached one.
touch "$dest/AbroMux.app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$dest/AbroMux.app" || true
echo "Installed $dest/AbroMux.app ($version)"
