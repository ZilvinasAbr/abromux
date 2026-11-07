#!/usr/bin/env bash
# A GitLab tab comes back after quitting: same merge request, section, view
# and Viewed marks. Prints what was saved and what came back.
set -euo pipefail
cd "$(dirname "$0")/../.."
export ABROMUX_SESSION_PATH=$PWD/target/shots/session.json
rm -f "$ABROMUX_SESSION_PATH"
A=dev/gitlab/app.sh
plugin() { $A rpc surface.plugin "{\"surface\":\"surface:2\",\"command\":\"$1\",\"args\":$2}" >/dev/null; }

$A start >/dev/null
$A rpc surface.create '{"type":"gitlab","url":"http://localhost:8929/alice/review-fixtures/-/merge_requests/1/diffs"}' >/dev/null
sleep 2
plugin viewed '{"path":"README.md"}'
plugin parallel '{"on":true}'
$A stop >/dev/null
sleep 1
echo "saved:"
jq -c '.. | objects | select(.kind? == "plugin")' "$ABROMUX_SESSION_PATH"

$A start >/dev/null
sleep 3
echo "restored:"
$A rpc surface.list | jq -c '[.. | objects | select(.type? == "gitlab")
  | {ref, title, section: .state.location.section, parallel: .state.location.parallel,
     viewed: .state.location.viewed, load: .state.review.load, folded: .state.changes.folded}]'
$A stop >/dev/null
