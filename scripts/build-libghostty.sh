#!/usr/bin/env bash
# Builds libghostty-vt (static) into vendor/ghostty-vt-out using a local Zig.
set -euo pipefail
cd "$(dirname "$0")/.."
GHOSTTY_REV=${GHOSTTY_REV:-vt-1.0.0}
ZIG_VERSION=0.17.0

case "$(uname -sm)" in
  "Darwin arm64") ZIG_HOST=aarch64-macos ;;
  "Linux x86_64") ZIG_HOST=x86_64-linux ;;
  "Linux aarch64" | "Linux arm64") ZIG_HOST=aarch64-linux ;;
  *) echo "build-libghostty.sh: unsupported host $(uname -sm)" >&2; exit 1 ;;
esac

mkdir -p vendor
if [ ! -x vendor/zig/zig ]; then
  mkdir -p vendor/zig
  curl -sfL "https://ziglang.org/download/$ZIG_VERSION/zig-$ZIG_HOST-$ZIG_VERSION.tar.xz" | tar xJ --strip-components=1 -C vendor/zig
fi
if [ ! -d vendor/ghostty ]; then
  git clone https://github.com/ghostty-org/ghostty.git vendor/ghostty
fi
# An older clone may not have the tag yet.
git -C vendor/ghostty rev-parse -q --verify "$GHOSTTY_REV^{commit}" >/dev/null ||
  git -C vendor/ghostty fetch -q --tags origin
git -C vendor/ghostty checkout -q "$GHOSTTY_REV"
(cd vendor/ghostty && ../zig/zig build -Demit-lib-vt -Doptimize=ReleaseFast -Demit-xcframework=false --prefix ../ghostty-vt-out)
ls -la vendor/ghostty-vt-out/lib/libghostty-vt.a
