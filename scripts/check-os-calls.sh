#!/usr/bin/env bash
# Fails if Rust code outside abromux-platform and the browser backends calls
# the OS directly: `cfg(target_os`, `objc::` or `msg_send!`. Everything else
# reaches the OS through abromux-platform (docs/linux-plan.md).
#
#   scripts/check-os-calls.sh
set -euo pipefail
cd "$(dirname "$0")/.."

if hits=$(git grep --untracked -nE 'cfg\(target_os|objc::|msg_send!' -- '*.rs' \
  ':!crates/abromux-platform/' ':!crates/abromux-browser/src/backend/'); then
  echo "OS calls outside crates/abromux-platform and crates/abromux-browser/src/backend:" >&2
  echo "$hits" >&2
  exit 1
fi
echo "OK: no OS calls outside abromux-platform and the browser backends"
