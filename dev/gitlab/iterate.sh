#!/usr/bin/env bash
# The plugin's edit loop in one go: stop the debug app, run the plugin's
# tests, rebuild (dev/gitlab/build.sh), and take a look (dev/gitlab/look.sh).
#
#   dev/gitlab/iterate.sh NAME [MR-URL] [COMMAND JSON]...   → target/shots/NAME.png
#   dev/gitlab/iterate.sh --no-build NAME …                  just the look
set -euo pipefail
cd "$(dirname "$0")/../.."
build=1
[ "${1:-}" = --no-build ] && { build=0; shift; }
name=$1; shift
mkdir -p target/shots
if [ $build = 1 ]; then
  dev/gitlab/app.sh stop >/dev/null
  cargo test -q -p abromux-gitlab 2>&1 | grep -E "test result|FAILED|panicked|^error" | grep -v "ok\. 0 passed" || true
  # Errors only: the patched gpui has warnings of its own.
  dev/gitlab/build.sh 2>&1 | grep -E "^error" -A12 || true
fi
dev/gitlab/look.sh "target/shots/$name.png" "$@"
