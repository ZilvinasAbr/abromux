#!/usr/bin/env bash
# How long the GitLab tab takes to load, against a GitLab far away
# (dev/gitlab/slow-proxy.py on :8930): the merge request list, then a merge
# request's review. Each timed from the command to "ready", over the socket.
#
#   dev/gitlab/load-time.sh [HOST] [MR-IID...]
set -euo pipefail
cd "$(dirname "$0")/../.."
host=${1:-http://localhost:8930}
shift || true
iids=("${@:-7 1 9}")
A=dev/gitlab/app.sh
$A start >/dev/null
now() { python3 -c 'import time; print(time.time())'; }
wait_for() { # surface, jq path, wanted value
  for _ in $(seq 1 1200); do
    v=$($A rpc surface.plugin "{\"surface\":\"$1\",\"command\":\"state\"}" | jq -r "$2")
    [ "$v" = "$3" ] && return 0
    [ "$v" = failed ] && { echo "failed: $($A rpc surface.plugin "{\"surface\":\"$1\",\"command\":\"state\"}" | jq -c '[.list.error, .review.error]')"; return 1; }
    sleep 0.05
  done
  echo "timed out"; return 1
}
t0=$(now)
s=$($A rpc surface.create "{\"type\":\"gitlab\",\"host\":\"$host\"}" | jq -r .ref)
wait_for "$s" .list.load ready
t1=$(now)
printf 'list            %6.2fs\n' "$(echo "$t1 - $t0" | bc)"
for iid in ${iids[@]}; do
  path=alice/review-fixtures
  t0=$(now)
  $A rpc surface.plugin "{\"surface\":\"$s\",\"command\":\"open\",\"args\":{\"url\":\"$host/$path/-/merge_requests/$iid\"}}" >/dev/null
  wait_for "$s" .review.load ready
  t1=$(now)
  printf 'review !%-6s  %6.2fs\n' "$iid" "$(echo "$t1 - $t0" | bc)"
  $A rpc surface.plugin "{\"surface\":\"$s\",\"command\":\"back\"}" >/dev/null
done
