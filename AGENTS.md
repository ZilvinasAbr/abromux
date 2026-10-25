# Notes for agents

## Versioning

Every commit has its own version automatically. Don't bump anything by hand per commit.

- `build.rs` builds the version from the `version` in `Cargo.toml` plus the short commit hash: `0.1.0+6dea3c2`. It adds `-dirty` when tracked files have uncommitted changes. A clean checkout of the release tag `v<version>` is just `0.1.0`.
- `abromux version` prints it. `scripts/bundle-app.sh` puts it in the app's `CFBundleShortVersionString`, and the commit count in `CFBundleVersion`.
- Bump `version` in `Cargo.toml` (semver) only for a deliberate release, in its own commit, and only when asked. Tag that commit `v<version>`; `scripts/bundle-app.sh --dist` builds only from such a tag.
- Use `env!("ABROMUX_VERSION")` in code, not `CARGO_PKG_VERSION`.

## Nightly app

To check a change in the real app, build it with `scripts/nightly.sh` (AbroMux Nightly.app), not `scripts/bundle-app.sh`/`update-app.sh`, which replace the user's release AbroMux.app. Details are in the README under "Nightly build".

In code, use `channel::app_name()` for the user-visible app name and `channel::support_dir()` for files under Application Support, never a hardcoded "AbroMux" or `abromux/` path, so the two apps stay apart.

## Performance

Run `scripts/perf.py run` after changes that could affect speed or memory. It records each run with its commit and compares it with earlier runs. `scripts/perf.py run --headless` is the quick check that needs no window.
