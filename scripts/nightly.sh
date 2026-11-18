#!/usr/bin/env bash
# Build the working tree as-is into /Applications/AbroMux Nightly.app (neon icon,
# own socket and session) and point an `abromux-nightly` CLI at it. The release
# AbroMux.app and `abromux` are left alone. On Linux it installs
# abromux-nightly into ~/.local instead (scripts/package-linux.sh --nightly).
#
#   scripts/nightly.sh                 # /Applications
#   scripts/nightly.sh ~/Applications  # or another folder
#   scripts/nightly.sh --name zoom     # "AbroMux Nightly zoom.app" and `abromux-nightly-zoom`,
#                                      # apart from the shared nightly too
#   scripts/nightly.sh --no-chromium   # without the built-in Chromium engine
set -euo pipefail

cd "$(dirname "$0")/.."
dest=/Applications
flavor=
extra=()
while (($#)); do
  case "$1" in
    --name) flavor="${2:-}"; shift ;;
    --no-chromium) extra+=(--no-chromium) ;;
    *) dest="$1" ;;
  esac
  shift
done
app="AbroMux Nightly${flavor:+ $flavor}"

# Linux: abromux-nightly in ~/.local/bin (or the folder given), with its own
# desktop entry, icon and folders.
if [[ "$(uname -s)" == Linux ]]; then
  [[ "$dest" == /Applications ]] && dest="$HOME/.local"
  exec scripts/package-linux.sh --nightly ${flavor:+--name "$flavor"} --prefix "$dest"
fi

scripts/bundle-app.sh --nightly ${flavor:+--name "$flavor"} ${extra[@]+"${extra[@]}"} "$dest"
ln -sf "$dest/$app.app/Contents/MacOS/abromux" ~/.cargo/bin/abromux-nightly${flavor:+-$flavor}

if pgrep -qf "$app.app/Contents/MacOS/abromux"; then
  echo "$app is running: quit it (⌘Q) and reopen to use the new build."
fi
