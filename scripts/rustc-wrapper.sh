#!/bin/sh
# Cargo's rustc-wrapper (.cargo/config.toml): caches compiler output with kache
# (https://github.com/kunobi-ninja/kache) when it's installed, so a new
# worktree reuses dependencies already built in another one. Without kache it
# runs rustc as is. Cargo calls it as `rustc-wrapper.sh <rustc> <args>...`.
if command -v kache >/dev/null 2>&1; then
  exec kache "$@"
fi
exec "$@"
