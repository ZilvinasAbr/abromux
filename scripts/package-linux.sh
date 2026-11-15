#!/usr/bin/env bash
# Build a release binary and install it as a Linux desktop app, or package it
# for a release. The Linux counterpart of scripts/bundle-app.sh.
#
#   scripts/package-linux.sh                    # installs into ~/.local
#   scripts/package-linux.sh --prefix /usr/local
#   scripts/package-linux.sh --nightly [--name zoom]  # abromux-nightly(-zoom), its own
#                                               # app id, icon and folders (scripts/nightly.sh)
#   scripts/package-linux.sh --dist             # dist/abromux-<version>-<arch>-linux.tar.gz
#                                               # and dist/AbroMux-<version>-<arch>.AppImage
#   scripts/package-linux.sh --dist --snapshot  # the same from any tree, named with its
#                                               # version (0.30.0+4793f8c), to try them
#   scripts/package-linux.sh --dist --no-appimage  # the tarball only
#
# An install is the binary in <prefix>/bin, a desktop entry named after the
# app id (dev.abromux.app.desktop, which Wayland compositors match the
# window's app_id against) and the icon in the hicolor theme.
#
# --dist ships only a clean checkout of a release tag, like bundle-app.sh:
# bump `version` in Cargo.toml, commit, `git tag v<version>`, then build. It
# writes a .sha256 next to each file; upload them to the GitHub release
# (`gh release upload v<version> dist/*`), whose AppImage the updater
# installs. The binary needs the glibc it was built against or newer, so
# build releases on the oldest distribution they should run on.
# The AppImage needs appimagetool, fetched once into vendor/.
set -euo pipefail
cd "$(dirname "$0")/.."

dist=false
snapshot=false
appimage=true
nightly=false
flavor=
prefix="$HOME/.local"
while (($#)); do
  case "$1" in
    --dist) dist=true ;;
    --snapshot) snapshot=true ;;
    --no-appimage) appimage=false ;;
    --nightly) nightly=true ;;
    --name) flavor="${2:-}"; shift ;;
    --prefix) prefix="${2:?--prefix needs a folder}"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
if [[ "$(uname -s)" != Linux ]]; then
  echo "package-linux.sh is for Linux; on macOS use scripts/bundle-app.sh." >&2
  exit 1
fi
if $dist && $nightly; then
  echo "--dist builds releases; nightlies are built locally." >&2
  exit 1
fi

# Names per channel, as channel.rs works them out.
if $nightly; then
  export ABROMUX_CHANNEL="nightly${flavor:+-$flavor}"
  id="dev.abromux.nightly${flavor:+.$flavor}"
  name="AbroMux Nightly${flavor:+ $flavor}"
  bin="abromux-nightly${flavor:+-$flavor}"
  svg=assets/app/abromux-nightly.svg
  png=assets/app/abromux-nightly.png
else
  unset ABROMUX_CHANNEL
  id=dev.abromux.app
  name=AbroMux
  bin=abromux
  svg=assets/app/abromux.svg
  png=assets/app/abromux.png
fi

[[ -f vendor/ghostty-vt-out/lib/libghostty-vt.a ]] || scripts/build-libghostty.sh
cargo build --release
version="$(target/release/abromux version | cut -d' ' -f2)"
if $dist && ! $snapshot && [[ "$version" == *+* ]]; then
  echo "Refusing to ship $version: --dist needs a clean checkout of tag v${version%%+*}." >&2
  exit 1
fi
arch="$(uname -m)"

# The desktop entry. `abromux <folder>` opens it as a workspace (and starts
# the app if it isn't running), so file managers can open folders with it.
desktop_entry() {
  cat <<EOF
[Desktop Entry]
Type=Application
Name=$name
GenericName=Terminal Workspaces
Comment=Terminal workspaces for coding agents
Exec=$1 %F
Icon=$id
Terminal=false
Categories=System;TerminalEmulator;
Keywords=terminal;shell;agents;claude;codex;
StartupWMClass=$id
StartupNotify=true
EOF
}

# The icon at `size` pixels, from the 1024 px PNG, with ImageMagick if it's
# there; the SVG covers every size anyway.
icon_png() {
  local size=$1 out=$2
  if command -v magick >/dev/null; then
    magick "$png" -resize "${size}x${size}" "$out"
  elif command -v convert >/dev/null; then
    convert "$png" -resize "${size}x${size}" "$out"
  else
    return 1
  fi
}

# Lay out bin/, share/applications/ and share/icons/ under $1.
install_tree() {
  local root=$1 exec_path=$2
  install -Dm755 target/release/abromux "$root/bin/$bin"
  mkdir -p "$root/share/applications"
  desktop_entry "$exec_path" >"$root/share/applications/$id.desktop"
  install -Dm644 "$svg" "$root/share/icons/hicolor/scalable/apps/$id.svg"
  for size in 64 128 256 512; do
    local dir="$root/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    icon_png "$size" "$dir/$id.png" || rmdir "$dir" 2>/dev/null || true
  done
}

if ! $dist; then
  install_tree "$prefix" "$prefix/bin/$bin"
  # Refresh the desktop's menus where the tool is there.
  command -v update-desktop-database >/dev/null &&
    update-desktop-database -q "$prefix/share/applications" || true
  echo "Installed $name $version: $prefix/bin/$bin, $id.desktop"
  if pgrep -f "$prefix/bin/$bin\$" >/dev/null; then
    echo "$name is running: quit it and open it again to use the new build."
  fi
  exit 0
fi

mkdir -p dist
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The tarball: the same tree as an install, to unpack into a prefix
# (`tar -xzf … -C ~/.local --strip-components=1`).
stem="abromux-$version-$arch-linux"
install_tree "$tmp/$stem" "$bin"
cp LICENSE README.md "$tmp/$stem/"
tar -C "$tmp" -czf "dist/$stem.tar.gz" "$stem"

files=("dist/$stem.tar.gz")

# The AppImage: an AppDir with the binary as AppRun. Wayland, Vulkan, fonts
# and the rest come from the system, as for the tarball.
if $appimage; then
  appdir="$tmp/AppDir"
  install_tree "$appdir/usr" "$bin"
  ln -s "usr/bin/$bin" "$appdir/AppRun"
  cp "$appdir/usr/share/applications/$id.desktop" "$appdir/"
  if [[ -f "$appdir/usr/share/icons/hicolor/256x256/apps/$id.png" ]]; then
    cp "$appdir/usr/share/icons/hicolor/256x256/apps/$id.png" "$appdir/$id.png"
  else
    cp "$svg" "$appdir/$id.svg"
  fi
  tool="vendor/appimagetool-$arch.AppImage"
  if [[ ! -x "$tool" ]]; then
    mkdir -p vendor
    curl -sfL -o "$tool" \
      "https://github.com/AppImage/appimagetool/releases/download/1.9.0/appimagetool-$arch.AppImage"
    chmod +x "$tool"
  fi
  image="dist/AbroMux-$version-$arch.AppImage"
  rm -f "$image"
  ARCH="$arch" "$tool" --appimage-extract-and-run --no-appstream "$appdir" "$image" >/dev/null

  files+=("$image")
fi

for f in "${files[@]}"; do
  (cd dist && sha256sum "$(basename "$f")" >"$(basename "$f").sha256")
done
echo "Built ${files[*]} ($version)"
