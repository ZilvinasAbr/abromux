---
name: manual-test
description: Build the current working tree into AbroMux Nightly.app and open it so the user can try a change by hand. Use when the user says /manual-test, "build and open it", "let me try it", or wants to test a fix in the real app.
---

# Manual test in AbroMux Nightly

Builds the working tree as-is (uncommitted changes included) into
`/Applications/AbroMux Nightly.app` and opens it. Never use
`scripts/bundle-app.sh` or `scripts/update-app.sh` for this: they replace the
user's release AbroMux.app.

Run everything from the current checkout (in a worktree, the worktree root).

1. **libghostty-vt.** The build needs `vendor/ghostty-vt-out`. A fresh worktree
   doesn't have it, so if it's missing, link the main checkout's copy instead of
   rebuilding it:

   ```bash
   main="$(git rev-parse --path-format=absolute --git-common-dir)/.."
   [ -e vendor/ghostty-vt-out ] || { mkdir -p vendor && ln -s "$(cd "$main" && pwd)/vendor/ghostty-vt-out" vendor/ghostty-vt-out; }
   ```

   If the main checkout doesn't have it either, run `./scripts/build-libghostty.sh` once.
   The path is gitignored, so the link never gets committed.

2. **Quit a running Nightly.** A running copy keeps the old build. Quit it
   gracefully (sessions are restored on the next launch), never `kill` it:

   ```bash
   pgrep -qf "AbroMux Nightly.app/Contents/MacOS/abromux" && osascript -e 'quit app "AbroMux Nightly"'
   ```

   Then wait until `pgrep` no longer finds it. If it is still running after
   about 10 s (it may be asking to confirm the quit), tell the user to confirm
   the quit and stop.

3. **Build.** Run it in the background with a long timeout (a clean build takes minutes):

   ```bash
   ./scripts/nightly.sh
   ```

   It ends with `Installed /Applications/AbroMux Nightly.app (<version>)`. A
   version ending in `-dirty` means uncommitted changes are included. If the build
   fails, show the error and stop.

4. **Open it** and check that it's the new process:

   ```bash
   open "/Applications/AbroMux Nightly.app"
   ps -o lstart= -p "$(pgrep -f 'AbroMux Nightly.app/Contents/MacOS/abromux' | head -1)"
   ```

   The start time should be from just now.

5. **Hand over.** Tell the user which version was installed and, in a few
   short steps, what to try for the change being tested and what they should see.
   The `abromux-nightly` CLI talks to this app.

To test several worktrees side by side, pass `--name <flavor>` to
`scripts/nightly.sh` (it builds "AbroMux Nightly <flavor>.app"), and use that app
name in steps 2 and 4.
