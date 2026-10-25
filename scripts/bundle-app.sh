#!/usr/bin/env bash
# Build a release binary and wrap it in AbroMux.app.
#
#   scripts/bundle-app.sh                 # installs /Applications/AbroMux.app
#   scripts/bundle-app.sh ~/Applications  # or another folder
#   scripts/bundle-app.sh --dist          # notarized dist/AbroMux-<version>.dmg
#
# --dist ships only a clean checkout of a release tag: bump `version` in
# Cargo.toml, commit, `git tag v<version>`, then build.
#   scripts/bundle-app.sh --nightly [dir] # "AbroMux Nightly.app" (scripts/nightly.sh)
#
# The nightly app has its own bundle id, name, neon icon and support folder,
# so it runs next to the release one.
#
# Signs with the first "Developer ID Application" certificate in the keychain
# (or $ABROMUX_SIGN_IDENTITY), falling back to an ad-hoc signature. --dist
# needs the certificate and notarytool credentials stored once with:
#   xcrun notarytool store-credentials abromux-notary --apple-id <id> --team-id <team>
# ($ABROMUX_NOTARY_PROFILE picks another profile name.)
#
# The `abromux` CLI on PATH can point at the bundled binary:
#   ln -sf /Applications/AbroMux.app/Contents/MacOS/abromux ~/.cargo/bin/abromux
set -euo pipefail

cd "$(dirname "$0")/.."
dist=false
nightly=false
dest=/Applications
for arg in "$@"; do
  case "$arg" in
    --dist) dist=true ;;
    --nightly) nightly=true ;;
    *) dest="$arg" ;;
  esac
done
if $dist && $nightly; then
  echo "--dist builds the release app only." >&2
  exit 1
fi
if $nightly; then
  name="AbroMux Nightly"
  bundle_id=dev.abromux.nightly
  icon=abromux-nightly
else
  name=AbroMux
  bundle_id=dev.abromux.app
  icon=abromux
fi

identity="${ABROMUX_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
  sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -n1)}"
profile="${ABROMUX_NOTARY_PROFILE:-abromux-notary}"
if $dist && [[ -z "$identity" ]]; then
  echo "--dist needs a Developer ID Application certificate." >&2
  exit 1
fi

cargo build --release
# The binary's own version (0.1.0 on a release tag, else 0.1.0+<sha>[-dirty]), and the commit count as the
# build number macOS compares to tell newer from older.
version="$(target/release/abromux version | cut -d' ' -f2)"
build="$(git rev-list --count HEAD 2>/dev/null || echo 0)"
if $dist && [[ "$version" == *+* ]]; then
  echo "Refusing to ship $version: --dist needs a clean checkout of tag v${version%%+*}." >&2
  exit 1
fi

tmp="$(mktemp -d)"
app="$tmp/$name.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/abromux "$app/Contents/MacOS/abromux"
cp "assets/app/$icon.icns" "$app/Contents/Resources/abromux.icns"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleDisplayName</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleExecutable</key><string>abromux</string>
  <key>CFBundleIconFile</key><string>abromux</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$build</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSAppleEventsUsageDescription</key><string>A program running in $name wants to control other apps.</string>
  <key>NSCameraUsageDescription</key><string>A program running in $name wants to use the camera.</string>
  <key>NSMicrophoneUsageDescription</key><string>A program running in $name wants to use the microphone.</string>
</dict>
</plist>
EOF

if [[ -n "$identity" ]]; then
  # Hardened runtime and a secure timestamp are what notarization requires.
  codesign --force --sign "$identity" --options runtime --timestamp \
    --entitlements assets/app/abromux.entitlements "$app"
  echo "Signed with $identity"
else
  # Ad-hoc sign so Gatekeeper and TCC treat it as one stable app.
  codesign --force --sign - "$app"
  echo "No Developer ID certificate; signed ad-hoc."
fi
codesign --verify --strict "$app"

if $dist; then
  mkdir -p dist
  dmg="dist/AbroMux-$version.dmg"
  ln -s /Applications "$tmp/Applications"
  rm -f "$dmg"
  hdiutil create -volname AbroMux -srcfolder "$tmp" -fs HFS+ -format UDZO -quiet "$dmg"
  codesign --force --sign "$identity" --timestamp "$dmg"
  # Notarizing the disk image covers the app inside it too.
  xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait
  xcrun stapler staple "$dmg"
  spctl --assess --type open --context context:primary-signature -v "$dmg"
  rm -rf "$tmp"
  echo "Built $dmg ($version)"
  exit 0
fi

rm -rf "$dest/$name.app"
mv "$app" "$dest/$name.app"
rm -rf "$tmp"
# Make Finder and the Dock pick up the new icon instead of a cached one.
touch "$dest/$name.app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$dest/$name.app" || true
echo "Installed $dest/$name.app ($version)"
