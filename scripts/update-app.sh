#!/usr/bin/env bash
# Pull the latest code, rebuild /Applications/AbroMux.app and point the `abromux`
# CLI at it. A running abromux keeps the old version until you quit and reopen it.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -z "$(git status --porcelain)" ]]; then
  git pull --ff-only
else
  echo "Working tree has changes; building it as-is without pulling."
fi

scripts/bundle-app.sh /Applications
ln -sf /Applications/AbroMux.app/Contents/MacOS/abromux ~/.cargo/bin/abromux

echo "abromux $(git rev-parse --short HEAD) installed."
if pgrep -qx abromux; then
  echo "abromux is running: quit it (⌘Q) and reopen to use the new version."
fi
