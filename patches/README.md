# Patched dependencies

Crates fixed locally and wired in with `[patch.crates-io]` in `Cargo.toml`.
Drop one when upstream fixes it.

- `xattr` 0.2.3 (gpui → gpui_http_client → zed-async-tar): libc 0.2.191
  dropped Linux's `ENOATTR` alias, so it maps to `ENODATA` there
  (`src/sys/mod.rs`). Only the Linux build needs it.
