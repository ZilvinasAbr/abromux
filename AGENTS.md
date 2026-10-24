# Notes for agents

## Versioning

Every commit has its own version automatically. Don't bump anything by hand per commit.

- `build.rs` builds the version from the `version` in `Cargo.toml` plus the short commit hash: `0.1.0+6dea3c2`. It adds `-dirty` when tracked files have uncommitted changes.
- `abromux version` prints it. `scripts/bundle-app.sh` puts it in the app's `CFBundleShortVersionString`, and the commit count in `CFBundleVersion`.
- Bump `version` in `Cargo.toml` (semver) only for a deliberate release, in its own commit, and only when asked.
- Use `env!("ABROMUX_VERSION")` in code, not `CARGO_PKG_VERSION`.

## Performance

Run `scripts/perf.py run` after changes that could affect speed or memory. It records each run with its commit and compares it with earlier runs. `scripts/perf.py run --headless` is the quick check that needs no window.
