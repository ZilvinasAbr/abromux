#!/usr/bin/env bash
# Drive a debug abromux for checking the GitLab plugin, apart from any
# running AbroMux: its own control socket, holder socket and no session.
#
#   dev/gitlab/app.sh start             # launch it (dev/gitlab/build.sh first)
#   dev/gitlab/app.sh rpc METHOD [JSON] # e.g. rpc surface.create '{"type":"gitlab"}'
#   dev/gitlab/app.sh snap OUT.png      # screenshot of its window
#   dev/gitlab/app.sh stop
#
# It's built on a gpui that renders its own screenshots (gpui-capture.py), so
# they work with the Mac locked or the window hidden.
set -euo pipefail
cd "$(dirname "$0")/../.."
STATE="${TMPDIR:-/tmp}/abromux-gitlab-dev"
mkdir -p "$STATE"
export ABROMUX_SOCKET_PATH="$STATE/abromux.sock"
export ABROMUX_HOLD_SOCKET="$STATE/hold.sock"
# No session, unless ABROMUX_SESSION_PATH names one (to test restoring tabs).
[ -n "${ABROMUX_SESSION_PATH:-}" ] || export ABROMUX_NO_RESTORE=1
# The seeded reviewer's token (dev/gitlab/up.sh). Set it empty to see the
# tab's sign-in form.
export ABROMUX_GITLAB_TOKEN="${ABROMUX_GITLAB_TOKEN-glpat-abromux-dev-bob-token}"
# Signing in here doesn't write test tokens to your Keychain.
export ABROMUX_GITLAB_NO_KEYCHAIN=1
unset ABROMUX_SURFACE_ID ABROMUX_WORKSPACE_ID __CFBundleIdentifier
export GPUI_CAPTURE_DIR="$STATE/capture"
mkdir -p "$GPUI_CAPTURE_DIR"
# ABROMUX_DEV_PROFILE=release runs `dev/gitlab/build.sh --release`'s build (for timing).
BIN=./target/capture/${ABROMUX_DEV_PROFILE:-debug}/abromux

case "${1:-}" in
  start)
    "$0" stop >/dev/null 2>&1 || true
    rm -f "$ABROMUX_SOCKET_PATH"
    # stdout carries `SNAP <window> <path>` lines (src/debug.rs); capture them.
    nohup bash -c '
      "$0" | while read -r tag window path; do
        if [ "$tag" != SNAP ]; then echo "$tag $window $path"; continue; fi
        rm -f "$path"
        screencapture -x -o -l "$window" "$path" && echo "captured $path"
      done' "$BIN" >"$STATE/app.log" 2>&1 &
    echo $! >"$STATE/pid"
    for _ in $(seq 1 100); do
      [ -S "$ABROMUX_SOCKET_PATH" ] && "$BIN" rpc system.ping >/dev/null 2>&1 && { echo started; exit; }
      sleep 0.2
    done
    echo "didn't start; log:"; cat "$STATE/app.log"; exit 1
    ;;
  rpc) shift; exec "$BIN" rpc "$@" ;;
  snap)
    out="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
    rm -f "$out"
    # The next frame is written to $out; make sure there is one.
    echo "$out" >"$GPUI_CAPTURE_DIR/request"
    "$BIN" rpc debug.frame '{"refresh":true}' >/dev/null 2>&1 || true
    for _ in $(seq 1 50); do [ -s "$out" ] && { echo "$out"; exit; }; sleep 0.1; done
    echo "no snapshot; log:"; tail -5 "$STATE/app.log"; exit 1
    ;;
  stop)
    "$BIN" rpc debug.input '{"steps":"quit"}' >/dev/null 2>&1 || true
    sleep 0.5
    [ -f "$STATE/pid" ] && pkill -P "$(cat "$STATE/pid")" 2>/dev/null || true
    rm -f "$STATE/pid"
    echo stopped
    ;;
  *) sed -n 2,8p "$0"; exit 2 ;;
esac
