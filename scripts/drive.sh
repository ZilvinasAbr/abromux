#!/usr/bin/env bash
# Run abromux with a ABROMUX_SCRIPT, capturing `snap:` steps through Snapper.app
# (which holds the Screen Recording permission; see tools/snapper/build.sh).
# usage: ABROMUX_SCRIPT="type:ls;key:enter;wait:500;snap:/tmp/a.png;quit" scripts/drive.sh
cd "$(dirname "$0")/.."
SNAPPER="$PWD/tools/snapper/Snapper.app"
./target/debug/abromux | while read -r tag window path; do
  if [ "$tag" != SNAP ]; then echo "$tag $window $path"; continue; fi
  rm -f "$path"
  open -W --stderr /tmp/abromux-snapper.log -a "$SNAPPER" --args --window "$window" "$path"
  if [ -f "$path" ]; then echo "captured $path"; else echo "capture failed: $path"; cat /tmp/abromux-snapper.log; fi
done
