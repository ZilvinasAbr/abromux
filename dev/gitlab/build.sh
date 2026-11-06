#!/usr/bin/env bash
# Build the debug abromux that dev/gitlab/app.sh runs: the normal debug build,
# but on a copy of gpui that can screenshot itself offscreen (see
# gpui-capture.py), in its own target dir. Cargo.lock is left as it was.
set -euo pipefail
cd "$(dirname "$0")/../.."
src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/gpui-0.2.2 | head -1)
gpui="target/gpui-capture"
if [ ! -f "$gpui/.patched" ] || [ dev/gitlab/gpui-capture.py -nt "$gpui/.patched" ]; then
  python3 -I dev/gitlab/gpui-capture.py "$src" "$gpui" >/dev/null
  touch "$gpui/.patched"
fi
cp Cargo.lock target/Cargo.lock.keep
trap 'cp target/Cargo.lock.keep Cargo.lock' EXIT
CARGO_TARGET_DIR=target/capture cargo build \
  --config "patch.crates-io.gpui.path='$gpui'" "$@"
