#!/usr/bin/env bash
# Frame times while scrolling the GitLab tab that dev/gitlab/look.sh opened:
# N scroll steps of DY pixels at X,Y, recorded with debug.frames.
#   dev/gitlab/scroll-perf.sh [N] [DY] [X] [Y]
set -euo pipefail
cd "$(dirname "$0")/../.."
n=${1:-30} dy=${2:--400} x=${3:-1000} y=${4:-600}
A=dev/gitlab/app.sh
$A rpc debug.frames '{"record":true}' >/dev/null
for _ in $(seq 1 "$n"); do
  $A rpc debug.input "{\"steps\":\"ui:scroll,$x,$y,$dy\"}" >/dev/null
  sleep 0.05
done
sleep 0.3
$A rpc debug.frames '{"record":false}'
