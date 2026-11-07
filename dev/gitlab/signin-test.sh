#!/usr/bin/env bash
# A rejected token, then signing in again from the tab: it ends up on the
# merge request it was opened on. (No Keychain writes: see app.sh.)
set -euo pipefail
cd "$(dirname "$0")/../.."
A=dev/gitlab/app.sh
state() { $A rpc surface.plugin '{"surface":"surface:2","command":"state"}' | jq -c "$1"; }
ABROMUX_GITLAB_TOKEN=glpat-wrong $A start >/dev/null
$A rpc surface.create '{"type":"gitlab","url":"http://localhost:8929/alice/review-fixtures/-/merge_requests/1"}' >/dev/null
sleep 2
echo "rejected:"; state '{page, load: .review.load, error: .review.error}'
$A rpc surface.plugin '{"surface":"surface:2","command":"sign_in","args":{"token":"glpat-abromux-dev-bob-token"}}' >/dev/null
sleep 3
echo "signed in:"; state '{page, me, load: .review.load, title: .review.summary.title}'
$A stop >/dev/null
