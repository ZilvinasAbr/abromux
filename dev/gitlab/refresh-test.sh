#!/usr/bin/env bash
# Refreshing keeps the reader's place: open !1's changes at limiter.rs, have
# alice reply on a thread meanwhile, refresh, and check the reply is in with
# the view where it was. (Adds a note to !1; rerun seed.py to reset.)
set -euo pipefail
cd "$(dirname "$0")/../.."
A=dev/gitlab/app.sh
API=http://localhost:8929/api/v4/projects/alice%2Freview-fixtures/merge_requests/1
ROOT="PRIVATE-TOKEN: glpat-abromux-dev-root-token"
state() { $A rpc surface.plugin '{"surface":"surface:2","command":"state"}' | jq -c "$1"; }
notes() { state '[.review.summary.files[].threads[].notes] | add'; }

$A start >/dev/null
$A rpc surface.create '{"type":"gitlab","url":"http://localhost:8929/alice/review-fixtures/-/merge_requests/1/diffs"}' >/dev/null
sleep 2
$A rpc surface.plugin '{"surface":"surface:2","command":"file","args":{"path":"src/limiter.rs"}}' >/dev/null
sleep 0.5
echo "before: notes on lines $(notes), top row $(state .changes.top_row)"

thread=$(curl -fsS -H "$ROOT" "$API/discussions" | jq -r '[.[] | select(.notes[0].position.new_line == 17)][0].id')
curl -fsS -H "$ROOT" -H "Sudo: alice" -X POST "$API/discussions/$thread/notes" \
  --data-urlencode "body=Done, thanks." >/dev/null

$A rpc surface.plugin '{"surface":"surface:2","command":"refresh"}' >/dev/null
sleep 2
echo "after:  notes on lines $(notes), top row $(state .changes.top_row), load $(state .review.load)"
$A stop >/dev/null
