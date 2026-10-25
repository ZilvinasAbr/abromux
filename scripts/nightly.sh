#!/usr/bin/env bash
# Build the working tree as-is into /Applications/AbroMux Nightly.app (neon icon,
# own socket and session) and point an `abromux-nightly` CLI at it. The release
# AbroMux.app and `abromux` are left alone.
#
#   scripts/nightly.sh                 # /Applications
#   scripts/nightly.sh ~/Applications  # or another folder
set -euo pipefail

cd "$(dirname "$0")/.."
dest="${1:-/Applications}"

scripts/bundle-app.sh --nightly "$dest"
ln -sf "$dest/AbroMux Nightly.app/Contents/MacOS/abromux" ~/.cargo/bin/abromux-nightly

if pgrep -qf "AbroMux Nightly.app/Contents/MacOS/abromux"; then
  echo "AbroMux Nightly is running: quit it (⌘Q) and reopen to use the new build."
fi
