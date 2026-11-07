#!/usr/bin/env bash
# One look at the GitLab tab: (re)start the debug app, open a merge request,
# print what the tab says it shows, and screenshot it.
#
#   dev/gitlab/look.sh OUT.png [MR-URL] [COMMAND JSON]...
#
# Each COMMAND is a `surface.plugin` call's {"command":…,"args":…}, run in
# order before the screenshot, e.g. '{"command":"file","args":{"path":"src/main.rs"}}'.
# Debug input steps go as {"input":"ui:scroll,900,600,-800"}.
set -euo pipefail
cd "$(dirname "$0")/../.."
out=$1
url=${2:-http://localhost:8929/alice/review-fixtures/-/merge_requests/1}
shift $(( $# >= 2 ? 2 : 1 ))
A=dev/gitlab/app.sh
$A start >/dev/null
surface=$($A rpc surface.create "{\"type\":\"gitlab\",\"url\":\"$url\"}" | jq -r .ref)
for _ in $(seq 1 50); do
  load=$($A rpc surface.plugin "{\"surface\":\"$surface\",\"command\":\"state\"}" | jq -r .review.load)
  [ "$load" = loading ] || break
  sleep 0.2
done
sleep 0.5
for cmd in "$@"; do
  if input=$(jq -er .input <<<"$cmd" 2>/dev/null); then
    $A rpc debug.input "{\"steps\":\"$input\"}" >/dev/null
  else
    $A rpc surface.plugin "$(jq -c --arg s "$surface" '. + {surface: $s}' <<<"$cmd")" >/dev/null
  fi
  sleep 0.4
done
# Avatars and layout settle.
sleep 1
$A rpc surface.plugin "{\"surface\":\"$surface\",\"command\":\"state\"}" \
  | jq -c '{page, section: .location.section, load: .review.load, error: .review.error, changes}'
$A snap "$out"
