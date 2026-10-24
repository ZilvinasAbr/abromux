#!/usr/bin/env bash
# Run abromux with a ABROMUX_SCRIPT, capturing `snap:` steps with screencapture
# (it runs under the calling terminal's Screen Recording permission).
# usage: ABROMUX_SCRIPT="type:ls;key:enter;wait:500;snap:/tmp/a.png;wait:1000;quit" scripts/drive.sh
# (abromux doesn't wait for the capture, so wait after a snap before quitting.)
cd "$(dirname "$0")/.."
./target/debug/abromux | while read -r tag window path; do
  if [ "$tag" != SNAP ]; then echo "$tag $window $path"; continue; fi
  rm -f "$path"
  if screencapture -x -o -l "$window" "$path" && [ -f "$path" ]; then echo "captured $path"; else echo "capture failed: $path"; fi
done
