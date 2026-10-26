#!/usr/bin/env bash
# Build the working tree as-is into /Applications/AbroMux Nightly.app (neon icon,
# own socket and session) and point an `abromux-nightly` CLI at it. The release
# AbroMux.app and `abromux` are left alone.
#
#   scripts/nightly.sh                 # /Applications
#   scripts/nightly.sh ~/Applications  # or another folder
#   scripts/nightly.sh --name zoom     # "AbroMux Nightly zoom.app" and `abromux-nightly-zoom`,
#                                      # apart from the shared nightly too
set -euo pipefail

cd "$(dirname "$0")/.."
dest=/Applications
flavor=
while (($#)); do
  case "$1" in
    --name) flavor="${2:-}"; shift ;;
    *) dest="$1" ;;
  esac
  shift
done
app="AbroMux Nightly${flavor:+ $flavor}"

scripts/bundle-app.sh --nightly ${flavor:+--name "$flavor"} "$dest"
ln -sf "$dest/$app.app/Contents/MacOS/abromux" ~/.cargo/bin/abromux-nightly${flavor:+-$flavor}

if pgrep -qf "$app.app/Contents/MacOS/abromux"; then
  echo "$app is running: quit it (⌘Q) and reopen to use the new build."
fi
