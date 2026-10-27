#!/usr/bin/env bash
# Builds libghostty-vt (static) into vendor/ghostty-vt-out using a local Zig.
set -euo pipefail
cd "$(dirname "$0")/.."
GHOSTTY_REV=${GHOSTTY_REV:-v1.4.0}
ZIG_VERSION=0.17.0

mkdir -p vendor
if [ ! -x vendor/zig/zig ]; then
  mkdir -p vendor/zig
  curl -sL "https://ziglang.org/download/$ZIG_VERSION/zig-aarch64-macos-$ZIG_VERSION.tar.xz" | tar xJ --strip-components=1 -C vendor/zig
fi
if [ ! -d vendor/ghostty ]; then
  git clone https://github.com/ghostty-org/ghostty.git vendor/ghostty
fi
git -C vendor/ghostty checkout -q "$GHOSTTY_REV"
(cd vendor/ghostty && ../zig/zig build -Demit-lib-vt -Doptimize=ReleaseFast -Demit-xcframework=false --prefix ../ghostty-vt-out)
ls -la vendor/ghostty-vt-out/lib/libghostty-vt.a
