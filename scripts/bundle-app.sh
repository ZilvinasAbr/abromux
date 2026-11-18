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
#   scripts/bundle-app.sh --chromium-edition [dir]  # "AbroMux Chromium.app" (dev.abromux.chromium)
#   scripts/bundle-app.sh --chromium-edition --dist # notarized dist/AbroMux-Chromium-<version>.dmg,
#                                           # from tag v<version>-chromium on the chromium branch
#   scripts/bundle-app.sh --nightly --name zoom [dir]  # "AbroMux Nightly zoom.app"
#   scripts/bundle-app.sh --nightly --dist  # notarized dist/AbroMux-Nightly-<version>.dmg,
#                                           # from a committed tree, for a GitHub pre-release
#
# The nightly app has its own bundle id, name, neon icon and support folder,
# so it runs next to the release one. A named nightly gets its own again, so
# several can run beside the shared one.
#
# Signs with the first "Developer ID Application" certificate in the keychain
# (or $ABROMUX_SIGN_IDENTITY), falling back to an ad-hoc signature. --dist
# needs the certificate and notarytool credentials stored once with:
#   xcrun notarytool store-credentials abromux-notary --apple-id <id> --team-id <team>
# ($ABROMUX_NOTARY_PROFILE picks another profile name.)
#
# Passkeys in browser panes need the restricted web-browser public-key
# credential entitlement, which only a provisioning profile can grant. Put the
# Developer ID profile for the bundle id at assets/app/<bundle id>.provisionprofile
# (or $ABROMUX_PROVISION_PROFILE) and it's embedded and its entitlements signed
# in. Without one the app is signed as before and WebAuthn is left to WebKit.
# docs/guide.md ("Passkeys") has the steps.
#
# The built-in Chromium engine (browser.engine "chromium") is in the bundle
# too: CEF's framework (~340 MB) in Contents/Frameworks, with the helper apps
# Chromium runs its renderer, GPU and other processes as. The first build
# downloads CEF (set CEF_PATH to keep one copy for every checkout) and needs
# cmake and ninja. --no-chromium leaves it out; that app falls back to WebKit.
#
# The `abromux` CLI on PATH can point at the bundled binary:
#   ln -sf /Applications/AbroMux.app/Contents/MacOS/abromux ~/.cargo/bin/abromux
set -euo pipefail

cd "$(dirname "$0")/.."
dist=false
nightly=false
dest=/Applications
flavor=
chromium=true
edition=false
while (($#)); do
  case "$1" in
    --dist) dist=true ;;
    --nightly) nightly=true ;;
    --no-chromium) chromium=false ;;
    --chromium-edition) edition=true ;;
    --name) flavor="${2:-}"; shift ;;
    *) dest="$1" ;;
  esac
  shift
done
if $dist && $nightly && [[ -n "$flavor" ]]; then
  echo "--dist builds the shared nightly only, not a --name one." >&2
  exit 1
fi
if [[ -n "$flavor" ]] && { ! $nightly || [[ ! "$flavor" =~ ^[a-z0-9-]+$ ]]; }; then
  echo "--name needs --nightly and a name of a-z, 0-9 and -." >&2
  exit 1
fi
if $edition && { $nightly || ! $chromium; }; then
  echo "--chromium-edition is a release with Chromium: not with --nightly or --no-chromium." >&2
  exit 1
fi
if $edition; then
  # Its own app beside AbroMux: name, bundle id, support folder, releases.
  name="AbroMux Chromium"
  bundle_id=dev.abromux.chromium
  icon=abromux
elif $nightly; then
  name="AbroMux Nightly${flavor:+ $flavor}"
  bundle_id="dev.abromux.nightly${flavor:+.$flavor}"
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

if $chromium; then
  cargo build --release --features chromium
  cargo build --release -p abromux-chromium-helper
else
  cargo build --release
fi
# The binary's own version (0.1.0 on a release tag, else 0.1.0+<sha>[-dirty]), and the commit count as the
# build number macOS compares to tell newer from older.
version="$(target/release/abromux version | cut -d' ' -f2)"
build="$(git rev-list --count HEAD 2>/dev/null || echo 0)"
if $dist && $nightly && [[ "$version" == *-dirty ]]; then
  echo "Refusing to ship $version: commit first, so the nightly names a commit." >&2
  exit 1
fi
if $dist && ! $nightly && [[ "$version" == *+* ]]; then
  if $edition; then
    echo "Refusing to ship $version: --dist needs a clean checkout of tag v${version%%+*}-chromium." >&2
  else
    echo "Refusing to ship $version: --dist needs a clean checkout of tag v${version%%+*}." >&2
  fi
  exit 1
fi
if $dist && $edition && [[ "$(git rev-parse HEAD)" != "$(git rev-parse "v$version-chromium^{commit}" 2>/dev/null)" ]]; then
  echo "Refusing to ship: the Chromium edition ships from tag v$version-chromium." >&2
  exit 1
fi

# Files abromux can open in its editor (src/editor.rs), offered in Finder's
# Open With but never made their default on their own (rank Alternate):
# text and source types, plus code extensions macOS knows no type for (.rs,
# .toml) or another one (.ts is an MPEG stream to it).
extensions="$(target/release/abromux default-app list | sed 's|.*|      <string>&</string>|')"

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
  <key>NSBluetoothAlwaysUsageDescription</key><string>$name uses Bluetooth to sign in to websites with a passkey on your phone.</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Text and source file</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.text</string>
        <string>public.plain-text</string>
        <string>public.source-code</string>
        <string>public.script</string>
        <string>public.shell-script</string>
        <string>public.json</string>
        <string>public.xml</string>
        <string>public.yaml</string>
        <string>net.daringfireball.markdown</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Code file</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>CFBundleTypeExtensions</key>
      <array>
$extensions
      </array>
    </dict>
  </array>
</dict>
</plist>
EOF

if $chromium; then
  # CEF's files, from $CEF_PATH or where cef-dll-sys's build put them.
  cef_dir=
  for d in ${CEF_PATH:+"$CEF_PATH"/*/cef_macos_* "$CEF_PATH"} target/release/build/cef-dll-sys-*/out/cef_macos_*; do
    if [[ -d "$d/Chromium Embedded Framework.framework" ]]; then cef_dir="$d"; break; fi
  done
  if [[ -z "$cef_dir" ]]; then
    echo "No Chromium Embedded Framework found (CEF_PATH or target/release/build)." >&2
    exit 1
  fi
  mkdir -p "$app/Contents/Frameworks"
  cp -R "$cef_dir/Chromium Embedded Framework.framework" "$app/Contents/Frameworks/"
  # The helpers: one binary, as the five apps Chromium looks for beside the
  # framework (crates/abromux-chromium-helper). Background apps: no Dock icon.
  for suffix in "" " (GPU)" " (Renderer)" " (Plugin)" " (Alerts)"; do
    helper="abromux Helper$suffix"
    id_suffix="$(echo "$suffix" | tr -d ' ()' | tr '[:upper:]' '[:lower:]')"
    happ="$app/Contents/Frameworks/$helper.app"
    mkdir -p "$happ/Contents/MacOS"
    cp target/release/abromux-chromium-helper "$happ/Contents/MacOS/$helper"
    cat > "$happ/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$name Helper$suffix</string>
  <key>CFBundleDisplayName</key><string>$name Helper$suffix</string>
  <key>CFBundleIdentifier</key><string>$bundle_id.helper${id_suffix:+.$id_suffix}</string>
  <key>CFBundleExecutable</key><string>$helper</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$build</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><string>1</string>
  <key>LSEnvironment</key><dict><key>MallocNanoZone</key><string>0</string></dict>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
  done
fi

entitlements=assets/app/abromux.entitlements
profile_src="${ABROMUX_PROVISION_PROFILE:-assets/app/$bundle_id.provisionprofile}"
if [[ -n "$identity" && -f "$profile_src" ]]; then
  # The profile's App ID and team must be signed in alongside the restricted
  # entitlement it grants, or macOS refuses to launch the app.
  security cms -D -i "$profile_src" > "$tmp/profile.plist"
  granted() { /usr/libexec/PlistBuddy -c "Print :Entitlements:$1" "$tmp/profile.plist" 2>/dev/null; }
  app_id="$(granted com.apple.application-identifier)"
  team_id="$(granted com.apple.developer.team-identifier)"
  if [[ "$app_id" != "$team_id.$bundle_id" ]]; then
    echo "$profile_src is for $app_id, not $bundle_id." >&2
    exit 1
  fi
  cp "$profile_src" "$app/Contents/embedded.provisionprofile"
  entitlements="$tmp/entitlements.plist"
  cp assets/app/abromux.entitlements "$entitlements"
  plist() { /usr/libexec/PlistBuddy -c "$1" "$entitlements"; }
  plist "Add :com.apple.application-identifier string $app_id"
  plist "Add :com.apple.developer.team-identifier string $team_id"
  if [[ "$(granted com.apple.developer.web-browser.public-key-credential)" == true ]]; then
    plist "Add :com.apple.developer.web-browser.public-key-credential bool true"
    echo "Passkeys on: $profile_src grants the web-browser entitlement."
  else
    echo "warning: $profile_src doesn't grant the web-browser entitlement; passkeys stay off." >&2
  fi
fi

# Chromium's parts are signed first, inside out, as the app's signature
# seals them. Under the hardened runtime the renderer and GPU helpers need
# JIT for V8 and the plugin helper unsigned memory (as Chrome's helpers do).
sign_nested() {
  local frameworks="$app/Contents/Frameworks"
  [[ -d "$frameworks/Chromium Embedded Framework.framework" ]] || return 0
  local args=(--force --sign "$1")
  [[ "$1" != - ]] && args+=(--options runtime --timestamp)
  local lib happ keys k
  for lib in "$frameworks/Chromium Embedded Framework.framework/Libraries/"*.dylib; do
    codesign "${args[@]}" "$lib"
  done
  codesign "${args[@]}" "$frameworks/Chromium Embedded Framework.framework"
  for happ in "$frameworks/abromux Helper"*.app; do
    case "$happ" in
      *"(Renderer).app"|*"(GPU).app") keys="com.apple.security.cs.allow-jit" ;;
      *"(Plugin).app") keys="com.apple.security.cs.allow-unsigned-executable-memory com.apple.security.cs.disable-library-validation" ;;
      *) keys= ;;
    esac
    {
      echo '<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict>'
      for k in $keys; do echo "<key>$k</key><true/>"; done
      echo '</dict></plist>'
    } > "$tmp/helper.entitlements"
    codesign "${args[@]}" --entitlements "$tmp/helper.entitlements" "$happ"
  done
}

if [[ -n "$identity" ]]; then
  sign_nested "$identity"
  # Hardened runtime and a secure timestamp are what notarization requires.
  codesign --force --sign "$identity" --options runtime --timestamp \
    --entitlements "$entitlements" "$app"
  echo "Signed with $identity"
else
  # Ad-hoc sign so Gatekeeper and TCC treat it as one stable app.
  sign_nested -
  codesign --force --sign - "$app"
  echo "No Developer ID certificate; signed ad-hoc."
fi
codesign --verify --strict "$app"

if $dist; then
  mkdir -p dist
  dmg="dist/${name// /-}-$version.dmg"
  ln -s /Applications "$tmp/Applications"
  rm -f "$dmg"
  hdiutil create -volname "$name" -srcfolder "$tmp" -fs HFS+ -format UDZO -quiet "$dmg"
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
