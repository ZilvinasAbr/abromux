# AbroMux guide

Everything AbroMux does, in detail. For a quick start, see the [README](../README.md).

## Features

- **Terminal panes** use [libghostty-vt](https://github.com/ghostty-org/ghostty) for all terminal state: VT parsing, screen and scrollback, colors, selection and key encoding. It is linked statically through a bindgen FFI layer. A shell runs in a PTY (`portable-pty`) and GPUI draws each cell. Text goes through the macOS input system, so IMEs and dead keys compose as usual. Mouse clicks, drags and the wheel go to TUI apps that ask for them (vim, htop, lazygit); hold Shift to select or scroll locally. Full-screen apps without mouse support get arrow keys from the wheel, like Ghostty's alternate scroll.
- **Browser panes** are native WKWebViews (wry, via `gpui-component`). Each has an address bar with back, forward and reload. Typing in the address bar suggests pages from your browsing history, ranked as in cmux (where the text matches, plus how recently, often and directly you've visited them). ↑/↓ pick one, Return opens it, Esc closes the list, ⇧⌫ deletes the highlighted entry, and ↓ on the page's own URL lists your most typed and recent ones. History is shared by all browser panes and kept in `browser-history.json` under Application Support. ⌘-click or middle-click on a link opens it in a new tab after the current one (⇧ to switch to it). Passkeys and security keys work in them once the app is signed with Apple's web-browser entitlement (see "Passkeys"). Hovering a link shows its URL at the page's bottom-left corner, as in Safari.
- **Downloads** from browser panes go to `~/Downloads` (a file already there gets ` (1)` and so on), including links the server marks `Content-Disposition: attachment`. Once there's one, a download button joins the toolbar: a bar under it shows progress while one runs, and it hops when one starts. Click it for the session's list: click a finished one to open it, the magnifier shows it in Finder, `×` cancels a running one, and Clear empties the list (the files stay). `abromux browser downloads` and `wait-download` do the same from the CLI.
- **Popovers over pages**: the command palette, ⌘I, the update popover, the sidebar's right-click menus, address-bar suggestions, the downloads list and dragging a tab all draw over a browser pane. Native webviews draw above everything the app draws, so while one of these is open the page is swapped for a bitmap of itself, taken just before; it comes back live when the popover closes.
- **Markdown tabs** show a local Markdown file rendered, GitHub style (tables, task lists, footnotes, HTML in the file, `#heading` links), and follow the file: an edit shows within half a second, at the same scroll position. Open one with `abromux markdown <file>` (or `abromux open <file>.md`), or ⌘-click a `.md` path in a terminal. Relative links and images in the file work, so do back/forward, ⌘F, zoom and the `abromux browser` commands; web links open in a browser tab beside it. They're browser tabs underneath, served from an `abromux-md://` address that only Markdown tabs load, and they show local files only. Scripts in a file don't run.
- **Files from Finder** open in a terminal editor, Neovim or tode (`editor.program`), in a new pane beside the focused one: pick AbroMux in a file's Open With menu, or make it the default app for a file type (see [Opening files](#opening-files)).
- **Workspaces** sit in a left sidebar, cmux style. Each entry shows the name, the focused tab's title, the git branch and the cwd, plus whatever scripts report through the CLI: status pills (`set-status`), a progress bar (`set-progress`) and the newest `log` line.
- **Coding agents** (Claude Code, Codex, Gemini CLI, Droid, Cursor CLI, OpenCode and Pi) show up in the sidebar once you've run `abromux hooks install`: a "Running" pill while the agent works and "Needs input" when it asks for permission (Cursor's hooks can't say when it asks). When it asks, or finishes a turn, its terminal gets a notification ("Codex: Asks to run …", "Claude Code: <the start of its reply>"). Their tabs show the agent's logo (Claude Code and Codex) or a robot.
- **Notifications** come from terminal programs (OSC 9, OSC 99 and OSC 777, as Claude Code, Codex and others send them), from the bell, and from `abromux notify`. One you aren't looking at rings its pane, puts a dot on its tab and on its workspace, shows its text under the workspace in the sidebar, counts on the dock icon, and, while AbroMux is in the background, shows a macOS banner that takes you to it when clicked (banners need AbroMux.app; a `cargo run` build has the rest). It's read once you look at that terminal. ⌘I lists them all; ⌘⇧U jumps to the newest unread one. ⌥⌘U marks the terminal in front of you unread (or read), and ⌃⌘U sends its notification to the back of the queue and jumps to the next one, to work through a pile one at a time. A bell in the macOS menu bar shows the unread count and lists them. [Notification hooks](#notifications) can filter or change each one.
- **Links** in terminal output underline while you hold ⌘ over them: URLs, OSC 8 hyperlinks, and paths to files that exist (relative ones from the shell's directory, `path:line` too). ⌘-click opens web links in a browser to the right of the terminal (a new tab there if that pane already shows one), Markdown files the same way as Markdown tabs, and anything else with its default app.
- **A merge requests panel** on the right (⌥⌘B) lists the workspace's, yours and the ones you opened lately; they're kept cached and fresh in the background, so any of them opens at once in a GitLab tab (see "GitLab merge requests").
- **Panes** split right or down, and each pane has a tab bar of terminal and browser tabs. Drag a divider to resize. Drag a tab to reorder it, onto another pane's tab bar or middle to move it there, onto a pane's edge to split that pane, or onto the outer edge of the pane area to split the whole workspace (full width or height), or onto a workspace in the sidebar to move it into that workspace. A highlight shows where it will land. Move Tab to New Workspace (⌘⇧P) gives the focused tab a workspace of its own.

- **GitLab merge requests** open in a tab that looks like GitLab's own review pages (see "GitLab reviews"): the list, the overview with its threads, commits, and the changes with a file tree, inline or side-by-side diffs, suggestions and image diffs. You can comment, reply, resolve threads, start threads on lines, approve, edit and delete your comments, edit, close or merge the merge request, and retry pipelines.

## Build from source

Requirements:

- macOS on Apple Silicon (the libghostty build script fetches an aarch64 Zig)
- Rust stable (pinned via `rust-toolchain.toml`)
- Xcode Command Line Tools (clang for bindgen, `codesign`)

Build and run:

```bash
./scripts/build-libghostty.sh   # once: local Zig 0.17 + libghostty-vt into vendor/
cargo run --release
```

`ABROMUX_DEMO=1 cargo run` opens with a sample layout.

To install it as a Mac app:

```bash
scripts/bundle-app.sh                                          # builds /Applications/AbroMux.app
ln -sf /Applications/AbroMux.app/Contents/MacOS/abromux ~/.cargo/bin/abromux   # the CLI on PATH
```

Run from the bundle, a bare `abromux` brings the running app to the front, or opens AbroMux.app if it isn't running. A dev build (`cargo run`) always starts its own app.

### Nightly build

To try the working tree as an app without replacing the release AbroMux.app:

```bash
scripts/nightly.sh                 # builds /Applications/AbroMux Nightly.app (or pass another folder)
abromux-nightly list-workspaces    # its CLI, symlinked into ~/.cargo/bin
scripts/nightly.sh --name zoom     # "AbroMux Nightly zoom.app" and `abromux-nightly-zoom`
```

A named nightly (bundle id `dev.abromux.nightly.<name>`, support folder `abromux-nightly-<name>/`) runs beside the release and the shared nightly, so agents testing different branches don't replace each other's build.

A nightly others can download: commit, then `scripts/bundle-app.sh --nightly --dist` builds a notarized `dist/AbroMux-Nightly-<version>.dmg`, published as a GitHub pre-release tagged `nightly-<commit>`. Pre-releases are never GitHub's "latest" release, so the release app's update check doesn't offer them.

AbroMux Nightly runs next to AbroMux. It has a neon icon (`assets/app/abromux-nightly.svg`), its own name, the bundle id `dev.abromux.nightly` (so separate notification permissions and settings), and its own socket and session in `~/Library/Application Support/abromux-nightly/`. It's signed like the release but never notarized: `scripts/bundle-app.sh --nightly` is what `nightly.sh` calls, and it refuses `--dist`. The binary decides which one it is at runtime from the bundle it sits in (`src/channel.rs`); `cargo run` counts as the release. Install agent hooks from the release `abromux`, not `abromux-nightly`: hooks point at the binary you install them from, and do nothing once that app is gone.

## Updates

AbroMux checks for updates like cmux does (cmux uses Sparkle; AbroMux does the same steps itself). A few seconds after launch, and once a day after that, it looks at the latest GitHub release. When there's a newer version, a pill appears at the foot of the sidebar (or in the titlebar when the sidebar is hidden): **Update Available: 0.13.0**. Click it for the version, size, release date and a link to the release notes, and:

- **Install and Relaunch** downloads the release's DMG (the pill shows the percentage), checks that the app in it is signed by the same Developer ID team as the one you're running, swaps it in for the current app and relaunches. Shells keep running across the relaunch, as they do across any quit.
- **Later** hides the pill until the next check.
- **Skip** stops offering that version. A check you start yourself still shows it.

Nothing downloads until you choose Install and Relaunch. **Check for Updates…** (in the AbroMux menu, the command palette, or Settings › App › Updates) checks right away and also tells you when you're already on the latest version. The palette's **Apply Update (If Available)** installs a found update, or checks if there isn't one yet. Turn the daily check off with `"app": { "automaticallyCheckForUpdates": false }` or the switch in Settings.

The check time and a skipped version are kept in `update.json` in the support folder, and each step is written to `update.log` there. An app running from the disk image or from a folder you can't write to says so and won't install; move it into Applications first. Nightly builds only check: they come from `scripts/nightly.sh`, not from releases. To try the flow from a nightly, launch it with `ABROMUX_UPDATE_FROM=0.1.0` so it thinks it's on an old version (`ABROMUX_UPDATE_FEED` points the check at another releases API URL). `cargo test update::tests::live -- --ignored` downloads, verifies and swaps the real latest release in a scratch folder.

## GitLab reviews

A GitLab tab (⌘⇧P → New GitLab Tab) shows merge requests the way GitLab's web UI does, in its dark theme (whatever the app's appearance). It's built in as a plugin (`crates/abromux-gitlab`, the `gitlab` Cargo feature, on by default) and writes what a reviewer and an author do: comments, replies, resolving, threads on lines, approvals, editing and deleting your comments, editing the title and description, draft, close and reopen, merging, and running, retrying or cancelling pipelines.

- **Sign in** with the GitLab URL and a personal access token with the `api` scope (a `read_api` token works too, but then writing says the token needs `api`). The token is checked, then kept in your login Keychain (service `abromux-gitlab`, account = the URL). New GitLab tabs then open on that GitLab, so you sign in once. `ABROMUX_GITLAB_TOKEN` overrides it.
- **The list** has the merge requests you're asked to review, the ones you created, and all you can see, open, merged, closed or all of them, with checklist progress, coloured labels, the target branch when it isn't the default, pipeline status, assignees and reviewers, resolved threads ("1 of 4") and Approved, Merged and Closed badges, newest first, and a filter.
- **A merge request** has GitLab's header and tabs. On a stack (open merge requests whose target branch is another's source branch), the header has GitLab's "Stacked merge requests" button, "2 of 3": it lists the stack, top first, with each one's files and lines changed, and opens the one you pick. Overview: the description, approvals, merge status, the activity (GitLab's own notes, comments, threads on the diff with the code around their line; resolved ones folded) and the sidebar (with each reviewer's review state, the issues it closes, time tracking and participants). `#12`, `!3` and `@name` in descriptions and comments link to GitLab, as there, and Jira tickets (`PAY-1423`) in the title, descriptions and comments link to Jira when abromux knows which one (`"jira": { "host": … }`, see "Tracked links"). Links in descriptions and comments open in a browser tab beside it, not the system browser. Descriptions and comments are drawn as GitLab's markdown: template comments (`<!-- -->`) and `[[_TOC_]]` hidden, `<details>` shown under its summary, `>>>` quotes, pictures uploaded to GitLab as links to them, `/group/project` links resolved. Commits, by day, each with its pipeline's status. Pipelines: the merge request's pipelines, newest first. Changes: the file tree (with search: `limit`, `*.rs`, `src/*`), each file's diff with syntax highlighting and GitLab's word-level changes, threads under their lines, suggestions as "Suggested change", images old and new side by side, and "Viewed" marks that fold files.
- **Changes tools**: Inline or Side-by-side; the arrows in a hunk header or at a file's end show unchanged lines (20 at a time, or all); "Compare main and latest version" picks other versions of the merge request to compare (an "added N commits" note's "Compare with previous version" does it too); a commit's title in Commits (or its sha in the activity) shows just that commit's changes, under its message, with Previous and Next; the file's header stays at the top while you read it. Large or generated files fold, with "View file" to see them in GitLab.
- **Overview tools**: the pipeline of the latest push (and, once merged, the merge commit's), and the activity filtered to comments or history, oldest or newest first. The merge box says how far behind the target the source branch is, with "Rebase source branch", and for a blocked merge which check failed ("Mark as ready" for a draft). Code blocks in descriptions and comments are highlighted as in GitLab, with a copy button. Scrolled down, the header folds into one line.
- **More Changes tools**: the file browser as a tree with folding folders or as a flat list, as wide as you drag its right edge (a double-click there puts it back to 320px; new tabs open at the last width); buttons to expand or collapse every file; "one file at a time", with Previous and Next file (`j`/`k` and `n`/`p` move between files in it); a click on a line number marks the line, as GitLab's line links do.
- **Reviewing**: the comment box at the end of the Overview; under each thread a Reply box (Reply, Reply and resolve) and Resolve thread or Reopen thread; on a diff line, the speech bubble that shows on hover starts a thread there, and "Start another thread" under a thread starts one more on its line (on the latest version, inline or side by side); Approve or Revoke approval in the approvals box; 👍, 👎 and "Add reaction" under the description react, the smiley at the top right of each comment reacts to that comment (its reactions show under its text), and a click on your own takes it back. Each write shows at once, before GitLab answers: an approval, a resolved thread, a reaction, a picked reviewer or label, and a new comment or reply (faded until GitLab has it, with nothing to do to it until then). Writes go to GitLab one after another, in the order made, and when the last one lands the page refreshes in place. One that fails is taken back, an error shows above the page, and what you typed goes back in its box. While you type in a box, the keys below don't fire, and Esc closes the box.
- **Zoom**: ⌘= and ⌘- make the tab bigger or smaller (50% to 300%), ⌘0 puts it back; View › Zoom In / Zoom Out / Actual Size do the same. It's one zoom for every GitLab tab, kept across restarts.
- **Mentions**: typing `@` and part of a name or username in a comment lists who it could be (people on the merge request first, then the project's members; a big project is searched on GitLab as you type). ↑ ↓ pick, Enter or Tab puts the username in, Esc closes the list.
- **Highlighting** follows GitLab's (Rouge's) tokens per language, among them YAML (keys, values, anchors, tags), PHP (variables, classes, attributes), JavaScript and TypeScript (types, decorators, template strings, regexes, JSX), CSS, SCSS and Less (selectors, properties, values), JSON, and HTML, Vue and Svelte (tags, attributes, and their `<script>` and `<style>`).
- **Keys**, as in GitLab: `j`/`]` and `k`/`[` for the next and previous file, `n` and `p` for the next and previous unresolved thread (also the arrows next to the thread count), `v` to mark the file you're reading Viewed and go on to the next one not viewed yet (the file you're on is outlined, its Viewed ringed; unmark with the checkbox), and `/` or `s` to search.
- **Managing the merge request**, as on GitLab's page: Edit right of the title opens its title and description in fields at the top of the Overview (Save changes, or ⌘Enter); the ⋮ beside it has Mark as draft or Mark as ready, Close merge request or Reopen merge request, and Copy reference. On your own comments a pencil edits the comment in place and a bin deletes it, after asking. ⌘Enter sends any box (comment, reply, line thread, edit).
- **Merging**: when GitLab says you may merge (`user.can_merge`), the merge box has Merge with "Delete source branch" and "Squash commits" ticked as the author asked (the facts under it follow the ticks). While the pipeline is still going it's "Set to auto-merge" instead, and once set the box says who set it, with Cancel auto-merge. Merging says which head it means, so a push in between makes GitLab refuse rather than merge something unseen.
- **Pipelines**: Retry on a failed or canceled one, Cancel on one still going, and Run pipeline above them.
- **The sidebar** edits as GitLab's does: Edit on Assignee, Reviewer and Labels opens a dropdown with a search, ticking people or labels, saved when it closes (a click outside, Edit again, or Esc); on GitLab's free tier, which takes one assignee and one reviewer, a pick replaces and saves at once. Milestone picks one of the project's open milestones. "assign yourself" when nobody is, and a label's × removes it. Label and milestone changes show in the activity as GitLab words them.
- **Big merge requests** show every file. GitLab's diff stops at 1000 files and leaves out files over 200 KB; the rest are listed from GitLab's stats with their line counts, and each one's old and new text is fetched and diffed here when it scrolls into view (a few at a time), with a Retry if that fails. Lines longer than 5000 characters (minified files) are cut short, saying by how much. The file tree only draws the rows in view, so a thousand files scroll as smoothly as ten.
- **Selecting text**: drag over a description or comment, or over code (diff lines, the overview's snippets, code blocks, suggestions), and ⌘C copies it. In the diff a selection keeps to its side, side by side, and one longer than the screen copies whole. Shift-click extends it; a click elsewhere drops it.
- **It stays current**: the merge requests panel's cache checks the one on screen every 20 seconds and loads it again in place when it changed (see "GitLab merge requests"), and the tab itself fetches it again every two minutes (and on Refresh) unless that just happened, without losing your place, so new comments and pushes show up as on GitLab's page.
- **The list** has GitLab's state tabs with their counts and its sort ("Created date", "Updated date", "Title", "Popularity", either way round).
- **Projects**: the Projects tab lists the projects you're a member of, most recently active first, with each one's open merge requests; typing searches GitLab's projects. Pick one for its merge requests (with the state tabs, filter and sort), and "All projects" to go back.
- **Search** (`/` or `s` anywhere in the tab, as on GitLab's pages): a search over the page finds merge requests on GitLab as you type, across every project you can see; `!12` finds those numbered 12 in the projects at hand, `group/project!12` that one, and a pasted link opens. With nothing typed it shows the list's merge requests. ↑ ↓ pick, Enter opens, Esc closes.
- **Opening links**: ⌘⇧P → **Open GitLab Merge Request…** asks for a link (one already copied is filled in) and opens it in a new tab. a merge request's link pasted into the list's search box (or ⌘V anywhere on the tab) opens it; a project's merge requests link (`…/group/project/-/merge_requests`) lists them. ⌘-clicking either kind of link in a terminal, or opening one from a browser tab, opens it in a GitLab tab where the browser would have gone (in a GitLab pane to the right, if there is one).
- **Speed**: what a page needs is fetched together, through one curl that runs the requests side by side over connections it keeps open (one multiplexed connection on HTTP/2), so a GitLab far away costs a round trip or two instead of one per request. The list shows as soon as the merge requests come, and approvals, threads, pipelines and branches fill in after. A merge request loaded before (in this tab or another, by the panel's background cache, or before a restart: what GitLab answered is kept on disk) shows its last copy at once while the fresh one loads, and so does a list. Members load a hundred at a time; the people pickers ask GitLab for the rest as you type.
- **Opening one directly**: `abromux rpc surface.create '{"type":"gitlab","url":"https://gitlab.example.com/group/project/-/merge_requests/12"}'`. A tab comes back after a restart on the same page, with its Viewed marks.

In a narrow pane the file tree and sidebar step aside; the button left of "Compare" brings the tree back. Developing the plugin against a GitLab in Docker is described in [`dev/gitlab/README.md`](../dev/gitlab/README.md).

### Local diffs

`abromux diff [<range>]` shows any git diff in the GitLab tab, as if it were a merge request, with no GitLab: like `hunk`, but in the review UI. `X...Y` diffs Y against where it forked from X, `X..Y` the two commits, `X` X against the working tree, and nothing the uncommitted changes (untracked files too). `--merge-base X` (as `git diff` has it) diffs the working tree against where it forked from X, so a branch's commits and what isn't committed yet, and `--since-fork` does that for the default branch (`origin/HEAD`, else `main` or `master`). ⌥⌘D (File › Review Changes, or the palette) opens that last one for the focused tab's repository without typing anything, or for each checkout of a workspace folder when the focused tab isn't in one; on the default branch itself it's the uncommitted changes. It opens beside the terminal it's run in (`--direction`, or `--pane` for a tab there); `--cwd` picks the repository and `--title` names it. The tab is answered from git and a small file of comment threads in `local-diffs/` under Application Support (`crates/abromux-gitlab/src/local.rs`), so Changes, Commits, expanding unchanged lines, threads on lines, replies, resolving and "Start a review" all work as for a merge request. The same diff asked for again reopens the same review and tab, and the tab reloads by itself when the threads, the revisions or the working tree change.

Comments go to a coding agent instead of GitLab. A new thread (or a submitted review's threads, together, or a reply) writes a prompt with the repository, the diff, the code around the line and the thread, and starts the agent in a terminal beside the tab: `--agent '<command>'` (default `claude`, or `$ABROMUX_DIFF_AGENT`; the prompt is appended, or put where `{prompt}` or `{prompt_file}` is). With `--send-here` (or `--send-to surface:N`), comments go to that terminal's agent as one line pointing at the prompt instead. The agent makes the change or answers, then replies in the thread with `abromux diff reply <id> <thread> [--resolve]`, and the reply shows under the line. `abromux diff list` lists the reviews and `abromux diff threads <id>` prints one's threads.

## Passkeys

Browser panes sign in with passkeys (iCloud Keychain, a phone over Bluetooth) and security keys, the way cmux does. WKWebView only allows WebAuthn for an app's own associated domains, so `src/webauthn.js` replaces `navigator.credentials.create/get` for public-key requests in the top frame and hands them to `src/webauthn.m`, which runs them through AuthenticationServices' web-browser APIs with the page's origin and challenge, and passes the credential back. Conditional (autofill) requests stay pending, since there's no autofill UI; sites show their "Sign in with a passkey" button instead. Frames and U2F `appid` requests are left to WebKit.

The web-browser APIs need the restricted `com.apple.developer.web-browser.public-key-credential` entitlement, which Apple grants per App ID and only a provisioning profile can carry. Without it the bridge stays out of the way and WebKit handles WebAuthn as before (for most sites, not at all). To turn it on:

1. In the Apple Developer portal, under Certificates, Identifiers & Profiles, register explicit App IDs `dev.abromux.app` and `dev.abromux.nightly`, and request the "Web Browser Public Key Credential Requests" capability for each (a request Apple reviews).
2. Once it's granted, make a Developer ID provisioning profile for each App ID with your Developer ID Application certificate.
3. Save them as `assets/app/dev.abromux.app.provisionprofile` and `assets/app/dev.abromux.nightly.provisionprofile` (or point `$ABROMUX_PROVISION_PROFILE` at one), then run `scripts/nightly.sh` or `scripts/bundle-app.sh`. It embeds the profile and signs its entitlements in, and says "Passkeys on".

The first passkey request asks to let AbroMux use passkeys (System Settings › Privacy & Security › Passkey Access for Web Browsers); a request that may use a phone also asks for Bluetooth. If passkeys are denied, security keys still work.

## Session restore

Quitting saves the session to `~/Library/Application Support/abromux/session.json` (the layout is also saved every few seconds, so a crash loses little). The next launch restores the window frame, workspaces with their names, descriptions, colors and groups, the split layout and ratios, tabs and their names, each terminal's working directory and font size, browser pages with their back/forward history (saved on quit) and zoom, and up to 2000 lines of each terminal's output (with colors). By default the shells themselves keep running while abromux is closed and come back exactly as they were (see [Persistence](#persistence)); what follows is how terminals come back when they couldn't be kept (after a reboot) or with `"persistence": "cmux-like"`. Then shells start fresh: running programs aren't kept alive. Coding agents' sessions are the exception. Run `abromux hooks install` once and abromux learns, through the agents' hooks, which session runs in which terminal (the same hooks feed the sidebar status and notifications; rerun `install` after updating abromux to pick up new hook events). On restore, it types the agent's resume command into that terminal's new shell: `claude --resume <id>` (with the same permission mode), `codex resume <id>`, `gemini --resume <id>`, `droid --resume <id>`, `cursor-agent --resume <id>`, `opencode --session <id>` or `pi --session <id>`. A Codex, OpenCode or Pi you had quit isn't resumed. The hooks go after any hooks already there and do nothing outside abromux: `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`), `~/.codex/hooks.json` (or `$CODEX_HOME`), `~/.gemini/settings.json`, `~/.factory/hooks.json` (or the `hooks` in `~/.factory/settings.json` when there's no hooks.json) and `~/.cursor/hooks.json`. OpenCode and Pi get a plugin file instead: `~/.config/opencode/plugins/abromux.js` and `~/.pi/agent/extensions/abromux.js`. Agents whose folder doesn't exist are skipped; `abromux hooks install gemini pi` does just those. Codex asks you once to review new hooks: trust them when it does. They point at the binary you ran `install` from, so run it from the installed app; `abromux hooks uninstall` removes them. `ABROMUX_NO_RESTORE=1` skips it; `ABROMUX_SESSION_PATH` uses another file. `ABROMUX_DEMO` and `ABROMUX_SCRIPT` runs don't touch the session unless `ABROMUX_SESSION_PATH` is set.

Reopen Previous Session (⌘⇧O, File menu, or `abromux restore-session`) brings back the workspaces of the session this launch restored that aren't open any more, such as ones you closed by mistake. Workspaces still open as they were are skipped, so it never doubles one. `abromux restore-session --from nightly` (or `stable`, or a file) adds another session's workspaces, for example after trying the nightly app; `abromux restore-session --export <file> [--force]` writes the current one to a file.

### Importing from cmux

File › Import cmux Session (also in the command palette, or `abromux restore-session --from cmux`) adds cmux's workspaces. It reads the file cmux restores itself from, `~/Library/Application Support/cmux/session-com.cmuxterm.app.json`, which cmux saves every few seconds while it runs and again on quit, so it works with cmux open or closed. `--from <file>` takes any cmux session file too, such as one under `cmux/session-history/`. What comes over: every cmux window's workspaces and their names (one an agent gave is replaced as abromux renames workspaces), the splits and their ratios, each pane's tabs and selected tab, the focused pane, each terminal's directory, font size and output, browser pages with their back/forward lists, and coding agents, which resume in their terminal (`claude --resume <id>` and so on) as on any restore. cmux's own extras (markdown panes, pinned workspaces, status pills, notifications) don't. As with Reopen Previous Session, a workspace already open here as it is in cmux is skipped, so importing twice adds nothing new. Shells start fresh in abromux; the programs running in cmux's terminals keep running there. An agent still working in cmux is resumed here as well, so quit it in cmux (or quit cmux) first if you don't want both on the same session.

## Sidebar

Each workspace's row shows its name, a description if it has one, the focused tab's title, the latest unread notification, status pills, progress and the newest log line from scripts, and the git branch and directory. Right-click a row for its menu: rename, add or edit a description, pick a color, move it, put it in a group, or close it (or every other one).

- **Description**: ⌥⌘E (or right-click → Add Description…) edits it under the name. Return saves, ⇧↩ adds a line, Esc cancels, and an empty one clears it.
- **Color**: right-click → Color, from cmux's sixteen (Red, Crimson, Orange, Amber, Olive, Green, Teal, Aqua, Blue, Navy, Indigo, Purple, Magenta, Rose, Brown, Charcoal) or your own palette (`workspaceColors.colors`). It shows as a bar down the row's left edge, or fills the row with `"indicatorStyle": "solidFill"`. A layout's `"color"` sets it too.
- **Order**: drag a row to move it. A line shows where it will land, between two rows; between two of a group's workspaces puts it in that group, and above a group's header keeps it out. Dropping it on a header makes it the group's first. Right-click → Move Up / Move Down / Move to Top, or ⌘⇧P → Move Workspace Up / Down. ⌘1…⌘9 follow the order.
- **Groups**: a header above workspaces that belong together. Right-click a row → Group → New Group starts one (named after the repository, with the name selected so you can type a new one), or ⌘⇧P → Group Workspaces by Folder groups every workspace that shares a repository or folder with another. Click a header to fold it (a folded group still shows the selected workspace, and a dot if one has an unread notification), double-click to rename it, drag it to move the whole group, and use its `+` for a new workspace in the group's folder. Right-click a header to rename, color, ungroup or close it. ⌘N from a workspace in a group puts the new one in the group.
- **Auto-naming** (off by default): with `"automation": { "workspaceAutoNaming": true }`, the first coding agent to run in a workspace names it; agents started later in other terminals leave the name alone. Claude Code titles its own terminal from the session, so a Claude workspace takes that title, and follows it as Claude Code retitles. Other agents' prompts are turned into a short name by that agent's own CLI (`codex exec`, or `claude -p` on Haiku), and renamed as later prompts move the work on (very short ones like "yes" don't count). That runs a request on your account per rename. A name you give the workspace always wins and stops the renaming; clearing it (an empty rename) starts it again. A try that fails (the CLI timing out, a rate limit) is made again on the next prompt, and after three in a row only every ten minutes. Agents other than Claude Code and Codex are named by whichever of those two is installed. Without either, the name follows the agent's terminal title. A ⇧⌘I workspace left on its made-up name is auto-named this way even with the setting off; typing a name in the ⇧⌘I dialog counts as naming it.

## Workspace folders

⇧⌘I (File › New Workspace Folder) makes a workspace with a folder of its own, a scoped place for an agent to work in. A dialog asks for its name, showing a made-up one (like `quiet-amber-falcon`) and the folder it will make; Enter with nothing typed takes the made-up name, Esc cancels. The folder goes under `~/.abromux/ws/` (`~/.abromux/ws-nightly[-<name>]/` for a nightly), and a shell opens in it. To put them elsewhere, pick a folder in Settings › New Workspaces › Workspace folders, or set `app.workspaceFoldersDirectory` (`~` works); `$ABROMUX_WORKSPACES_DIR` overrides both. Folders made before a change stay where they are. Renaming the workspace later doesn't move the folder, since shells and worktrees point into it.

The folder holds `.abromux/workspace.json` (its name and what was checked out) and an `AGENTS.md` (with a `CLAUDE.md` that imports it) telling the agent to bring projects in rather than edit them in place. Shells in the workspace get `$ABROMUX_WORKSPACE_DIR`. The workspace keeps its folder across restarts.

- `abromux folder add <path> [--branch <name>]` checks a project out into the folder: for a git repo, a worktree of the whole repo on a new branch `ws/<folder>` (or `--branch`, which may already exist if no other worktree has it); anything else is copied.
- `abromux folder` prints the folder, `folder list` its checkouts, `folder all` every workspace folder, `folder repos` the git repos right under `~/repos`, `~/src`, `~/code`, `~/projects`, `~/dev` and `~/Developer`, and `folder root` where the folders live. They work from any shell inside a workspace folder and don't need the app.

Closing the workspace keeps its folder.

### Cleaning up

⇧⌘X (or right-click a workspace with a folder › Clean Up and Delete Folder…) moves the workspace into the **Cleanup** group, which always stays last in the sidebar, and opens a tab in the folder running a cleanup agent: `claude 'Read .abromux/cleanup.md and follow it.'`. It looks for everything the workspace made outside its folder, removes those things without asking, and ends with `abromux folder remove`, which closes the workspace. Workspaces are treated as scratch folders for one task, so it only stops to ask when work would really be lost: uncommitted changes or unpushed, unmerged commits in a worktree, or a changed copy of a folder. Things that were there before or are shared with other projects are kept, and listed at the end. Claude asks before each command as usual. Set `automation.workspaceCleanupAgent` for another agent CLI; the instruction is passed as its one argument.

To make that findable, each folder's `AGENTS.md` asks agents to keep track of what they make:

- Shells in the workspace get `COMPOSE_PROJECT_NAME=<folder>`, so `docker compose` stacks started there are the workspace's.
- Containers, volumes and networks made with plain `docker` get `--label abromux.workspace=<folder>`.
- Anything else (a database on a shared server, a cloud bucket, a `brew services` entry, a long-running process) is written down in `.abromux/resources.md`, with the command that removes it.

`abromux folder resources` shows that list and the Docker resources it finds by label or compose project.

`abromux folder remove [<path>] [--force]` does the deleting without an agent: `git worktree remove` for each worktree in its repo, then the folder. It deletes a `ws/<folder>` branch it made if that's merged, and keeps it otherwise (a branch passed to `--branch` that already existed always stays). It refuses while a worktree has uncommitted or untracked files, or Docker resources of the workspace are still there; `--force` goes ahead anyway. If AbroMux has the workspace open, the app closes it first and then deletes the folder, so it works from the workspace's own shell, and a notification says how it went. `abromux folder all --closed` lists folders no open workspace has, to remove by path.

## GitLab merge requests

⌥⌘B (View › Toggle Merge Requests Panel, or `abromux mr panel`) shows a panel on the window's right with three lists:

- **This workspace**: open merge requests from the branches checked out in the selected workspace: each checkout in its folder (the branch it's on now, and the one `folder add` made) and each terminal's repo, where the `origin` remote is on your GitLab.
- **Mine**: your open merge requests, plus the ones you're asked to review (Settings can drop those, or add ones assigned to you).
- **Recently opened**: the ones you opened lately in GitLab tabs, newest first.

Click one, or find any cached one with ⇧⌘M (File › Go to Merge Request…; paste a link or `group/project!12` to open one that isn't cached), and it opens in a [GitLab tab](#gitlab-reviews) in the focused pane, or the tab already showing it. It shows at once from the cache, and refreshes quietly.

`"gitlab": { "enabled": false }` (Settings › GitLab › Use GitLab) turns GitLab off: merge request links open as web pages, the panel, its commands and its background checks are gone, and workspaces don't list their branches' merge requests. Local diffs still open.

It uses the GitLab tab's sign-in: the GitLab you signed in to there and its token in the Keychain (`ABROMUX_GITLAB_TOKEN` overrides it). Settings › GitLab › Host picks another of those, e.g. when you've signed in to several.

**What's cached.** A merge request stays cached while it's in one of the lists, or open in a GitLab tab. Merged and closed ones are dropped at once, unless a tab still shows them. Past `recentLimit` (50), the longest-unopened ones drop out of Recently opened. Of each, what its tab shows is loaded in the background, the way the tab loads it, and what GitLab answered is kept on disk: a tab opened on it (also after a restart) draws that at once, then refreshes. Diffs come whole, within limits: a file whose diff is bigger than `maxFileDiffKb` (1024), and every file past `maxMrDiffMb` (20) or `maxMrDiffFiles` (3000) of a merge request's diff, is listed with its line counts and fetched when it scrolls into view, as GitLab's own too-big files are. Past `cacheSizeMb` (500) on disk, the loads of recently opened ones you haven't opened for longest go first, then yours; they stay listed, and are loaded again when you open them.

**Keeping it fresh.** A cheap GraphQL check asks about many merge requests in one request: their update time (comments change it), state, pipeline and head. Only when that shows a change is the merge request loaded again, and a tab showing it takes the new load in place. How often each is checked: on screen every `onScreenSeconds` (20); pipeline running every `runningPipelineSeconds` (30); yours, the workspaces' and the `frequentRecent` (10) most recently opened every `mineSeconds` (60); the other recent ones every `recentSeconds` (300). While the app is in the background, every interval is three times longer; coming back checks anything older than half a minute. Requests run one at a time in two lanes, so a big merge request loading in the background never holds up the one on screen.

**Rate limits.** A 429 from GitLab pauses the panel's requests until its `Retry-After` and doubles all the intervals (to ×32 at most); running low on `RateLimit-Remaining` stretches them before it comes to that. After five minutes without either, they shrink by a fifth, back to normal. The more they're stretched, the less runs at all: background loads stop first, the merge request on screen last. The panel says when it's slowed down or paused.

Everything is in `gitlab-cache/` under Application Support (`index.json` for the lists, `reviews/` for the loads); another host starts the lists afresh. All the values above are under `"gitlab"` in the config file and in Settings › GitLab:

```json
"gitlab": {
  "enabled": true,
  "host": "https://gitlab.example.com",
  "includeReviewRequests": true, "includeAssigned": false,
  "recentLimit": 50, "cacheSizeMb": 500,
  "maxFileDiffKb": 1024, "maxMrDiffMb": 20, "maxMrDiffFiles": 3000,
  "onScreenSeconds": 20, "runningPipelineSeconds": 30,
  "mineSeconds": 60, "frequentRecent": 10, "recentSeconds": 300
}
```

`abromux mr list [mine|recent|workspace]` prints the cache (`--json` for everything), `abromux mr open <url|group/project!iid>` opens one in the caller's workspace, and `abromux mr refresh [<ref>]` checks everything now, or loads one again.

## Tracked links

A workspace keeps a list of links: the Jira tickets, GitLab merge requests and pages its work is about. The selected workspace shows them in the sidebar, under its row: a ticket with its key (`PAY-1423`) and title, a merge request with just its title (no `Draft:` marker or ticket keys in front of it); the ones on screen are highlighted. A merge request stacked on another (its target branch is the other's source branch) is listed under it, indented. Hold ⌥⌘ to see their numbers, and ⌥⌘1…9 shows one.

Showing a link never makes a second tab of it: if a tab already shows that ticket or merge request (any page of it: `…/browse/PAY-1423`, a board with `selectedIssue=PAY-1423`, a merge request's `/diffs`), that tab comes forward. Otherwise it opens in the workspace's link pane, the pane its pages are in (a new one to the right if there's none), a merge request in a GitLab tab. ⌘-clicking a link in a terminal, or a page opening one in a new tab, goes to the existing tab the same way.

What's listed:
- Tickets and merge requests opened in the workspace's tabs are tracked on their own.
- Its merge requests from its branches (the merge requests panel's "This workspace") are listed after the tracked ones.
- ⌥⌘T tracks the focused tab's page, whatever it is; again untracks it. The × on a row untracks it too, and it isn't tracked again on its own.
- From a terminal: `abromux link add <url|PAY-123|group/project!iid>`, `abromux link remove <n|key|url>`, `abromux link [list]`, `abromux link open <n|key|url>` (`--focus false` leaves focus where it is), and `abromux link open --all` for every one. Workspace folders' `AGENTS.md` asks agents to track what they work on this way.

A bare key like `PAY-1423` needs to know its Jira: `"jira": { "host": "acme.atlassian.net" }` in the config file, else the one a ticket tracked before is on. `"sidebar": { "showLinks": false }` hides the list. The links are saved with the session.

## Settings window

⌘, (or the app menu › Settings…) opens Settings: a page each for General, Sidebar, Workspace Colors, Panes, Keyboard Shortcuts, Commands and GitLab, with a search field over all of them. It's a front end for the config file below, not a separate store. Each change writes just that key to `config.json` and applies at once. A setting put back to its default is removed from the file, and the rest of the file (other keys, their order, commands) stays as you wrote it. The undo arrow next to a setting resets it, and the one by a page's title resets the whole page.

On Keyboard Shortcuts, click a shortcut and press the new keys; Esc cancels. A shortcut needs ⌘, ⌃ or ⌥, unless it's a function key. If another action had that key, a toast says the new one takes it over. × removes a shortcut. Chords (`ctrl-a h`), per-context keys (`"when"`), workspace palette colors and commands are edited in the file; Settings shows them, and General › Config Files opens the file. If the file doesn't parse, Settings says why and saves nothing until it's fixed.

## Config file

abromux's own settings live in `~/.config/abromux/config.json` (or `$XDG_CONFIG_HOME/abromux/config.json`), shared by the release and nightly apps. Every key is optional:

```json
{
  "panes": {
    "resizeStep": 20,
    "dividerColor": "#3a3935",
    "focusBorderColor": "#c96442b0"
  }
}
```

`resizeStep` is how many pixels ⌃⇧H/J/K/L and `resize-pane` move a divider. `dividerColor` is the line between split panes, and `focusBorderColor` the border around the focused one. Colors are `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`. Unknown keys are errors, so a typo doesn't go unnoticed.

### Persistence

`"persistence"` picks what keeps terminals going when abromux quits:

- `"abromux"` (the default): shells keep running after abromux quits, like a tmux session you detached from, and the next launch takes them back. A dev server keeps serving and an agent keeps working.
  - **The holder:** shells run in `abromux hold`, a small background process. The app starts it on demand, one for the release and one for the nightly. It exits once it has no terminals left, and nothing needs installing.
  - **While abromux runs**, the holder passes each PTY to the app, which reads and writes it directly. Speed, latency and CPU are the same as with `"cmux-like"`.
  - **On quit**, each terminal's exact state is saved with libghostty-vt's snapshot format, in `held/` next to the session. The holder then keeps reading the shells.
  - **On the next launch**, you get the same shell back: the screen as you left it, then whatever it printed while abromux was closed (up to 16 MB per terminal).
  - **After a crash** there's no snapshot, so only the output since the crash comes back. A full-screen program redraws when the pane is resized.
  - **Closing a tab or workspace** ends its shells; quitting doesn't. `pkill -f "abromux hold"` ends every held shell without opening abromux.
  - **After a reboot**, abromux falls back to the `"cmux-like"` restore. So do runs that don't restore a session (`ABROMUX_NO_RESTORE`, `ABROMUX_DEMO`, `ABROMUX_SCRIPT`), whose shells are never held. `$ABROMUX_HOLD_SOCKET` moves the holder's socket.
- `"cmux-like"`: shells end with the app, as in cmux, and the next launch restores them as described in [Session restore](#session-restore).

The palette (⌘⇧P) has **Persistence: abromux** and **Persistence: cmux-like** to switch without editing the file. They write the key into `config.json` (the rest of the file stays as you wrote it) and reload. It applies to terminals opened after the change, and a reload is enough. A terminal already running in the holder is taken back at the next launch, whatever the setting says.

### Browser engine

```json
{ "browser": { "engine": "terminalBrowser" } }
```

`engine` is `builtIn` (the default, the WebKit browser pane) or `terminalBrowser` (`"terminal-browser"` works too); Settings has it under General › Browser. With `terminalBrowser`, every new browser tab (⌘⇧L, the globe, ⌘-clicked links, `url` commands, layouts, `abromux new-surface --type browser`) is a terminal running `terminal-browser open --no-merge <url>` in your login shell, and closing the browser closes the tab. The tab shows a globe and the page's title, as a built-in one does: abromux asks terminal-browser's control socket (`~/.local/state/terminal-browser-*/instances/*.sock`) for the active tab's title and URL, matched to the tab by its tty.

A terminal-browser tab takes the built-in browser's keys: ⌘L, ⌘R, ⌘[ / ⌘], ⌘= / ⌘- / ⌘0 (zoom), and ⌘A, ⌘C, ⌘X, ⌘Z, ⇧⌘Z in the page. ⌘F (find), ⌥⌘I (devtools) and ⌥⌘C (console) work as in the built-in browser: abromux sends terminal-browser the key it binds to the same command in its `shortcuts.json` (`~/.config/terminal-browser/`, or its settings with ⌃,), by default ⇧⌘F, ⇧⌘I and ⌥⌘J, read on each press. Its own keys work too, except ⇧⌘I, which is abromux's New Workspace Folder; F12 opens devtools. Window-level keys (⌘T, ⌘W, ⌘P, ⌘N, …) stay abromux's. ⌘-click opens a link in a new terminal-browser tab, the pointer follows the page (a hand over links), and the page gets key releases, focus and blur, and sideways scrolling. The Edit menu's Copy, Cut and Select All (and their keys) go to the page or its address bar, so ⌘L then ⌘C copies the URL. ⌘R on a local file (`file://`) reloads it through `terminal-browser action … reload`, since terminal-browser's own ⌘R does nothing there.

terminal-browser run in a shell (`terminal-browser open ./report.html`, as an agent does) is treated as a browser tab while it runs: abromux finds its control socket within two seconds and matches it to the pane by tty. The tab shows the page's title and a globe and takes the keys above, so ⌘R reloads and ⌘= zooms the page rather than the terminal's font. When the browser quits, the pane is a shell again, and the session saves it as the terminal it is.

abromux looks for `terminal-browser` on PATH, in `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin`; if it isn't there, the tab falls back to the built-in browser with a warning. The setting applies to tabs opened after it changes; open ones stay as they are. A terminal-browser tab is restored as a browser tab on the page it last showed (its own back/forward history isn't kept). The `abromux browser` automation commands only drive built-in tabs, so `abromux browser open` always opens one (use `terminal-browser action` to drive a terminal-browser).

`"terminalBrowserPanes": true` in `browser` (Settings: General › Browser › terminal-browser new tabs as panes) has terminal-browser open new tabs as abromux panes instead of its own tabs. Links that ask for a new tab (`target=_blank`, ⌘-click, middle-click), the context menu's "Open link in new tab" and terminal-browser's new-tab box then open a terminal-browser split to the right of the browser they came from (`TB_ABROMUX_SPLIT=down`, `left` or `up` in that pane's environment picks another side). Pages opened with `window.open` and window features (OAuth and SSO sign-ins that talk to their opener) stay popups. terminal-browser has no setting for this, so abromux adds a few lines to its `app/browser/dist/main.js`, and takes them out when the setting is off. That file is outside terminal-browser's signed Electron app, so its signature, entitlements and passkeys are untouched. `terminal-browser upgrade` replaces the file; abromux puts the lines back before it next starts a terminal-browser, and if the code they hook into has moved, it changes nothing and says so. A running terminal-browser keeps the code it started with: all of them share one daemon, so a change applies once every terminal-browser has quit. That daemon's environment is the first pane's, so the new pane comes from each browser's own `ABROMUX_SURFACE_ID`; a browser that isn't in an abromux pane opens a normal tab. Shells get `ABROMUX_BIN`, the CLI of the app they run in, which is what the new pane is made with.

### Opening files

```json
{ "editor": { "program": "tode" } }
```

AbroMux can open files from Finder. It's listed in Open With for text and source files, and for the code extensions `abromux default-app list` prints (`.rs`, `.ts`, `.toml`… that macOS has no text type for). It never makes itself their default app on its own. Each file opens in a terminal running the editor `program` names, `nvim` (the default) or `tode`, in a new pane to the right of the focused one. That's in the workspace whose folder, or focused terminal's directory, holds the file most closely, else the selected one. The editor starts in the workspace's folder when the file is in it, else in the file's own folder. tode runs as `tode -n <file>`, so it opens in that pane rather than in a tode window already open. Quitting the editor closes the pane. Settings has it under General › Editor. Folders opened the same way (dropped on the Dock icon) open as workspaces.

To have files open in AbroMux on a double-click:

```bash
abromux default-app code        # every extension `default-app list` prints
abromux default-app rs ts py    # or just these
```

That's what Finder's Get Info › Open with › Change All… does, one extension at a time; Change All… there takes it back to another app. It must be the `abromux` inside AbroMux.app (or a nightly), since the app the command belongs to becomes the default.

`abromux edit <file>…` opens files the same way from a shell, in a pane beside that terminal. A file that doesn't exist yet is fine if its folder does. The session saves an editor tab as the file it had open and opens it again on the next launch, in whichever editor is set then.

The pane's tab shows a pencil and the file's name. abromux looks for the editor on PATH, in `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin`; if it isn't there, a warning says so and nothing opens.

### Sidebar and workspace settings

These use cmux's key names. Shown with their defaults:

```json
{
  "app": {
    "appearance": "system",
    "workspaceInheritWorkingDirectory": true,
    "newWorkspacePlacement": "end",
    "automaticallyCheckForUpdates": true,
    "workspaceFoldersDirectory": null,
    "keepWorkspaceOpenWhenClosingLastSurface": false,
    "reorderOnNotification": true
  },
  "sidebar": {
    "fontSize": 12.5,
    "hideAllDetails": false,
    "wrapWorkspaceTitles": false,
    "showWorkspaceDescription": true,
    "workspaceDescriptionColor": null,
    "showNotificationMessage": true,
    "showBranchDirectory": true,
    "stackBranchDirectory": false,
    "pathLastSegmentOnly": false,
    "showProgress": true,
    "showLog": true,
    "showCustomMetadata": true,
    "selectionColor": null
  },
  "sidebarAppearance": { "tintColor": null, "tintOpacity": 0.18 },
  "workspaceColors": {
    "indicatorStyle": "leftRail",
    "selectionColor": null,
    "unselectedFillOpacity": 0.7,
    "colors": { "Red": "#C0392B", "Blue": "#1565C0" }
  },
  "workspaceGroups": { "newWorkspacePlacement": "afterCurrent", "groupByAgentStatus": false },
  "automation": { "workspaceAutoNaming": false, "workspaceCleanupAgent": null }
}
```

- `appearance`: `system` follows macOS's light or dark mode as it changes; `light` or `dark` keeps the app in one (also in Settings › General). It covers the windows, sidebar, menus, Markdown tabs, web pages that follow the system's color scheme, and terminals: a Ghostty `theme = light:A,dark:B` uses the one for the appearance, and without a theme the built-in colors switch too. Running terminals recolor when it changes. GitLab tabs stay in GitLab's dark theme.
- `workspaceInheritWorkingDirectory`: ⌘N and the `+` start in the focused terminal's directory; `false` starts in your home folder.
- `keepWorkspaceOpenWhenClosingLastSurface`: ⌘W on a workspace's last tab opens a fresh terminal in the same directory instead of closing the workspace. Only ⌘W: a shell that exits, or the tab's `×`, still closes it.
- `reorderOnNotification`: a workspace that gets a notification you aren't looking at moves to the top of the sidebar, or to the top of its group if it's in one. `false` (or `"off"`) keeps the order as you left it. cmux's `"notifications"` is the same as `true`; its `"agentActivity"` is taken as `"notifications"` too, since agent turns already notify here.
- `automaticallyCheckForUpdates`: look for a new release once a day ([Updates](#updates)).
- `workspaceFoldersDirectory`: where ⇧⌘I makes [workspace folders](#workspace-folders); `null` is `~/.abromux/ws` (per channel for nightlies).
- `newWorkspacePlacement`: where a new workspace goes, `top`, `afterCurrent` (after the selected one, or after its group) or `end`. `workspaceGroups.newWorkspacePlacement` is the same inside a group, where `top` means right after the group's first workspace.
- `workspaceGroups.groupByAgentStatus` (Settings › General) shows the sidebar in sections by what each workspace's coding agent is doing, instead of your groups (off by default): **Verify** (an agent waits for you), **Running**, **Idle** (an agent is open but done with its turn) and **No agent**. A workspace with several agents goes in the most pressing one. Sections fold on click; ⌘1…9 and next/previous workspace follow the order shown. Rows can't be dragged while it's on, and your groups come back unchanged when it's off. Making a group (New Group) turns it off, so the new group shows.
- `fontSize` sizes the workspace names (8–24); the smaller lines scale with it. `hideAllDetails` shows names only. `wrapWorkspaceTitles` lets long names wrap.
- `showWorkspaceDescription`, `showNotificationMessage`, `showBranchDirectory`, `showProgress`, `showLog` and `showCustomMetadata` (status pills) hide those lines when `false`. `stackBranchDirectory` puts the branch and the directory on lines of their own, and `pathLastSegmentOnly` shows `…/api` instead of `~/code/api`.
- `selectionColor` (in `sidebar` or `workspaceColors`) is the selected row's fill; it's the accent color by default. `sidebarAppearance.tintColor` lays a tint of that opacity over the sidebar.
- `workspaceColors.colors` replaces the palette the Color menu offers and `set-color` names (cmux's sixteen by default). `indicatorStyle` is `leftRail` or `solidFill`; with `solidFill`, rows other than the selected one show their color at `unselectedFillOpacity`.

### Shortcuts and commands

`"shortcuts"` changes the keys of any action, by name. `abromux config shortcuts` lists every name with its keys as they are now, marking the ones your file sets:

```json
{
  "shortcuts": {
    "splitRight": "cmd-\\",
    "closeTab": null,
    "lastPane": "ctrl-a l",
    "flashPane": ["⌘⇧H", "ctrl-a f"],
    "find": { "keys": "cmd-shift-f", "when": "Terminal" }
  },
  "commands": [
    { "title": "Run Tests", "run": "cargo test", "shortcut": "cmd-alt-t" },
    { "title": "Dev Server", "run": "npm run dev", "newTab": true },
    { "title": "Open Docs", "url": "https://docs.rs" }
  ]
}
```

- A shortcut is a keystroke like `cmd-shift-p` (modifiers `cmd`, `alt`/`option`, `ctrl`, `shift`) or `⌘⇧P`, or several separated by spaces for a two-step chord (`ctrl-a l`). A list gives an action several; `null` or `[]` unbinds it.
- Setting an action's keys replaces all of its defaults. A key you give one action is taken from whatever else had it by default, so `"splitRight": "cmd-k"` also frees ⌘K from Clear Scrollback.
- `when` binds it only in a key context: `Terminal`, `Browser`, `Browser > Input` (the address bar and find field), or a gpui context expression. By default an action keeps the contexts its defaults had (find works in terminals and browsers), and a command's shortcut works everywhere.
- Shift with punctuation is written either way: `cmd-shift-]` and `cmd-}` are the same key.

`"commands"` adds your own entries to the command palette (⌘⇧P), each with an optional `shortcut`. `run` types the text into the focused terminal and presses Return; with `"newTab": true`, or when a browser has focus, it runs in a new terminal tab instead. `url` opens a page in a new browser tab. `description` shows under the title in the palette, `keywords` are more words it finds the command by, and `"confirm": true` asks before every run.

`workspace` opens a new workspace built from a layout, in cmux's format:

```json
{ "title": "Web Dev", "restart": "ignore", "workspace": {
  "name": "Web", "cwd": "~/code/web", "env": { "PORT": "3000" }, "setup": "nvm use",
  "layout": { "direction": "horizontal", "split": 0.6, "children": [
    { "pane": { "surfaces": [
      { "name": "Dev", "command": "npm run dev", "focus": true },
      { "name": "Tests", "cwd": "./tests" }
    ] } },
    { "pane": { "surfaces": [{ "type": "browser", "url": "localhost:3000" }] } }
  ] }
} }
```

- A layout node is a split (`direction` `horizontal` for side by side or `vertical` for stacked, `split` the first child's share from 0.1 to 0.9, and exactly two `children`) or a `pane` with its tabs (`surfaces`). Without `layout` it's one terminal.
- A tab is a `terminal` (the default `type`) or a `browser`. A terminal can have a `command` to run, a `cwd` and an `env`; a browser has a `url`. `name` names the tab, and `focus` picks the tab that has focus when it's built.
- `cwd` is `~/…` under your home folder, `/…` absolute, or relative: a tab's to the workspace's, and the workspace's to the project (below) or the focused terminal's directory.
- `env` on the workspace goes to every shell in it; `setup` is typed into the first terminal before its command.
- `restart` says what to do when a workspace of that name is already open: `new` opens another (the default), `ignore` switches to it, `recreate` closes it and builds it again in its place, and `confirm` asks first.
- `abromux new-workspace --layout '<json>'` (or `--layout @layout.json`) builds a layout node from the CLI, with `--name` and `--cwd`.

A project can have its own commands. The palette looks from the focused terminal's directory upward for `.abromux/config.json`, or cmux's `.cmux/cmux.json` or `cmux.json`, and adds the `"commands"` of the first it finds (other keys in it are ignored). cmux's `name` and `command` work for `title` and `run`, so a `cmux.json` works as it is. A project command replaces one of yours with the same title, its palette entry names the project, and it has no shortcut. Since anyone who can commit to a repository can add one, a project command asks before it runs the first time, showing what it types; after you say Run it doesn't ask again until the command changes. `abromux config project` shows the project file found from the current directory and which of its commands will ask.

Reloading the config applies new shortcuts at once, in the menus too.

⌘⇧, (or ⌘⇧P → Reload Config, or `abromux reload-config`) reads the file again without a restart. If it doesn't parse, the current settings stay and a toast shows the error with its line and column, and it does the same at launch. `abromux config check [<file>]` validates a file without the app, `abromux config path` prints where it's read from, and `abromux config open` (or ⌘⇧P → Open Config File) opens it in its default app, making an empty one first if there's none. The Ghostty config below is separate and is still read only at launch.

### Notifications

```json
{
  "notifications": {
    "showInMenuBar": true,
    "hooks": [
      { "id": "no-banner-for-codex", "command": "jq 'if .notification.title == \"Codex\" then .effects.desktop = false else . end'" },
      { "id": "speak", "command": "say \"$(jq -r .notification.body)\"", "timeoutSeconds": 10 }
    ]
  }
}
```

`showInMenuBar` (on by default, as in cmux) puts a bell in the macOS menu bar with the unread count. Its menu lists the newest unread notifications (click one to go to it) and has Show Notifications, Jump to Latest Unread, Mark All Read and Clear All.

`hooks` are cmux's notification hooks: shell commands each notification (not the bell) passes through, in order. A hook gets it as JSON on stdin and prints it back, changed or not:

```json
{
  "version": 1,
  "notification": { "id": 7, "workspaceId": "workspace:2", "surfaceId": "surface:5",
                    "title": "Claude Code", "subtitle": "", "body": "Needs your input" },
  "context": { "cwd": "/path/to/project", "hookId": "speak", "appFocused": false, "focusedPanel": false },
  "effects": { "record": true, "markUnread": true, "desktop": true, "sound": true }
}
```

`record: false` drops the notification, `markUnread: false` records it as read, `desktop: false` skips the macOS banner and `sound: false` its sound; a changed `title`, `subtitle` or `body` is what's shown. A hook that prints nothing changes nothing, so any command can be a hook. Hooks run in the background (the banner waits for them) in the workspace's directory, with `ABROMUX_WORKSPACE_ID`, `ABROMUX_SURFACE_ID` and `ABROMUX_SOCKET_PATH` set so they can call `abromux`. If one fails, takes longer than `timeoutSeconds` (default 20) or prints something that isn't JSON, the notification goes ahead as if there were no hooks and a toast says which hook failed. `"enabled": false` turns one off. Unlike cmux, only this file's hooks run, not a project's.

## Ghostty config

Terminals read your Ghostty config: `~/.config/ghostty/config` (or `$XDG_CONFIG_HOME`), then `~/Library/Application Support/com.mitchellh.ghostty/config`, plus their `config-file` includes. abromux uses `theme` (from `~/.config/ghostty/themes` or Ghostty.app; `light:A,dark:B` follows the app's appearance), `font-family` (further `font-family` lines are fallbacks, as in Ghostty), `font-size`, `foreground`, `background`, `palette`, `cursor-color`, `cursor-text`, `cursor-style`, `selection-background`/`-foreground`, `keybind`s whose action is `text:`, `csi:` or `esc:`, and `clipboard-paste-protection` / `clipboard-paste-bracketed-safe` (on by default, as in Ghostty: pasting text that could run commands, such as a newline when the program hasn't turned on bracketed paste, asks first; Enter pastes, Esc cancels). Programs can copy to the clipboard with OSC 52, as terminal-browser, Neovim and tmux do (`clipboard-write = deny` stops them; there's no prompt, so `ask` refuses too). `copy-on-select = clipboard` copies a selection to the clipboard when you let go of the mouse (Ghostty's default `true` uses a selection pasteboard macOS doesn't have, so it does nothing here). `mouse-scroll-multiplier` scales scrolling, as one number or `precision:<n>,discrete:<n>` for the trackpad and the mouse wheel. `scrollbar = never` hides the overlay scrollbar. `shell-integration = none` turns off abromux's shell integration (below). Other keys are ignored for now. Changes apply to new terminals.

Ghostty has Nerd Font symbols built in; abromux doesn't, so for the icons Neovim, eza or starship draw it falls back to the Nerd Fonts you have installed (symbols-only and "Mono" ones first), after your `font-family` fallbacks and before macOS's own fonts. Install one (`brew install --cask font-symbols-only-nerd-font`) if icons show as boxes.

## Shell integration

zsh starts with a small integration (`src/shell/zsh`, written to `~/Library/Application Support/abromux/shell-integration/zsh`) that marks the prompt with OSC 133. abromux sets `ZDOTDIR` to that folder; its `.zshenv` puts your `ZDOTDIR` back, runs your `.zshenv`, and zsh reads the rest of your config as usual. With the marks, resizing a pane blanks the prompt for zsh to redraw instead of rewrapping it, which used to pile up pieces of the prompt line. One copy can still stay behind when a narrower pane wraps the prompt, until the pane widens again: since Ghostty `dde3d4d6b`, libghostty-vt clears the prompt after the reflow, from its last wrapped row only (Ghostty 1.3.1 cleared first). A `zsh` started inside a pane doesn't get the integration. Other shells have none yet.

## CLI

The same `abromux` binary controls a running app over a Unix socket (`~/Library/Application Support/abromux/abromux.sock`, or `$ABROMUX_SOCKET_PATH`). Command names follow cmux's CLI; `abromux --help` lists them all.

```bash
abromux ~/code/project                      # new workspace there (launches abromux if needed)
abromux list-workspaces
abromux tree --all
abromux new-split right --command 'cargo watch -x test'
abromux send --surface surface:3 'ls -la\n'
abromux send-key --surface surface:3 ctrl-c
abromux read-screen --surface surface:3 --scrollback --lines 50
abromux read-selection --surface surface:3  # the selected text (also read-screen --selection; works in a browser tab too)
abromux rename-tab --surface surface:3 'dev server'   # no title: back to the program's own
abromux tab-action --tab surface:3 close-right   # also close-left, close-others, new-terminal-right, new-browser-right,
                                                 # duplicate, move-to-new-workspace, reload, rename, clear-name, mark-read, mark-unread
abromux respawn-pane --surface surface:3 --command 'npm run dev'   # kill the shell, start a fresh one
abromux last-pane                           # back to the pane focused before
abromux swap-pane -L                        # swap the focused pane's tabs with the pane to its left
abromux move-surface surface:3 --pane pane:2 --index 0   # or --workspace workspace:2, --before/--after surface:5
abromux reorder-surface surface:3 --after surface:5      # within its pane
abromux split-off --surface surface:3 right            # into a new pane beside its own (alias drag-surface-to-split)
abromux move-tab-to-new-workspace --surface surface:3 --title logs   # aliases detach-tab, break-pane
abromux join-pane --target-pane pane:2 --pane pane:4   # pane:4's active tab to the end of pane:2
abromux zoom-pane                           # like ⌘⇧↩; on, off or toggle
abromux trigger-flash --surface surface:3   # flash its pane, like ⌘⇧H
abromux markdown README.md                  # a Markdown tab split off to the right; it follows edits to the file
abromux edit src/main.rs                    # the file in editor.program (nvim or tode), in a pane split off to the right
abromux browser open https://example.com
abromux browser eval 'document.title'
abromux browser zoom 125%                   # or in, out, reset, 1.5; no argument prints it
abromux notify --title 'Tests' --body 'all green'   # rings the pane it runs in
abromux list-notifications
abromux jump-to-unread
abromux mark-notification-read --workspace workspace:2   # or --id 4, --all
abromux dismiss-notification --all-read     # or --id 4
abromux wait-for build-done --timeout 600   # blocks until another script runs: abromux wait-for -S build-done
abromux set-status build compiling --color '#e0a84f'   # a pill under the workspace
abromux set-progress 0.4 --label 'Building'
abromux log --level error --source build 'Build failed'
abromux workspace-action set-description 'Ship the release'   # cmux's workspace actions
abromux workspace-action --workspace workspace:2 set-color Teal
abromux reorder-workspace --workspace workspace:4 --index 0
abromux group create --name api             # a sidebar group holding this workspace
abromux group add --group group:1 --workspace workspace:5
abromux new-workspace --cwd ~/code/api --group group:1 --description 'Fix the 500s'
abromux restore-session --from nightly
abromux --json list-panes
```

`abromux browser` also automates pages, for agents and scripts. `snapshot` prints the page as an accessibility tree with a ref per element, and the action commands take a ref from the latest snapshot (`e3`) or a CSS selector:

```bash
abromux browser snapshot --interactive        # - textbox "Username" [ref=e2] ...
abromux browser fill e2 'alice@example.com'
abromux browser click 'button[type=submit]'
abromux browser wait --text 'Welcome' --timeout 5000
abromux browser get text '#result'
abromux browser screenshot page.png
```

There are also `dblclick`, `hover`, `focus`, `check`/`uncheck`, `type`, `press`, `keydown`/`keyup`, `select`, `scroll`, `get`, `is`, `find` (by role, text, label, placeholder, alt, title or test id) and `wait` for selectors, URLs and load states. The events are synthesized in the page with JavaScript, so sites that only react to real input won't respond to them.

More for debugging and sessions:

```bash
abromux browser console --level error          # what the page logged (also --filter, --limit, --clear)
abromux browser errors                         # uncaught exceptions and unhandled rejections
abromux browser network --filter /api/         # fetch/XHR with status and time, plus the page's other loads
abromux browser frame e7                       # later commands run in that iframe (cross-origin too); `frame main` goes back
abromux browser tab new https://example.com    # also `tab` (list), `tab switch 1`, `tab close`
abromux browser cookies                        # the page's cookies, HttpOnly too (--all: every site)
abromux browser cookies set theme dark --path /
abromux browser storage local set token abc    # local or session: get [key], set, remove, clear
abromux browser state save auth.json           # cookies + localStorage, Playwright's storageState format
abromux browser state load auth.json
abromux browser wait-download --path ./out/    # the running download, else the next one; prints its path
```

A page script in every frame (`src/browser_page.js`) records console messages, errors and requests (the last 1000 of each per pane, kept across navigations), and gives each frame a token. `frame` asks the iframe for its token with a `postMessage`, which works across origins, and later scripts run there through WebKit's `evaluateJavaScript:inFrame:`. `network` sees `fetch` and XHR with their status, and the page's other loads (documents, scripts, styles, images) through the Resource Timing API, whose status WebKit may leave blank. Cookies go through the webview's `WKHTTPCookieStore`. `state load` restores localStorage only for the origin the page is on, so open the site first.

Coding agents running in abromux can learn this CLI from two agent skills in [`skills/`](../skills): `abromux` (splits, tabs, running and reading other terminals, sidebar status and notifications) and `abromux-browser` (the browser automation above). `abromux skills install` copies them into `~/.claude/skills` (or `$CLAUDE_CONFIG_DIR`) and, if Codex is set up, `~/.codex/skills` (or `$CODEX_HOME`). They're built into the binary, so rerun it after updating abromux; `abromux skills uninstall` removes them, and `abromux skills show [name]` prints one.

`abromux completion zsh` (or `bash`, `fish`) prints a completion script for the commands and their subcommands: `eval "$(abromux completion zsh)"` in `~/.zshrc`, after `compinit`.

`wait-for` works without the app, through a file per name in `$TMPDIR`, as cmux does: a signal sent before anyone waits is kept, and each one wakes a single waiter.

Targets are refs (`workspace:2`, `pane:5`, `surface:3`) or 0-based indexes. Each shell gets `ABROMUX_SOCKET_PATH`, `ABROMUX_WORKSPACE_ID` and `ABROMUX_SURFACE_ID`, so `abromux` run inside a pane acts on that pane by default (otherwise on what's focused). The wire format is one JSON object per line (`{"id":1,"method":"workspace.list","params":{}}`); `abromux rpc <method> [json]` sends one directly.

## Keyboard and mouse

| | Mouse | Keys |
|---|---|---|
| New workspace | `+` in sidebar header / footer | ⌘N |
| New workspace folder | File › New Workspace Folder | ⇧⌘I |
| Review every repository of the workspace in [hunk](https://hunk.dev/): a column right of the focused pane, one pane per repository (its folder's checkouts and those its tabs are in) running `hunk diff --watch` against where the branch forked from the default branch, as ⌥⌘D compares | File › Review Changes in Hunk | ⌘⇧P → Review Changes in Hunk (`reviewInHunk` in `"shortcuts"`) |
| Open the workspace in Neovim: a new terminal tab in its folder (else the focused tab's directory) running `nvim .` | File › Open Workspace in Neovim | ⌘⇧P → Open Workspace in Neovim (`openInNeovim` in `"shortcuts"`) |
| Clean up and delete workspace folder | right-click → Clean Up and Delete Folder… | ⇧⌘X |
| Rename workspace | double-click its name (Enter saves, Esc cancels) | ⌘⇧R |
| Workspace description | right-click → Add Description… (Return saves, ⇧↩ new line) | ⌥⌘E |
| Workspace color, group, move, close others | right-click the row | |
| Reorder workspaces | drag a row (onto a group to join it) or a group header | ⌘⇧P → Move Workspace Up / Down |
| Fold a group | click its header (double-click renames, right-click for more) | ⌘⇧P → Collapse or Expand Group |
| Reopen workspaces closed since launch | | ⌘⇧O |
| Delete workspace | hover → `×` | ⌘⇧W |
| Select workspace | click | ⌘1…⌘8, ⌘9 for the last, ⌃⌘[ / ⌃⌘] (hold ⌘ to see the numbers) |
| New terminal tab | terminal icon in pane tab bar | ⌘T |
| New browser tab | globe icon | ⌘⇧L (⌘L focuses address bar) |
| Split right / down | split icons | ⌘D / ⌘⇧D |
| Move tab / split by dragging | drag a tab onto a tab bar, a pane's edge, the window's edge or a workspace in the sidebar | ⌘⇧P → Move Tab to New Workspace |
| Rename tab | double-click it (Enter saves, Esc cancels; an empty name goes back to the program's title) | ⌘R (in a browser tab, terminal-browser ones too, ⌘R reloads; use ⌘⇧P → Rename Tab…) |
| Close tab | tab `×` / middle-click | ⌘W (last tab closes the pane; last pane closes the workspace, unless `app.keepWorkspaceOpenWhenClosingLastSurface`) |
| Cycle tabs | | ⌘⇧[ / ⌘⇧], ⌃Tab / ⌃⇧Tab |
| Select tab in focused pane | click | ⌃1…⌃8, ⌃9 for the last (hold ⌃ to see the numbers) |
| Focus pane in a direction | click | ⌥⌘← → ↑ ↓ |
| Focus the previously focused pane | | ⌘⇧P → Focus Last Pane |
| Swap the focused pane with its neighbour | | ⌘⇧P → Swap Pane Left / Right / Up / Down (focus moves with it) |
| Resize focused pane | drag a divider | ⌃⇧H / J / K / L (20 px a press, `panes.resizeStep` in the [config file](#config-file)) |
| Flash focused pane | | ⌘⇧H |
| Zoom focused pane to fill the workspace, or unzoom | | ⌘⇧↩ (focusing another pane unzooms) |
| Command palette: every command that applies to the focused terminal or browser, with its shortcut | | ⌘⇧P, type to filter, ↑/↓ or ⌃N/⌃P, Enter runs, Esc closes |
| Switch workspace by name | | ⌘P |
| Notifications list | bell icon in the titlebar | ⌘I (Esc closes) |
| Jump to newest unread notification | click it in the list or the menu-bar bell | ⌘⇧U |
| Mark the focused terminal unread, or read | | ⌥⌘U |
| Mark it the oldest unread and jump to the next | | ⌃⌘U |
| Open a link in a terminal | ⌘-click | |
| Open a page link in a new tab | ⌘-click (⇧⌘-click to switch to it) | |
| Toggle sidebar | sidebar icon | ⌘B |
| Copy / paste in terminal | drag to select, double-click selects a word (with `copy-on-select = clipboard` it's copied right away) | ⌘C / ⌘V (asks first if the text could run commands) |
| Scroll back | wheel or trackpad; the scrollbar shows while scrolled back and can be dragged | |
| Password prompt | a lock icon in the corner while the terminal doesn't echo what you type | |
| Paste file paths | drop files on a terminal (shell-escaped) | |
| Restart the shell (kills what runs in it) | | ⌘⇧P → Restart Shell |
| Font size (per terminal) | | ⌘+ / ⌘- / ⌘0 |
| Clear screen and scrollback | | ⌘K |
| Find in terminal (screen and scrollback) | | ⌘F, then ⌘G / ⇧⌘G or Enter / ⇧Enter, Esc to close; ⌘E finds the selection |
| Find in page (browser) | | ⌘F, same keys as find in terminal |
| Browser back / forward / reload | arrows and ↻ next to the address bar | ⌘[ / ⌘], ⌘R |
| Browser zoom in / out / actual size (kept per tab, and across restarts) | View menu | ⌘= / ⌘-, ⌘0 |
| Web inspector / JS console | | ⌥⌘I / ⌥⌘C |
| Settings | app menu › Settings… | ⌘, |
| Reload the config file | | ⌘⇧, |
| Change any of these keys | | `"shortcuts"` in the [config file](#shortcuts-and-commands) |

## Performance

`scripts/perf.py run` builds a release binary and measures it:

- binary size
- headless VT parse throughput (`abromux bench-vt`)
- headless kitty graphics frame cost: parse, snapshot, BGRA conversion (`abromux bench-kitty`)
- startup time
- idle memory and CPU
- memory per terminal
- in-app throughput
- frame draw time and late frames during in-app throughput
- kitty graphics in-app: 1920×1200 frames per second through shared memory (as terminal-browser sends them) and as base64 through the pty, frame draw p99 while streaming, and the footprint they add
- UI actions, each until the frame showing its result is on screen: new tab, split and workspace (until the prompt), closing a tab, switching workspaces, focusing tabs and panes, keystroke echo, scrolling, and a browser pane until its page has loaded. Each one also records that frame's draw time, which is abromux's own part. The terminal timings use a bare zsh so your shell config doesn't count, and one more times a new tab with your own login shell.

Each run is recorded with its commit, branch and machine, then compared with the last 10 runs on the same machine. A metric is flagged only when it's worse by more than both its own threshold and 3× the baseline's spread. Run it after each change; leave the Mac alone for the ~1.5 minutes it takes.

```bash
scripts/perf.py run                       # full suite (~1.5 min, opens the app)
scripts/perf.py run --headless            # no window: size + VT parse (~10 s)
scripts/perf.py run --label "try X"       # note stored with the run
scripts/perf.py history                   # recent runs with trend sparklines
scripts/perf.py history vt_color_mbps     # one metric over time
scripts/perf.py compare <id>              # re-compare a recorded run
scripts/perf.py report                    # HTML report with a chart per metric (opens it)
scripts/perf.py report --md -o perf.md    # the same as Markdown, e.g. for a PR
scripts/perf.py install-hook [--block]    # headless suite before every git push
```

History lives in `<git common dir>/abromux-perf/history.jsonl`, outside the tree. All worktrees share it and commits never touch it; `$ABROMUX_PERF_HISTORY` overrides the location. [`perf-vs-cmux.md`](perf-vs-cmux.md) compares abromux with cmux, measured with `scripts/bench.sh`.

## Layout

- `src/ghostty/`: FFI bindings and a safe wrapper (`Terminal`, `Frame` snapshots, key encoding, selection)
- `src/terminal.rs`: PTY plumbing plus a custom GPUI `Element` that paints the grid
- `src/browser.rs`: the webview pane; `src/webauthn.{rs,js,m}`: the passkey bridge
- `src/terminal_browser.rs`: browser tabs that run terminal-browser (`browser.engine`)
- `src/editor.rs`: files opened from Finder or `abromux edit`, in a terminal editor (`editor.program`); `abromux default-app`
- `src/workspace.rs`: the workspace, split-tree and surface model
- `src/main.rs`: the app shell (pane tab bars, splits, actions); `src/sidebar.rs`: the sidebar's rows, groups, menus and drag to reorder
- `src/bench.rs`: `abromux bench-vt` and `abromux bench-kitty`, the headless parse benchmarks used by `scripts/perf.py`
- `src/server.rs`: the control socket's method handlers; `src/cli.rs`: the `abromux` commands; `src/ipc.rs`: the shared wire format
- `src/plugins.rs`: the plugins built in; `crates/abromux-plugin`: the interface a plugin's tab implements; `crates/abromux-gitlab`: the GitLab review tab
- `dev/gitlab/`: a GitLab in Docker with review fixtures, and scripts to drive and screenshot the plugin (see its README)

## Known POC limits

- Native webviews always draw above GPUI content. Hidden tabs and workspaces are hidden explicitly, and the app's popovers swap a page they cover for a still bitmap of it (see "Popovers over pages"); a popover the app doesn't know about (a new one) would still draw under the page.
- While a page has keyboard focus, keys with ⌘ or ⌃ go to the app first (`abx_page_shortcuts_install` in `src/browser_native.m`): a shortcut the app handles doesn't reach the page, so a site can't use ⌘T, ⌘W, ⌘L and the like for itself. Other ⌘ keys (⌘C, ⌘A, ⌘B in an editor) reach the page as usual.
