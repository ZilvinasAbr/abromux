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
- **Coding agents** (Claude Code, Codex, Gemini CLI, Droid, Cursor CLI, OpenCode and Pi) show up in the sidebar once you've run `abromux hooks install`: a "Running" pill while the agent works, "Needs input" when it asks for permission (Cursor's hooks can't say when it asks) and "Done" when it has finished. When it asks, or finishes a turn, its terminal gets a notification ("Codex: Asks to run …", "Claude Code: <the start of its reply>"), shown on the agent's pill rather than as a separate line under the workspace. A Claude Code turn that ends with work still running that will wake it again (a background command, a monitor, a scheduled wakeup or cron job, a background subagent) isn't the end: the pill stays "Running" and nothing notifies, so an agent polling for something ("no new PRs, checking again…") stays quiet until it's done. `notifications.agentTurnComplete` changes that (see [Notifications](#notifications)). Their tabs show the agent's logo (Claude Code and Codex) or a robot.
- **Notifications** come from terminal programs (OSC 9, OSC 99 and OSC 777, as Claude Code, Codex and others send them), from the bell, and from `abromux notify`. One you aren't looking at rings its pane, puts a dot on its tab and on its workspace, shows its text under the workspace in the sidebar, counts on the dock icon, and, while AbroMux is in the background, shows a macOS banner that takes you to it when clicked (banners need AbroMux.app; a `cargo run` build has the rest). On Linux the banner goes to the desktop's notification server (the Omarchy shell's on Omarchy, else Mako, dunst, ...); clicking it shows the terminal and, on Hyprland and sway, brings the window forward. It's read once you look at that terminal. ⌘I lists them all; ⌘⇧U jumps to the newest unread one. ⌥⌘U marks the terminal in front of you unread (or read), a workspace's right-click menu (or ⌘⇧P → Mark Workspace as Unread, or Read) the whole workspace, and ⌃⌘U sends its notification to the back of the queue and jumps to the next one, to work through a pile one at a time. A bell in the macOS menu bar shows the unread count and lists them; on Linux it's a tray icon (StatusNotifierItem, shown in the Omarchy shell's tray drawer or Waybar's `tray` module), off unless `"notifications": { "showInMenuBar": true }`. [Notification hooks](#notifications) can filter or change each one.
- **Program status**: a program can say what it's doing with the [Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status) (OSC 7501): idle, working (with a percentage), done, blocked on the user (approval, an answer or a sign-in) or failed, with a message. The terminal's state shows as a pill under its workspace ("cargo · Working 40%", "terraform · Needs approval"), named by the program's `app`, and turning blocked, done or failed notifies like an agent's hooks do (the message is the notification's text). Working and blocked end when the program exits or the shell's next prompt starts (OSC 133 A); done and failed stay until you type in that terminal. Of several records (a program reporting per region, say), the pill shows the one that most needs you. ConEmu's progress, OSC 9;4, maps onto it for programs that only send that, until one sends OSC 7501. A terminal running a coding agent with abromux's hooks (Claude Code sends OSC 9;4 too) shows only the agent's pill and notifications, not a second set for its program status. A program can ask whether it's supported (`OSC 7501 ; ?`), and `abromux list-pane-surfaces --json` and the `program.status` event give each terminal's status.
- **Links** in terminal output underline while you hold ⌘ over them: URLs, OSC 8 hyperlinks, and paths to files that exist (relative ones from the shell's directory, `path:line` too). ⌘-click opens web links in a browser to the right of the terminal (a new tab there if that pane already shows one), Markdown files the same way as Markdown tabs, and anything else with its default app.
- **A merge requests panel** on the right (⌥⌘B) lists the workspace's, yours and the ones you opened lately; they're kept cached and fresh in the background, so any of them opens at once in a GitLab tab (see "GitLab merge requests").
- **Panes** split right or down, and each pane has a tab bar of terminal and browser tabs. Drag a divider to resize. Drag a tab to reorder it, onto another pane's tab bar or middle to move it there, onto a pane's edge to split that pane, or onto the outer edge of the pane area to split the whole workspace (full width or height), or onto a workspace in the sidebar to move it into that workspace. A highlight shows where it will land. Move Tab to New Workspace (⌘⇧P) gives the focused tab a workspace of its own.

- **GitLab merge requests** open in a tab that looks like GitLab's own review pages (see "GitLab reviews"): the list, the overview with its threads, commits, and the changes with a file tree, inline or side-by-side diffs, suggestions and image diffs. You can comment, reply, resolve threads, start threads on lines, approve, edit and delete your comments, edit, close or merge the merge request, and retry pipelines.

## Build from source

Requirements:

- macOS on Apple Silicon, or Linux on x86_64 or aarch64 (see [Linux](#linux))
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

### Linux

Linux runs on Wayland, with Omarchy as the reference desktop ([docs/linux-plan.md](linux-plan.md)): notifications, the tray bell, the system appearance and Linux shortcuts (Ctrl+Shift, nothing on Super) work. There's no built-in browser: browser tabs use [terminal-browser](https://terminal-browser.sh) when it's installed, else your default browser. Packages first:

```bash
# Arch / Omarchy
sudo pacman -S --needed base-devel clang pkgconf wayland libxkbcommon fontconfig freetype2 vulkan-icd-loader
# Ubuntu / Debian
sudo apt install build-essential clang libclang-dev pkg-config libwayland-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libfontconfig-dev libfreetype-dev libvulkan-dev libx11-dev libx11-xcb-dev libxcb1-dev
```

Then the same `./scripts/build-libghostty.sh` (it fetches the Zig for this machine) and `cargo run --release`. The app needs a Vulkan driver (Mesa's, or `lavapipe` for software rendering) and finds Wayland through `WAYLAND_DISPLAY`. The window has no title bar of its own where the compositor draws none (Hyprland, sway); on GNOME, which leaves decorations to apps, it draws resize edges and minimize, maximize and close buttons at its top right (`ABROMUX_CLIENT_DECORATIONS=1` asks for them on any compositor). Its files follow the XDG folders: the session, history and caches in `$XDG_DATA_HOME/abromux` (`~/.local/share/abromux`), the control socket in `$XDG_RUNTIME_DIR/abromux`, prefs, logs and the server's socket in `$XDG_STATE_HOME/abromux` (`~/.local/state/abromux`), and the config in `$XDG_CONFIG_HOME/abromux` as on macOS. A nightly is a build with `ABROMUX_CHANNEL=nightly` (or `nightly-<name>`) set, and uses `abromux-nightly` (`abromux-nightly-<name>`) folders.

To install it as a desktop app, `scripts/package-linux.sh` builds a release binary and installs it into `~/.local` (`--prefix` for another): `bin/abromux`, the desktop entry `dev.abromux.app.desktop` (named after the window's Wayland `app_id`, so docks and launchers match them) and the icons. `scripts/nightly.sh` does the same for `abromux-nightly`, with its own app id, icon and folders. `scripts/package-linux.sh --dist`, from a release tag, makes the tarball and AppImage for the GitHub release, and `packaging/aur/` has the AUR package.

To try it without a desktop, as on a server, run it in a headless sway with software Vulkan, type into it through a uinput keyboard, and take screenshots with `grim` (as root; `noop` is libseat's backend for that):

```bash
sudo apt install sway grim ydotool wl-clipboard mesa-vulkan-drivers   # or pacman -S sway grim ydotool wl-clipboard vulkan-swrast
export XDG_RUNTIME_DIR=$(mktemp -d) YDOTOOL_SOCKET=/tmp/ydotool.sock
ydotoold --socket-path=$YDOTOOL_SOCKET &                # a keyboard sway reads like a real one
WLR_BACKENDS=headless,libinput LIBSEAT_BACKEND=noop WLR_RENDERER=pixman sway &
export WAYLAND_DISPLAY=wayland-1 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
cargo run &
ydotool type 'ls'; ydotool key 28:1 28:0                # evdev codes: 28 is Enter, 29 Ctrl, 42 Shift, 56 Alt
grim shot.png
```

Use a uinput keyboard rather than `wtype`: each `wtype` run adds a virtual keyboard to the seat and removes it again, and gpui loses keys around that (and can crash starting up while one is half set up), which a real keyboard never does.

### Nightly build

To try the working tree as an app without replacing the release AbroMux.app:

```bash
scripts/nightly.sh                 # builds /Applications/AbroMux Nightly.app (or pass another folder)
abromux-nightly list-workspaces    # its CLI, symlinked into ~/.cargo/bin
scripts/nightly.sh --name zoom     # "AbroMux Nightly zoom.app" and `abromux-nightly-zoom`
```

A named nightly (bundle id `dev.abromux.nightly.<name>`, support folder `abromux-nightly-<name>/`) runs beside the release and the shared nightly, so agents testing different branches don't replace each other's build.

A nightly others can download: commit, then `scripts/bundle-app.sh --nightly --dist` builds a notarized `dist/AbroMux-Nightly-<version>.dmg`, published as a GitHub pre-release tagged `nightly-<commit>`. Pre-releases are never GitHub's "latest" release, so the release app's update check doesn't offer them.

AbroMux Nightly runs next to AbroMux. It has a neon icon (`assets/app/abromux-nightly.svg`), its own name, the bundle id `dev.abromux.nightly` (so separate notification permissions and settings), and its own socket and session in `~/Library/Application Support/abromux-nightly/`. It's signed like the release but never notarized: `scripts/bundle-app.sh --nightly` is what `nightly.sh` calls, and it refuses `--dist`. The binary decides which one it is at runtime from the bundle it sits in (`crates/abromux-platform/src/channel.rs`); `cargo run` counts as the release. Install agent hooks from the release `abromux`, not `abromux-nightly`: hooks point at the binary you install them from, and do nothing once that app is gone.

## Updates

AbroMux checks for updates like cmux does (cmux uses Sparkle; AbroMux does the same steps itself). A few seconds after launch, and once a day after that, it looks at the latest GitHub release. When there's a newer version, a pill appears at the foot of the sidebar (or in the titlebar when the sidebar is hidden): **Update Available: 0.13.0**. Click it for the version, size, release date and a link to the release notes, and:

- **Install and Relaunch** downloads the release's DMG (the pill shows the percentage), checks that the app in it is signed by the same Developer ID team as the one you're running, swaps it in for the current app and relaunches. Shells keep running across the relaunch, as they do across any quit.
- **Later** hides the pill until the next check.
- **Skip** stops offering that version. A check you start yourself still shows it.

Nothing downloads until you choose Install and Relaunch. **Check for Updates…** (in the AbroMux menu, the command palette, or Settings › App › Updates) checks right away and also tells you when you're already on the latest version. The palette's **Apply Update (If Available)** installs a found update, or checks if there isn't one yet. Turn the daily check off with `"app": { "automaticallyCheckForUpdates": false }` or the switch in Settings.

On Linux the check is the same, and what Install does depends on how AbroMux was installed: an AppImage downloads the release's `AbroMux-<version>-<arch>.AppImage`, checks its size and SHA-256 against the release, replaces itself and relaunches; the AUR package says to update it with your AUR helper (pacman owns its files); a tarball or other copy says to download the release. Linux checks run on their own only for an AppImage or a release build, not for one built from a commit.

The check time and a skipped version are kept in `update.json` in the support folder (the state folder on Linux), and each step is written to `update.log` there. An app running from the disk image or from a folder you can't write to says so and won't install; move it into Applications first. Nightly builds only check: they come from `scripts/nightly.sh`, not from releases. To try the flow from a nightly, launch it with `ABROMUX_UPDATE_FROM=0.1.0` so it thinks it's on an old version (`ABROMUX_UPDATE_FEED` points the check at another releases API URL). `cargo test update::tests::live -- --ignored` downloads, verifies and swaps the real latest release in a scratch folder.

## GitLab reviews

A GitLab tab (⌘⇧P → New GitLab Tab) shows merge requests the way GitLab's web UI does, in its light or dark theme as the app's appearance is (Settings › App › Appearance). It's built in as a plugin (`crates/abromux-gitlab`, the `gitlab` Cargo feature, on by default) and writes what a reviewer and an author do: comments, replies, resolving, threads on lines, approvals, editing and deleting your comments, editing the title and description, draft, close and reopen, merging, and running, retrying or cancelling pipelines.

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
- **Keys**, as in GitLab: `j`/`]` and `k`/`[` for the next and previous file, `n` and `p` for the next and previous unresolved thread (also the arrows next to the thread count), `v` to mark the file you're reading Viewed and go on to the next one not viewed yet (the file you're on is outlined, its Viewed ringed; unmark with the checkbox), and `/` or `s` to search. While a text field has the cursor (the file tree's search, a comment), these letters are typed into it instead.
- **Managing the merge request**, as on GitLab's page: Edit right of the title opens its title and description in fields at the top of the Overview (Save changes, or ⌘Enter); the ⋮ beside it has Mark as draft or Mark as ready, Close merge request or Reopen merge request, and Copy reference. On your own comments a pencil edits the comment in place and a bin deletes it, after asking. ⌘Enter sends any box (comment, reply, line thread, edit).
- **Merging**: when GitLab says you may merge (`user.can_merge`), the merge box has Merge with "Delete source branch" and "Squash commits" ticked as the author asked (the facts under it follow the ticks). While the pipeline is still going it's "Set to auto-merge" instead, and once set the box says who set it, with Cancel auto-merge. Merging says which head it means, so a push in between makes GitLab refuse rather than merge something unseen.
- **Your review**, as GitLab's: "Start a review" from a line, a reply or the overview's comment box (the button by Comment, Reply and "Add comment now"; "Add to review" once one is under way) holds the comment in your review. Pending comments show where they'll go (on their line, in the thread they reply to, in the activity) marked Pending, with their own Edit and Delete. The Your review button in the tab bar counts them and opens GitLab's panel: Comment, Approve or Request changes, an optional summary, Submit review or Continue review, and the pending comments, each with where it goes.
- **Suggestions**: a suggestion has GitLab's Apply suggestion (its dropdown takes a commit message, GitLab's own when left empty) and Add suggestion to batch; with a batch, Apply suggestions applies them in one commit. Right after a commit GitLab refuses suggestions for a moment while it updates the merge request; the tab waits that out instead of failing.
- **Writing comments**: every comment box (the overview's, a reply, a line's, editing a comment or the description) has GitLab's Write and Preview, and its toolbar: bold, italic, strikethrough, quote, code, link, bulleted and numbered lists, a checklist, a table, and the paperclip that uploads files and images to the project and puts their markdown in. A comment on a diff line also has Insert suggestion, which writes the suggestion block with the line(s) to edit. The overview's Comment has GitLab's dropdown: a comment, or Start thread for one that can be resolved.
- **Comments on several lines**: shift-click another line's comment button while writing one, or pick the first line in the form's "Commenting on lines +12 to +15" dropdown; the lines are marked, Insert suggestion takes them all, and the thread says "Comment on lines …" as on GitLab. The file header's comment button comments on the whole file (GitLab's file comments); those threads show under the header.
- **The header**, as GitLab 19's: Code (the commands to check out the branch, Open in Web IDE, and the Patches and Plain diff downloads), Edit, the to-do (Add a to-do item / Mark as done), the notifications bell, and the ⋮ menu, which also has Lock discussion. Edit has the title, description, target branch and the merge options (delete the source branch, squash commits).
- **The sidebar's extras**: ↻ by a reviewer who has reviewed asks them for another review; Time tracking's + logs time spent (with a summary) or sets the estimate.
- **Task lists** in the description tick and untick with a click, saved to the description.
- **Merging** has "Edit commit message" (the merge commit's and, squashing, the squashed one's, filled in as GitLab would write them). A merged merge request has Revert and Cherry-pick: onto a branch, or (ticked by default) into a new branch with a merge request, as GitLab's "Start a new merge request with these changes".
- **Changes' Whitespace** toggle is GitLab's "Show whitespace changes": off, lines that differ only in whitespace show as unchanged and hunks with nothing else go, with threads kept on their lines.
- **New merge request**, on a project's list: source and target branches picked from the project's as you type (the default branch as target), a title from the branch when none is typed, the description, and Mark as draft, Delete source branch and Squash commits. Creating it opens it.
- **Filtering the list** takes GitLab's tokens: `author:@alice`, `assignee:@bob`, `reviewer:@bob`, `label:~backend` (`~"two words"`), `milestone:%v1.0`, `draft:yes`, `target:main`, `source:fix-`, each negated with a leading `-`, and `none` or `any`; with words for the title, reference, author and branches. It narrows the loaded list as you type; the row of tokens under the search puts one in.
- **Reports**, as GitLab's: once the head pipeline has finished, the Overview has the Code Quality widget ("Code Quality scans found 2 new findings", each finding's place opening its line on Changes) and the Test summary ("1 failed, 6 total tests", the failed tests with View details for their trace), and the Reports tab lists the reports on the left with the one picked on the right: each test suite's duration and counts, the failed tests, the findings.
- **Comments on images**: a click on the new image of an image diff starts a thread at that point, in the image's own pixels; threads show under the image, with GitLab's numbered badges on their points.
- **On a paid GitLab** (Premium, Ultimate), where the server has them: approval rules under the approval line (each with its approvals, "Requires 1 approval from Backend."), Merge as Add to merge train with the train's state and Remove from merge train, and "Depends on N merge requests being merged" with adding (`!12`) and removing dependencies. A free GitLab answers 404 to these, in the page's first batch of requests, and nothing extra shows.
- **Internal notes**: "Make this an internal note" under the comment box makes a comment, or a thread, that only the project's members see; internal notes show GitLab's "Internal note" badge on its warm background.
- **GitLab Duo**, where the server has it (an Enterprise GitLab with Duo): "Request review from GitLab Duo" under the reviewers, "Summarize my review with GitLab Duo" in the Your review panel, "Generate commit message" in the merge widget, "Summarize code changes with GitLab Duo" in a new merge request's description, and Ask GitLab Duo (⋮) for questions about the merge request. Duo writes in the background and its answer goes into the box it's for. Without Duo none of this shows.
- **More of GitLab's page**: the merge widget's merge checks; `/` quick actions with their list, comment templates and Text style in the editor; Create issue to resolve thread; the sidebar's Work items (Closes, Related, Mentions; + to link or create one); Add previously merged commits on Commits; each pipeline's stages, failed jobs with Retry and Download artifacts; a file's Show options (View file, Copy link to the file, Hide comments); on the list, Bulk edit, Export as CSV, the RSS feed, recent searches and pages (Show 20 / 50 / 100 items, or all); and in every comment box, Switch to rich text editing, which shows the text formatted beside the box as you type. [`gitlab-parity.md`](gitlab-parity.md) lists every control of GitLab's page and how the tab's was checked.
- **Report abuse** in the ⋮ menu opens GitLab's own page for it: GitLab has no API for abuse reports.
- **Pipelines**: Retry on a failed or canceled one, Cancel on one still going, and Run pipeline above them.
- **The sidebar** edits as GitLab's does: Edit on Assignee, Reviewer and Labels opens a dropdown with a search, ticking people or labels, saved when it closes (a click outside, Edit again, or Esc); on GitLab's free tier, which takes one assignee and one reviewer, a pick replaces and saves at once. Milestone picks one of the project's open milestones. "assign yourself" when nobody is, and a label's × removes it. Label and milestone changes show in the activity as GitLab words them.
- **Big merge requests** show every file. GitLab's diff stops at 1000 files and leaves out files over 200 KB; the rest are listed from GitLab's stats with their line counts, and each one's old and new text is fetched and diffed here when it scrolls into view (a few at a time), with a Retry if that fails. Lines longer than 5000 characters (minified files) are cut short, saying by how much. The file tree only draws the rows in view, so a thousand files scroll as smoothly as ten.
- **Selecting text**: drag over a description or comment, or over code (diff lines, the overview's snippets, code blocks, suggestions), and ⌘C copies it. In the diff a selection keeps to its side, side by side, and one longer than the screen copies whole. Shift-click extends it; a click elsewhere drops it.
- **It stays current**: the merge requests panel's cache checks the one on screen every 20 seconds and loads it again in place when it changed (see "GitLab merge requests"), and the tab itself fetches it again every two minutes (and on Refresh) unless that just happened, without losing your place, so new comments and pushes show up as on GitLab's page.
- **The list** has GitLab's state tabs with their counts and its sort ("Created date", "Updated date", "Title", "Popularity", either way round).
- **Projects**: the Projects tab lists the projects you're a member of, most recently active first, with each one's open merge requests; typing searches GitLab's projects. Pick one for its merge requests (with the state tabs, filter and sort), and "All projects" to go back.
- **Search** (`/` or `s` anywhere in the tab, as on GitLab's pages): a search over the page finds merge requests on GitLab as you type, across every project you can see; `!12` finds those numbered 12 in the projects at hand, `group/project!12` that one, and a pasted link opens. With nothing typed it shows the list's merge requests. ↑ ↓ pick, Enter opens, Esc closes.
- **Opening links**: ⌘⇧P → **Open GitLab Merge Request…** asks for a link (one already copied is filled in) and opens it in a new tab. a merge request's link pasted into the list's search box (or ⌘V anywhere on the tab) opens it; a project's merge requests link (`…/group/project/-/merge_requests`) lists them. ⌘-clicking either kind of link in a terminal, opening one from a browser tab (terminal-browser's new-tab links too, with `terminalBrowserPanes`), or `abromux new-surface --type browser --url` with one, opens it in a GitLab tab where the browser would have gone (in a GitLab pane to the right, if there is one). A link clicked in a GitLab tab opens in a new tab in that tab's pane.
- **Speed**: what a page needs is fetched together, through one curl that runs the requests side by side over connections it keeps open (one multiplexed connection on HTTP/2), so a GitLab far away costs a round trip or two instead of one per request. The list shows as soon as the merge requests come, and approvals, threads, pipelines and branches fill in after. A merge request loaded before (in this tab or another, by the panel's background cache, or before a restart: what GitLab answered is kept on disk) shows its last copy at once while the fresh one loads, and so does a list. Members load a hundred at a time; the people pickers ask GitLab for the rest as you type.
- **Opening one directly**: `abromux rpc surface.create '{"type":"gitlab","url":"https://gitlab.example.com/group/project/-/merge_requests/12"}'`. A tab comes back after a restart on the same page, with its Viewed marks.

In a narrow pane the file tree and sidebar step aside; the button left of "Compare" brings the tree back. Developing the plugin against a GitLab in Docker is described in [`dev/gitlab/README.md`](../dev/gitlab/README.md).

### Local diffs

`abromux diff [<range>]` shows any git diff in the GitLab tab, as if it were a merge request, with no GitLab: like `hunk`, but in the review UI. `X...Y` diffs Y against where it forked from X, `X..Y` the two commits, `X` X against the working tree, and nothing the uncommitted changes (untracked files too). `--merge-base X` (as `git diff` has it) diffs the working tree against where it forked from X, so a branch's commits and what isn't committed yet, and `--since-fork` does that for the default branch (`origin/HEAD`, else `main` or `master`). ⌥⌘D (File › Review Changes, or the palette) opens that last one for the focused tab's repository without typing anything, or for each checkout of a workspace folder when the focused tab isn't in one; on the default branch itself it's the uncommitted changes. It opens beside the terminal it's run in (`--direction`, or `--pane` for a tab there); `--cwd` picks the repository and `--title` names it. The tab is answered from git and a small file of comment threads in `local-diffs/` under Application Support (`crates/abromux-gitlab/src/local.rs`), so Changes, Commits, expanding unchanged lines, threads on lines, replies, resolving and "Start a review" all work as for a merge request. The same diff asked for again reopens the same review and tab, and the tab reloads by itself when the threads, the revisions or the working tree change.

Comments go to a coding agent instead of GitLab. A new thread (or a submitted review's threads, together, or a reply) writes a prompt with the repository, the diff, the code around the line and the thread, and starts the agent in a terminal beside the tab: `--agent '<command>'` (default `claude`, or `$ABROMUX_DIFF_AGENT`; the prompt is appended, or put where `{prompt}` or `{prompt_file}` is). With `--send-here` (or `--send-to surface:N`), comments go to that terminal's agent as one line pointing at the prompt instead. The agent makes the change or answers, then replies in the thread with `abromux diff reply <id> <thread> [--resolve]`, and the reply shows under the line. `abromux diff list` lists the reviews and `abromux diff threads <id>` prints one's threads.

## Passkeys

Browser panes sign in with passkeys (iCloud Keychain, a phone over Bluetooth) and security keys, the way cmux does. WKWebView only allows WebAuthn for an app's own associated domains, so `src/webauthn.js` replaces `navigator.credentials.create/get` for public-key requests in the top frame and hands them to `crates/abromux-platform/src/macos/webauthn.m`, which runs them through AuthenticationServices' web-browser APIs with the page's origin and challenge, and passes the credential back. Conditional (autofill) requests stay pending, since there's no autofill UI; sites show their "Sign in with a passkey" button instead. Frames and U2F `appid` requests are left to WebKit.

The web-browser APIs need the restricted `com.apple.developer.web-browser.public-key-credential` entitlement, which Apple grants per App ID and only a provisioning profile can carry. Without it the bridge stays out of the way and WebKit handles WebAuthn as before (for most sites, not at all). To turn it on:

1. In the Apple Developer portal, under Certificates, Identifiers & Profiles, register explicit App IDs `dev.abromux.app` and `dev.abromux.nightly`, and request the "Web Browser Public Key Credential Requests" capability for each (a request Apple reviews).
2. Once it's granted, make a Developer ID provisioning profile for each App ID with your Developer ID Application certificate.
3. Save them as `assets/app/dev.abromux.app.provisionprofile` and `assets/app/dev.abromux.nightly.provisionprofile` (or point `$ABROMUX_PROVISION_PROFILE` at one), then run `scripts/nightly.sh` or `scripts/bundle-app.sh`. It embeds the profile and signs its entitlements in, and says "Passkeys on".

The first passkey request asks to let AbroMux use passkeys (System Settings › Privacy & Security › Passkey Access for Web Browsers); a request that may use a phone also asks for Bluetooth. If passkeys are denied, security keys still work.

## Session restore

Quitting saves the session to `~/Library/Application Support/abromux/session.json` (the layout is also saved every few seconds, so a crash loses little). The next launch restores the window frame (on the monitor it was on: the frame is kept for each arrangement of screens, so it also goes back when a monitor is plugged in again, and is fitted onto the screen that's left when one is unplugged), workspaces with their names, descriptions, colors and groups, the split layout and ratios, tabs and their names, each terminal's working directory and font size, browser pages with their back/forward history (saved on quit) and zoom, and up to 2000 lines of each terminal's output (with colors). By default the shells themselves keep running while abromux is closed and come back exactly as they were (see [Persistence](#persistence)); what follows is how terminals come back when they couldn't be kept (after a reboot) or with `"persistence": "cmux-like"`. Then shells start fresh: running programs aren't kept alive. Coding agents' sessions are the exception. Run `abromux hooks install` once and abromux learns, through the agents' hooks, which session runs in which terminal (the same hooks feed the sidebar status and notifications; rerun `install` after updating abromux to pick up new hook events). On restore, it types the agent's resume command into that terminal's new shell: `claude --resume <id>` (with the same permission mode), `codex resume <id>`, `gemini --resume <id>`, `droid --resume <id>`, `cursor-agent --resume <id>`, `opencode --session <id>` or `pi --session <id>`. A Codex, OpenCode or Pi you had quit isn't resumed. The hooks go after any hooks already there and do nothing outside abromux: `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`), `~/.codex/hooks.json` (or `$CODEX_HOME`), `~/.gemini/settings.json`, `~/.factory/hooks.json` (or the `hooks` in `~/.factory/settings.json` when there's no hooks.json) and `~/.cursor/hooks.json`. OpenCode and Pi get a plugin file instead: `~/.config/opencode/plugins/abromux.js` and `~/.pi/agent/extensions/abromux.js`. Agents whose folder doesn't exist are skipped; `abromux hooks install gemini pi` does just those. Codex asks you once to review new hooks: trust them when it does. They point at the binary you ran `install` from, so run it from the installed app; `abromux hooks uninstall` removes them. `ABROMUX_NO_RESTORE=1` skips it; `ABROMUX_SESSION_PATH` uses another file. `ABROMUX_DEMO` and `ABROMUX_SCRIPT` runs don't touch the session unless `ABROMUX_SESSION_PATH` is set.

Reopen Previous Session (⌘⇧O, File menu, or `abromux restore-session`) brings back the workspaces of the session this launch restored that aren't open any more, such as ones you closed by mistake. Workspaces still open as they were are skipped, so it never doubles one. `abromux restore-session --from nightly` (or `stable`, or a file) adds another session's workspaces, for example after trying the nightly app; `abromux restore-session --export <file> [--force]` writes the current one to a file.

### Importing from cmux

File › Import cmux Session (also in the command palette, or `abromux restore-session --from cmux`) adds cmux's workspaces. It reads the file cmux restores itself from, `~/Library/Application Support/cmux/session-com.cmuxterm.app.json`, which cmux saves every few seconds while it runs and again on quit, so it works with cmux open or closed. `--from <file>` takes any cmux session file too, such as one under `cmux/session-history/`. What comes over: every cmux window's workspaces and their names (one an agent gave is replaced as abromux renames workspaces), the splits and their ratios, each pane's tabs and selected tab, the focused pane, each terminal's directory, font size and output, browser pages with their back/forward lists, and coding agents, which resume in their terminal (`claude --resume <id>` and so on) as on any restore. cmux's own extras (markdown panes, pinned workspaces, status pills, notifications) don't. As with Reopen Previous Session, a workspace already open here as it is in cmux is skipped, so importing twice adds nothing new. Shells start fresh in abromux; the programs running in cmux's terminals keep running there. An agent still working in cmux is resumed here as well, so quit it in cmux (or quit cmux) first if you don't want both on the same session.

## Remote workspaces

A workspace can run its terminals on another machine, such as a Linux VPS, which keeps them (and the agents in them) going whether or not the app is open:

**Servers** (Settings › Servers) are those machines by name: add one with its name and address (`ssh://me@box`, `box:2222`, a Tailscale name, a host from `~/.ssh/config`, or `tailnet://box` for a server on the tailnet, below), see whether it's up with Check, remove it with ×. More addresses after the first, separated by commas, are tried in order when it doesn't answer (its LAN name, then its Tailscale name). The first Check remembers which machine answered, and says so if another one answers there later. Each server gets **New Workspace on …**, **New Tab on …** and **Open Workspaces from …** (its workspaces that aren't open here: after losing the session, or on another Mac) in ⌘⇧P. They're kept in `~/.config/abromux/servers.json`, beside `config.json` rather than in it, so other versions of the app that share the config file aren't upset by a key they don't know. From a shell, the app open or not:

```bash
abromux servers add vps ssh://me@box       # list, remove <name>, check [<name>...]
abromux new-workspace --host vps --name "VPS"
abromux new-surface --host vps             # one tab there, in any workspace (--host local: this Mac)
```

`--host` takes a server's name or an address, on `new-workspace`, `new-surface`, `new-pane` and `new-split`. A workspace or tab keeps the address, so renaming or removing the server later doesn't strand it.

Its terminals, and every tab or split opened in it later, start there in the host user's login shell, in `--cwd` if that folder exists there (else the home folder). The sidebar shows the host under the name. Quitting the app leaves them running; the next launch takes them back with the screen as it was and what they printed meanwhile, as with local terminals. A broken connection is retried in the background (a dim line in the terminal says so), and the terminal picks up where it was. Closing a tab or the workspace ends its shell there.

On the host, install the server, which needs only Rust (no Zig, no Wayland):

```bash
cargo install --git https://github.com/ZilvinasAbr/abromux abromux-server   # into ~/.cargo/bin
```

Or a package: `scripts/package-server.sh [--deb] [--vt]` makes a tarball (with an `abromux` link and a systemd unit) and a `.deb`; there's an AUR `abromux-server` too. `abromux-server install [--tailnet]` runs it as a systemd user service, so it's up from boot (with `loginctl enable-linger`).

The app reaches it with `ssh HOST abromux-server --stdio`, which starts the server when it isn't running. That needs ssh to log in without asking anything: a key in your agent, or Tailscale SSH. One ssh connection per host is shared by all its terminals (`ControlMaster`, sockets in `/tmp/abromux-ssh-<uid>`). `$ABROMUX_SSH` runs another ssh program, and `$ABROMUX_REMOTE_COMMAND` replaces what runs on the host. The server keeps its socket in `~/.local/state/abromux/server.sock`; it ends with its last terminal, so a reboot of the host ends them too. If your host ends a user's processes when they log out (`KillUserProcesses`), run `loginctl enable-linger` there.

**Agents there report here.** The shells there get the server's control socket (`ABROMUX_SOCKET_PATH`), and the app keeps a link to each server it has something on. Run `abromux-server hooks install` on the host once (or link `abromux-server` as `abromux` on its PATH, and `abromux hooks install`): Claude Code, Codex and the others show Running or Needs input, notify, and name the workspace as they do here. The whole `abromux` CLI works there too (`abromux-server` is `abromux` on a host): `notify`, `set-status`, `new-split`, `send`, `read-screen`, `browser open`, `tree` and the rest act on the app wherever it is, and `abromux new-workspace` or `abromux open <dir>` there makes a workspace on that machine. Commands that need the app on the same machine (`edit`, `config`, `markdown`, `do`) say so. While the app is closed, the server keeps those reports, and a notification a terminal prints, and the app shows them when it's back. A server the app can't reach fades its workspaces in the sidebar. A new workspace on a server goes into that server's group (⌘⇧P **Group Workspaces by Server** groups older ones), and ⌘P shows which server each workspace is on and finds them by its name. `abromux --server vps <command>` runs any command through that server.

**This Mac's server is the same.** It keeps a copy of the session too, and the app starts from it when it's newer than its own `session.json` (or that's gone). It also owns the control socket now, so `abromux` commands and agent hooks keep working while the app is closed: it answers `list-workspaces`, keeps status, progress, the log, notifications and agent reports, and the app shows them at its next launch. Another app (another Mac, or the web client) can open a terminal this app has: the app shares it, and holds it alone again once the other is gone.

**Workspace folders there.** `abromux new-workspace --host vps --folder "fix login"` makes a workspace folder on the host (as ⇧⌘I does here: `AGENTS.md`, `CLAUDE.md`, `.abromux/workspace.json`) and opens the workspace in it; `abromux folder add ~/repos/x` in its terminals makes a worktree there, and `abromux-server folder list|all|remove` the rest.

**The model.** Each server keeps the model of the app's workspaces (theirs, for a remote one) and is its source: it numbers every change, the app's included, and sets status pills, progress and workspace names itself (the app shows them), so they're right whether or not the app is open. `{"op":"model"}` on its socket gives it, and `{"op":"subscribe"}` gives it and then every change, for scripts and other clients.

**Ports.** A dev server on the host isn't on this machine: `abromux forward 3000` (in the workspace) makes `http://127.0.0.1:<port>` here reach port 3000 there, through the server. `forward list`, `forward remove 3000`, and `forward ports` (what listens there). Forwards are the workspace's: saved with it, on the same port next time, and closed with it. When something starts listening on a host (a dev server), the app says so, and ⌘⇧P **Forward Port N of <server>** forwards it.

**Tailscale and a phone.** `abromux-server --tailnet` (or `install --tailnet`) also listens on the host's Tailscale addresses, port 7297, letting in only its Tailscale user (and `--tailnet-allow` logins). An app reaches it at `tailnet://host`, no ssh needed, and a phone's browser at `http://host:7297/`: the terminals there, each in a terminal with the keys a phone keyboard lacks (`#term=<id>` opens one directly, for the Home Screen). Server-side parsing (`--vt` packages, or the app's own server) gives it the screen as it is; without it, what was printed since the last app left.

[server-plan.md](server-plan.md) has how it's built and what's left.

## Sidebar

Each workspace's row shows its name, a description if it has one, the focused tab's title, the latest unread notification, status pills, progress and the newest log line from scripts, and the git branch and directory. Right-click a row for its menu: rename, add or edit a description, pick a color, move it, put it in a group, or close it (or every other one).

- **Description**: ⌥⌘E (or right-click → Add Description…) edits it under the name. Return saves, ⇧↩ adds a line, Esc cancels, and an empty one clears it.
- **Color**: right-click → Color, from cmux's sixteen (Red, Crimson, Orange, Amber, Olive, Green, Teal, Aqua, Blue, Navy, Indigo, Purple, Magenta, Rose, Brown, Charcoal) or your own palette (`workspaceColors.colors`). It shows as a bar down the row's left edge, or fills the row with `"indicatorStyle": "solidFill"`. A layout's `"color"` sets it too.
- **Order**: drag a row to move it. A line shows where it will land, between two rows; between two of a group's workspaces puts it in that group, and above a group's header keeps it out. Dropping it on a header makes it the group's first. Right-click → Move Up / Move Down / Move to Top, or ⌘⇧P → Move Workspace Up / Down. ⌘1…⌘9 follow the order.
- **Groups**: a header above workspaces that belong together. Right-click a row → Group → New Group starts one (named after the repository, with the name selected so you can type a new one), or ⌘⇧P → Group Workspaces by Folder groups every workspace that shares a repository or folder with another. Click a header to fold it (a folded group still shows the selected workspace, and a dot if one has an unread notification), double-click to rename it, drag it to move the whole group, and use its `+` for a new workspace in the group's folder. Right-click a header to rename, color, ungroup or close it. ⌘N from a workspace in a group puts the new one in the group.
- **Auto-naming** (on by default; turn it off with `"automation": { "workspaceAutoNaming": false }`): the first coding agent to run in a workspace names it; agents started later in other terminals leave the name alone. Claude Code titles its own terminal from the session, so a Claude workspace takes that title, and follows it as Claude Code retitles. Other agents' prompts are turned into a short name by that agent's own CLI (`codex exec`, or `claude -p` on Haiku), and renamed as later prompts move the work on (very short ones like "yes" don't count). That runs a request on your account per rename. A name you give the workspace always wins and stops the renaming; clearing it (an empty rename) starts it again. A try that fails (the CLI timing out, a rate limit) is made again on the next prompt, and after three in a row only every ten minutes. Agents other than Claude Code and Codex are named by whichever of those two is installed. Without either, the name follows the agent's terminal title. A ⇧⌘I workspace left on its made-up name is auto-named this way even with the setting off; typing a name in the ⇧⌘I dialog counts as naming it.

## Workspace folders

A workspace folder is a workspace with a folder of its own, a scoped place for an agent to work in. The feature is a Lua script that comes with AbroMux, `abromux.workspace_folders` (`src/lua/workspace_folders.lua`), written on the same API as your own [scripts](#lua-scripts); the app itself only knows that a workspace can have a folder and env vars (`workspace.create`'s `folder` and `env`). Turn it on in `init.lua`:

```lua
require("abromux.workspace_folders").setup{}
-- or: setup{ root = "~/work/ws", agent = "codex", key = "cmd-shift-i", cleanup_key = false }
```

`setup{}` takes `root` (where the folders go), `agent` (the cleanup agent's command), `key` and `cleanup_key` (the two shortcuts, `false` for none), and `agents_md` / `cleanup_md` (your own text for those files). To change more, copy the file next to `init.lua`, `require` your copy, and edit it.

**New Workspace Folder…** (⇧⌘I, in the palette) asks for a name; Enter with nothing typed takes a made-up one (like `quiet-amber-falcon`), which the workspace's first agent then renames ([auto-naming](#sidebar)), and Esc cancels. The folder goes under `~/.abromux/ws/` (`~/.abromux/ws-nightly[-<name>]/` for a nightly), or `root`, or `app.workspaceFoldersDirectory` (Settings › New Workspaces › Workspace folders; `~` works); `$ABROMUX_WORKSPACES_DIR` overrides them all. `abromux new_workspace_folder name=<name>` does the same from a shell. The phone app's New Workspace runs it too. Renaming the workspace later doesn't move the folder, since shells and worktrees point into it.

The folder holds `.abromux/workspace.json` (its name and what was checked out) and an `AGENTS.md` (with a `CLAUDE.md` that imports it) telling the agent to bring projects in rather than edit them in place. Its shells get `$ABROMUX_WORKSPACE_DIR` and `COMPOSE_PROJECT_NAME=<folder>`, and the workspace keeps its folder and those across restarts.

The script's `folder` action is also a command (any `init.lua` action is: `abromux <action> …`), and works from any shell inside a workspace folder:

- `abromux folder add <path> [--branch <name>]` checks a project out into the folder: for a git repo, a worktree of the whole repo on a new branch `ws/<folder>` (or `--branch`, which may already exist if no other worktree has it); anything else is copied.
- `abromux folder` prints the folder, `folder list` its checkouts, `folder all [--closed]` every workspace folder, `folder repos` the git repos right under `~/repos`, `~/src`, `~/code`, `~/projects`, `~/dev` and `~/Developer`, and `folder root` where the folders live.

Closing the workspace keeps its folder.

### Cleaning up

**Clean Up and Delete Workspace Folder…** (⇧⌘X, in the palette) moves the workspace into the **Cleanup** group, which always stays last in the sidebar (`workspace.action` `move-to-cleanup`), writes `.abromux/cleanup.md`, and opens a tab in the folder running a cleanup agent: `claude 'Read .abromux/cleanup.md and follow it.'`. It looks for everything the workspace made outside its folder, removes those things without asking, and ends with `abromux folder remove`, which closes the workspace. Workspaces are treated as scratch folders for one task, so it only stops to ask when work would really be lost: uncommitted changes or unpushed, unmerged commits in a worktree, or a changed copy of a folder. Things that were there before or are shared with other projects are kept, and listed at the end. Claude asks before each command as usual. `agent` in `setup{}` (or `automation.workspaceCleanupAgent`) picks another agent CLI; the instruction is passed as its one argument. `abromux clean_up_workspace_folder workspace=<ref>` does it from a shell, and the phone app's Clean Up runs it with `focus=false`, which leaves the Mac's selection where it is.

To make that findable, each folder's `AGENTS.md` asks agents to keep track of what they make:

- `docker compose` stacks started in its shells are named after the folder (`$COMPOSE_PROJECT_NAME`).
- Containers, volumes and networks made with plain `docker` get `--label abromux.workspace=<folder>`.
- Anything else (a database on a shared server, a cloud bucket, a `brew services` entry, a long-running process) is written down in `.abromux/resources.md`, with the command that removes it.

`abromux folder resources` shows that list and the Docker resources it finds by label or compose project.

`abromux folder remove [<path>] [--force]` does the deleting without an agent: `git worktree remove` for each worktree in its repo, then the folder. It deletes a `ws/<folder>` branch it made if that's merged, and keeps it otherwise (a branch passed to `--branch` that already existed always stays). It refuses while a worktree has uncommitted or untracked files, or Docker resources of the workspace are still there; `--force` goes ahead anyway. If AbroMux has the workspace open, it closes it last, so it works from the workspace's own shell. `abromux folder all --closed` lists folders no open workspace has, to remove by path.

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

## Claude tabs

A Claude tab runs Claude Code as a chat page instead of a terminal: replies render as Markdown, tool calls fold into one-line cards (edits as diffs, commands with their output, todos as a checklist), and permission prompts, questions and plans are cards you answer in place.

- **File ▸ New Claude Tab** (⇧⌘T; Ctrl+Alt+Y on Linux) opens one in the focused tab's folder.
- From a terminal, `abromux claude [<dir> [<prompt>...]]` opens one as a tab in that pane (`--split right` for a pane beside it instead). It takes the model, effort and permission mode to start with, and a first message:

  ```sh
  abromux claude                                   # here
  abromux claude ~/code/app fix the flaky test     # there, and send that
  abromux claude --model opus --effort high --mode plan
  abromux claude --continue                        # this folder's latest conversation
  abromux claude --resume <session-id> --split right
  ```

  `--model` takes `opus`, `sonnet`, `haiku`, `fable`, `default`, `opus[1m]` or a full model name; `--effort` low, medium, high, xhigh or max; `--mode` ask, accept-edits, plan, auto, dont-ask or bypass. A typo fails before a tab opens. The same options work as `surface.create` parameters (`"type": "claude"`).
- It runs `claude -p` with stream-json in and out through your login shell (`$SHELL -lc`), so it finds `claude` on the same PATH a terminal would. The process starts when you focus the composer or send, and stops when the tab closes.
- ↩ sends, ⇧↩ adds a line, Esc stops Claude. With a prompt waiting and the composer empty, ↩ allows and Esc denies; sending a message instead declines the prompt and tells Claude what to do. Paste or drop images to attach them; drop any other file (from Finder, say) to mention it as `@path`. `/` lists slash commands and `@` your folder's files.
- **Permission mode** (the button under the composer, or ⇧Tab to cycle the first four): Ask permissions, Accept edits, Plan mode, Auto mode (Claude checks each action and runs the safe ones; for models that have it), Don't ask (only what's already allowed runs) and Bypass permissions (everything runs; for sandboxes). A new tab starts in Auto mode unless `--mode` says otherwise; `"claude": { "defaultMode": "ask" }` in the config file picks another (ask, accept-edits, plan, auto or dont-ask).
- **Model and effort**: the model button (it shows the model and its effort, "Opus 5.5 Medium") opens a menu of the main models, numbered: press 1–4 to pick one. **Effort** under them picks the reasoning effort (Low to Max, as the model offers), and **More models** has Default and the older and dated ones. The ring beside it shows how full the context is.
- Code in replies, diffs and files read back is colored by language (highlight.js); a ```` ```diff ```` block shows as added and removed lines.
- Subagents show their steps inside their card, and each turn that edits files ends with what it changed (+/−); click a file to jump to its edit.
- **Rewind**: hover one of your messages and click ↺ to go back to before it. Your message returns to the composer to edit, and Claude forgets what came after (the conversation forks). If Claude changed files since, you can restore them too.
- **Switch Claude Code: Terminal ↔ Claude Tab** (⌥⌘Y; Ctrl+Alt+Shift+Y on Linux; also in the command palette and File menu) swaps the focused tab between the two in place. A terminal running Claude Code becomes a Claude tab on the same conversation, in its permission mode. A Claude tab becomes a terminal that runs `claude --resume <id>` with its model, effort and mode, as a terminal would type it (with `claude.configDir` as `CLAUDE_CONFIG_DIR`). The old tab closes, ending its process, so only one side writes to the conversation. A pinned tab keeps its pin. It waits while Claude is mid-turn, and a terminal's session is known only through abromux's hooks (`abromux hooks install`).
- `/clear` starts a new conversation; `abromux claude --continue` or `--resume <id>` picks up an earlier one.
- Its sidebar pill and notifications work like a terminal agent's: Running, Needs input, Done.
- The session id, model, effort and mode are in the tab's URL, so a restored tab shows the conversation from Claude Code's transcript, picks it up with `--resume` when you write again, and keeps its settings (bypass mode excepted: a restored tab asks again).
- `"claude": { "configDir": "~/.claude-work" }` in the config file picks the Claude Code config folder (and so the account) Claude tabs use, as `CLAUDE_CONFIG_DIR` would.
- It's the built-in browser's page, so it needs macOS for now.

## Pinned tabs

A workspace keeps the tabs its work is about pinned: its Jira tickets and GitLab merge requests, and any other tab (a page, a terminal running a server or an agent). The selected workspace lists them in the sidebar, under its row: a ticket with its key (`PAY-1423`) and title, a merge request with just its title (no `Draft:` marker or ticket keys in front of it), any other tab with its icon, title and its program's status; the ones on screen are highlighted. A merge request stacked on another (its target branch is the other's source branch) is listed under it, indented. Hold ⌥⌘ to see their numbers, and ⌥⌘1…9 shows one.

Closing a pinned tab (⌘W, its ×, middle-click) hides it instead: it leaves the layout and keeps running, dimmed in the list. Showing it again (a click on its row, ⌥⌘1…9, ⌘P, or opening its link again) puts it back in the pane it was in, or the link pane if that one has closed. ⇧⌘W closes and unpins it, asking first if a command or an agent is still running in it. A hidden tab's notifications and status still show, and opening a notification puts its tab back. Closing the workspace closes its hidden tabs too.

Showing a link never makes a second tab of it: if a tab already shows that ticket or merge request (any page of it: `…/browse/PAY-1423`, a board with `selectedIssue=PAY-1423`, a merge request's `/diffs`), hidden or not, that tab comes forward. Otherwise it opens, pinned, in the workspace's link pane, the pane its pages are in (a new one to the right if there's none), a merge request in a GitLab tab. ⌘-clicking a link in a terminal, or a page opening one in a new tab, goes to the existing tab the same way.

What's listed:
- Tabs showing a ticket or merge request are pinned on their own, one tab per ticket or merge request.
- ⌥⌘T pins the focused tab, whatever it is; again unpins it. The × on a row unpins it (a hidden one closes, as with ⇧⌘W), and a ticket or merge request unpinned isn't pinned again on its own.
- Under "Suggested": its merge requests from its branches (the merge requests panel's "This workspace") that no tab shows yet, and links tracked before tabs were pinned. A click opens one in a pinned tab; its × stops suggesting it.
- From a terminal: `abromux tab pin|unpin [--surface <ref>]` and `abromux tab list`; `abromux link add <url|PAY-123|group/project!iid>` opens a link in a hidden pinned tab (unless a tab shows it already), `abromux link remove <n|key|url|surface>` unpins it, `abromux link [list]`, `abromux link open <n|key|url>` (`--focus false` leaves focus where it is), and `abromux link open --all` for every one. `abromux close-surface` hides a pinned tab; `--unpin` closes it for good. Workspace folders' `AGENTS.md` asks agents to pin what they work on this way.

⌘P shows two columns: every workspace on the left (with its ⌘1…9), and the selected workspace's tabs on the right: the pinned ones first (hidden ones too, with their ⌥⌘1…9), then its other tabs, then the suggested links. What you type filters both columns; Tab or ⇧Tab moves between them, ↑ ↓ pick a row, Enter goes there. The workspace and tab you're on are marked with a dot.

A bare key like `PAY-1423` needs to know its Jira: `"jira": { "host": "acme.atlassian.net" }` in the config file, else the one a ticket opened before is on. `"sidebar": { "showLinks": false }` hides the list. Pinned tabs, hidden ones with them, are saved with the session.

## Phone access

Control abromux on this Mac from a phone, over [Tailscale](https://tailscale.com): the workspaces in a sidebar as the Mac shows them, each terminal live with typing and keys into it, Claude tabs as the same chat, the notifications, and the notifications pushed to the phone when you're away. As in Rex, the network and who's asking are Tailscale's job, so there is no password or pairing token.

It needs [Tailscale](https://tailscale.com/download) on the Mac and on the phone, signed in to the same account, with MagicDNS and HTTPS certificates on for the tailnet (the Tailscale admin console, DNS). Then turn on **Settings › Phone Access**: abromux puts itself on the Mac's tailnet address and shows the address and a QR code to scan. If Tailscale isn't ready, the page says what's missing. Or from a terminal:

```bash
abromux remote enable     # the same, printing the URL and QR code
abromux remote            # status: URL, tailscale serve, phones connected
abromux remote pair       # the URL and QR code again
abromux remote allow someone@example.com   # another Tailscale user (`deny` takes them out)
abromux remote push test  # a notification to every phone that turned them on
abromux remote disable
```

On the phone, signed in to Tailscale as the Mac's Tailscale user: scan the code (or open the URL in Safari), Share → Add to Home Screen, and open abromux from there. Open **Notifications** at the bottom of the sidebar and tap **Turn On Alerts** to get notifications (iOS sends web apps notifications only from the Home Screen, iOS 16.4 and later).

- **The app.** The sidebar lists the workspaces as the Mac's does: groups (tap a header to fold it), colors, descriptions, the latest notification, agent pills with what they said, branch and folder. It sits on the right: swipe in from the right edge (or tap the sidebar button) to open it; on an iPad or a phone on its side it stays open. A workspace shows its tabs along the top: tap one to show it, **+** for a new Claude chat or terminal there. The **+** at the bottom of the sidebar makes a new workspace, named or not: a plain one in your home folder, or, with [workspace folders](#workspace-folders) turned on in `init.lua`, a workspace folder as ⇧⌘I makes (it runs that action with `action.run`). Either opens on the phone and leaves the Mac's selection alone. Hold a workspace in the sidebar (or tap the title at the top) for what it offers: for a workspace folder, **Clean Up and Delete Folder…**, which after you confirm runs ⇧⌘X's action with `action.run` and opens the cleanup agent's tab on the phone, again leaving the Mac's selection alone. Notifications, at the bottom of the sidebar, lists them, and turns the phone alerts on or off. It follows the phone's light or dark setting.

- **How it's reached.** The app listens on a loopback port only (`127.0.0.1:7298`; the nightly 7299). While phone access is on, the app runs `tailscale serve --bg --https=8443 http://127.0.0.1:7298` (the nightly uses 10000) when it starts, so it's at `https://<mac>.<tailnet>.ts.net:8443/` with Tailscale's certificate, and on nothing but the tailnet; turning phone access off takes that down. If that port of the tailnet address already serves something else, abromux says so and leaves it: pick another with `"httpsPort"` or `--https-port` (443, 8443 or 10000, the ports `tailscale serve` takes).
- **Who gets in.** `tailscale serve` tells abromux which tailnet user each request is from (`Tailscale-User-Login`). The Mac's own Tailscale user always gets in, and so do the logins in `allow` (Settings › Phone Access › Who Can Connect); a request that didn't come through `tailscale serve` has no user and gets nothing. WebSockets and POSTs must also come from the page itself, so another site open on the phone can't use it.
- **What a phone can do.** Workspaces, panes, tabs, terminals (reading, typing, keys), Claude tabs, notifications, and `init.lua`'s actions (`action.list`, `action.run`; no more than typing `abromux <action>` in one of its terminals could). Not browser automation (your logged-in browser), files, quitting, or agent reports, unless `"allMethods": true`.
- **Terminals.** A terminal shows at the Mac's size, in a font small enough to fit the phone's width (A− and A+ change it; past the smallest it scrolls sideways). Its screen comes first, then its output as it prints (`surface.stream`). Keys and text go through abromux, so they're sent the way the Mac's terminal expects. A web page tab offers to open its page on the phone; other tabs show on the Mac only.
- **Claude tabs.** A [Claude tab](#claude-tabs) shows on the phone as the same chat page, with the conversation so far, and stays in step with the Mac: what either side sends, answers or changes (mode, model, effort, stopping) goes to the one `claude` running on the Mac. The **+** in the composer adds photos; Return types a new line on the phone's keyboard (send with the arrow). Links open on the phone.
- **Claude Code in a terminal.** A terminal running Claude Code (known from abromux's hooks, or from the `claude` process among its shell's and the session file Claude Code keeps for it) shows on the phone as the same chat page, read from Claude Code's transcript as it's written, with a **Chat | Terminal** switch at the top (the phone remembers your pick per tab). The Mac's terminal is left as it is. What you send is pasted into the terminal and Return pressed, so it's one message even with several lines; stop presses Esc, and `/clear` follows the new conversation. Permission prompts, the model, the mode and rewinding stay in the terminal: when Claude asks something there, a bar over the chat switches to the terminal. When Claude Code ends, the tab goes back to the terminal. `surface.stream` with `claude: true` is the stream behind it; `surface.send_text` with `submit: true` is the send.
- **Notifications.** abromux's own notifications (an agent needs input or finished) are pushed to the phones that turned them on, when no phone has the app open and the Mac has had no keyboard or mouse use for a minute (`pushWhenIdle`, seconds; 0: always). `"pushPreview": false` leaves the text out. Pushes go from the Mac to the phone's push service (Apple's), encrypted for the phone (Web Push).
- **Staying awake.** While phone access is on and an agent is working, the Mac doesn't idle-sleep (`caffeinate -i`, ended when no agent is working or the app quits; `"keepAwake": false` turns it off). The display still sleeps. A closed lid still sleeps a Mac that isn't on power with an external display.

The settings are `"remote"` in the config file, beside the rest (every key optional; these are the defaults, apart from `enabled` and `allow`):

```json
"remote": {
  "enabled": true,
  "allow": ["friend@example.com"],
  "allMethods": false,
  "keepAwake": true,
  "pushWhenIdle": 60,
  "pushPreview": true,
  "port": 7298,
  "httpsPort": 8443
}
```

The release and nightly apps share the config file, so phone access is on for both; each has its own ports unless `port` and `httpsPort` say otherwise. The push keys and the phones that subscribed are in `push.json` in the support folder (`~/Library/Application Support/abromux/`; only you can read it).

## Settings window

⌘, (or the app menu › Settings…) opens Settings: a page each for General, Sidebar, Workspace Colors, Panes, Keyboard Shortcuts, Commands, GitLab and Phone Access, with a search field over all of them. It's a front end for the config file below, not a separate store. Each change writes just that key to `config.json` and applies at once. A setting put back to its default is removed from the file, and the rest of the file (other keys, their order, commands) stays as you wrote it. The undo arrow next to a setting resets it, and the one by a page's title resets the whole page.

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
  - **The server:** shells run in `abromux server` (it used to be `abromux hold`), a small background process. The app starts it on demand, one for the release and one for each nightly. It exits once it has no terminals left, and nothing needs installing. It's the same program that runs on another machine for [remote workspaces](#remote-workspaces).
  - **While abromux runs**, the server passes each PTY to the app, which reads and writes it directly. Speed, latency and CPU are the same as with `"cmux-like"`.
  - **On quit**, each terminal's exact state is saved with libghostty-vt's snapshot format, in `held/` next to the session. The server then keeps reading the shells.
  - **On the next launch**, you get the same shell back: the screen as you left it, then whatever it printed while abromux was closed (up to 16 MB per terminal).
  - **After a crash** there's no snapshot, so only the output since the crash comes back. A full-screen program redraws when the pane is resized.
  - **Closing a tab or workspace** ends its shells; quitting doesn't. `pkill -f "abromux server"` ends every held shell without opening abromux.
  - **After a reboot**, abromux falls back to the `"cmux-like"` restore. So do runs that don't restore a session (`ABROMUX_NO_RESTORE`, `ABROMUX_DEMO`, `ABROMUX_SCRIPT`), whose shells are never held. `$ABROMUX_SERVER_SOCKET` (or the old `$ABROMUX_HOLD_SOCKET`) moves the server's socket. After an update from a version that had `abromux hold`, its shells are still taken back from it.
- `"cmux-like"`: shells end with the app, as in cmux, and the next launch restores them as described in [Session restore](#session-restore).

The palette (⌘⇧P) has **Persistence: abromux** and **Persistence: cmux-like** to switch without editing the file. They write the key into `config.json` (the rest of the file stays as you wrote it) and reload. It applies to terminals opened after the change, and a reload is enough. A terminal already running in the server is taken back at the next launch, whatever the setting says.

### Browser engine

```json
{ "browser": { "engine": "terminalBrowser" } }
```

`engine` is `builtIn` (the WebKit browser pane, macOS only), `terminalBrowser` (`"terminal-browser"` works too) or `system` (your default browser); Settings has it under General › Browser. Without one, it's `builtIn` on macOS, and on Linux `terminalBrowser` if terminal-browser is installed, else `system`. With `system`, pages open in your default browser (`open`, `xdg-open`) instead of a tab: ⌘/Ctrl-clicked links, `url` commands, tracked links, `abromux new-surface --type browser --url …` (which says no tab was made) and Markdown files. New Browser Tab still makes a tab, whose address bar sends what you type there to the default browser; tabs restored from a session or made by a layout show their address and an Open in browser button, and open nothing by themselves. With `terminalBrowser`, every new browser tab (⌘⇧L, the globe, ⌘-clicked links, `url` commands, layouts, `abromux new-surface --type browser`) is a terminal running `terminal-browser open --no-merge <url>` in your login shell, and closing the browser closes the tab. The tab shows a globe and the page's title, as a built-in one does: abromux asks terminal-browser's control socket (`~/.local/state/terminal-browser-*/instances/*.sock`) for the active tab's title and URL, matched to the tab by its tty.

A terminal-browser tab takes the built-in browser's keys: ⌘L, ⌘R, ⌘[ / ⌘], ⌘= / ⌘- / ⌘0 (zoom), and ⌘A, ⌘C, ⌘X, ⌘Z, ⇧⌘Z in the page. ⌘F (find), ⌥⌘I (devtools) and ⌥⌘C (console) work as in the built-in browser: abromux sends terminal-browser the key it binds to the same command in its `shortcuts.json` (`~/.config/terminal-browser/`, or its settings with ⌃,), by default ⇧⌘F, ⇧⌘I and ⌥⌘J, read on each press. Its own keys work too, except ⇧⌘I, which is abromux's New Workspace Folder; F12 opens devtools. Window-level keys (⌘T, ⌘W, ⌘P, ⌘N, …) stay abromux's. ⌘-click opens a link in a new terminal-browser tab, the pointer follows the page (a hand over links), and the page gets key releases, focus and blur, and sideways scrolling. The Edit menu's Copy, Cut and Select All (and their keys) go to the page or its address bar, so ⌘L then ⌘C copies the URL. ⌘R on a local file (`file://`) reloads it through `terminal-browser action … reload`, since terminal-browser's own ⌘R does nothing there.

terminal-browser run in a shell (`terminal-browser open ./report.html`, as an agent does) is treated as a browser tab while it runs: abromux finds its control socket within two seconds and matches it to the pane by tty. The tab shows the page's title and a globe and takes the keys above, so ⌘R reloads and ⌘= zooms the page rather than the terminal's font. When the browser quits, the pane is a shell again, and the session saves it as the terminal it is.

abromux looks for `terminal-browser` on PATH, in `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin`; if it isn't there, the tab falls back to the built-in browser with a warning. The setting applies to tabs opened after it changes; open ones stay as they are. A terminal-browser tab is restored as a browser tab on the page it last showed (its own back/forward history isn't kept). The `abromux browser` automation commands only drive built-in tabs, so `abromux browser open` always opens one (use `terminal-browser action` to drive a terminal-browser).

`"terminalBrowserPanes": true` in `browser` (Settings: General › Browser › terminal-browser new tabs as panes) has terminal-browser open new tabs as abromux panes instead of its own tabs. Links that ask for a new tab (`target=_blank`, ⌘-click, middle-click), the context menu's "Open link in new tab" and terminal-browser's new-tab box then open a browser tab in the pane of the browser they came from (`TB_ABROMUX_SPLIT=right`, `down`, `left` or `up` in that pane's environment opens a split on that side instead). Pages opened with `window.open` and window features (OAuth and SSO sign-ins that talk to their opener) stay popups. terminal-browser has no setting for this, so abromux adds a few lines to its `app/browser/dist/main.js`, and takes them out when the setting is off. That file is outside terminal-browser's signed Electron app, so its signature, entitlements and passkeys are untouched. `terminal-browser upgrade` replaces the file; abromux puts the lines back before it next starts a terminal-browser, and if the code they hook into has moved, it changes nothing and says so. A running terminal-browser keeps the code it started with: all of them share one daemon, so a change applies once every terminal-browser has quit. That daemon's environment is the first pane's, so the new tab's place comes from each browser's own `ABROMUX_SURFACE_ID`; a browser that isn't in an abromux pane opens a normal tab. Shells get `ABROMUX_BIN`, the CLI of the app they run in, which is what the new tab is made with.

#### Without the built-in browser (Linux)

Linux has no built-in browser, so these need macOS: the `abromux browser` automation commands (snapshot, click, fill, eval, cookies, find, …, which answer that they need it), find in page, the web inspector and console, downloads, passkeys, and per-tab zoom and back/forward history. Markdown tabs (`abromux new-surface --type markdown`) are rendered once to an HTML file in `$XDG_RUNTIME_DIR/abromux/markdown/` and opened in terminal-browser or the default browser, with links and images resolving next to the file; they don't follow later edits to the file (open it again).

### Opening files

```json
{ "editor": { "program": "tode" } }
```

AbroMux can open files from Finder. It's listed in Open With for text and source files, and for the code extensions `abromux default-app list` prints (`.rs`, `.ts`, `.toml`… that macOS has no text type for). It never makes itself their default app on its own. Each file opens in a terminal running the editor `program` names, `nvim` (the default) or `tode`, in a new pane to the right of the focused one. That's in the workspace whose folder, or focused terminal's directory, holds the file most closely, else the selected one. The editor starts in the workspace's folder when the file is in it, else in the file's own folder. tode runs as `tode -n <file>`, so it opens in that pane rather than in a tode window already open. Quitting the editor closes the pane. Settings has it under General › Editor. Folders opened the same way (dropped on the Dock icon) open as workspaces. Images (PNG, JPEG, GIF, WebP, SVG, BMP, AVIF) aren't for the editor: AbroMux is in their Open With too, and each opens in a viewer tab (the Markdown viewer's) in a new pane to the right, the same as `abromux open <image>`.

To have files open in AbroMux on a double-click:

```bash
abromux default-app code        # every extension `default-app list` prints
abromux default-app rs ts py    # or just these
abromux default-app png jpg     # images, in the viewer
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
  "workspaceGroups": { "newWorkspacePlacement": "afterCurrent" },
  "automation": { "workspaceAutoNaming": true, "workspaceCleanupAgent": null }
}
```

- `appearance`: `system` follows macOS's light or dark mode as it changes; `light` or `dark` keeps the app in one (also in Settings › General). It covers the windows, sidebar, menus, Markdown tabs, web pages that follow the system's color scheme, and terminals: a Ghostty `theme = light:A,dark:B` uses the one for the appearance, and without a theme the built-in colors switch too. Running terminals recolor when it changes. GitLab tabs stay in GitLab's dark theme.
- `workspaceInheritWorkingDirectory`: ⌘N and the `+` start in the focused terminal's directory; `false` starts in your home folder.
- `keepWorkspaceOpenWhenClosingLastSurface`: ⌘W on a workspace's last tab opens a fresh terminal in the same directory instead of closing the workspace. Only ⌘W: a shell that exits, or the tab's `×`, still closes it.
- `reorderOnNotification`: a workspace that gets a notification you aren't looking at moves to the top of the sidebar, or to the top of its group if it's in one. `false` (or `"off"`) keeps the order as you left it. cmux's `"notifications"` is the same as `true`; its `"agentActivity"` is taken as `"notifications"` too, since agent turns already notify here.
- `automaticallyCheckForUpdates`: look for a new release once a day ([Updates](#updates)).
- `workspaceFoldersDirectory`: where the [workspace folders](#workspace-folders) script makes them; `null` is `~/.abromux/ws` (per channel for nightlies). The app only shows it in Settings; the script reads it.
- `newWorkspacePlacement`: where a new workspace goes, `top`, `afterCurrent` (after the selected one, or after its group) or `end`. `workspaceGroups.newWorkspacePlacement` is the same inside a group, where `top` means right after the group's first workspace.
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

`"commands"` adds your own entries to the command palette (⌘⇧P), each with an optional `shortcut`. `script` runs a Lua script (see [Lua scripts](#lua-scripts)). `run` types the text into the focused terminal and presses Return; with `"newTab": true`, or when a browser has focus, it runs in a new terminal tab instead. `url` opens a page in a new browser tab. `description` shows under the title in the palette, `keywords` are more words it finds the command by, and `"confirm": true` asks before every run.

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
    "needsInput": "banner",
    "done": "unread",
    "error": "banner",
    "agentTurnComplete": "whenIdle",
    "hooks": [
      { "id": "no-banner-for-codex", "command": "jq 'if .notification.title == \"Codex\" then .effects.desktop = false else . end'" },
      { "id": "speak", "command": "say \"$(jq -r .notification.body)\"", "timeoutSeconds": 10 }
    ]
  }
}
```

`showInMenuBar` (on by default, as in cmux) puts a bell in the macOS menu bar with the unread count. Its menu lists the newest unread notifications (click one to go to it) and has Show Notifications, Jump to Latest Unread, Mark All Read and Clear All.

`needsInput`, `done` and `error` (Settings: General › Notifications) say what a coding agent's or program's notification does by what happened: an agent waiting for you (a permission prompt, a question; a program's OSC 7501 `blocked`), an agent finishing its turn (a program's `done`), or a program failing. `banner` (the default for `needsInput` and `error`) marks it unread and shows a banner while AbroMux is in the background; `unread` (the default for `done`, since an agent finishes every turn) marks it unread with no banner or sound; `off` doesn't record it. Other notifications (OSC 9/99/777, `abromux notify`) always get a banner.

`agentTurnComplete` (cmux's key) says which of a coding agent's turns count as finishing, and so notify as `done` says (Settings: General › Notifications):

- `"whenIdle"` (the default): only once the work the agent left running has drained. A turn that ends while a background command, monitor, scheduled wakeup or cron job it started is still going says nothing, and its pill stays "Running"; the turn after the last of it finishes notifies. Background work is seen through Claude Code's hooks; other agents notify every turn.
- `"always"`: every turn, as before.
- `"never"`: no turn ever notifies (the same as `"done": "off"` for agents, while programs' `done` still notifies).

Asking for permission notifies whatever this says, and so does a `/loop` tick that schedules its next one saying it found something (`ScheduleWakeup` with `noop: false`). An agent can always ask for you itself with `abromux notify`.

`hooks` are cmux's notification hooks: shell commands each notification (not the bell) passes through, in order. A hook gets it as JSON on stdin and prints it back, changed or not:

```json
{
  "version": 1,
  "notification": { "id": 7, "workspaceId": "workspace:2", "surfaceId": "surface:5",
                    "title": "Claude Code", "subtitle": "", "body": "Needs your input",
                    "kind": "needsInput" },
  "context": { "cwd": "/path/to/project", "hookId": "speak", "appFocused": false, "focusedPanel": false },
  "effects": { "record": true, "markUnread": true, "desktop": true, "sound": true }
}
```

`kind` is `needsInput`, `done`, `error` or `message` (anything else a program or `abromux notify` sent), and `effects` start as its setting above says (`desktop` and `sound` false for `unread`). `record: false` drops the notification, `markUnread: false` records it as read, `desktop: false` skips the macOS banner and `sound: false` its sound; a changed `title`, `subtitle` or `body` is what's shown. A hook that prints nothing changes nothing, so any command can be a hook. Hooks run in the background (the banner waits for them) in the workspace's directory, with `ABROMUX_WORKSPACE_ID`, `ABROMUX_SURFACE_ID` and `ABROMUX_SOCKET_PATH` set so they can call `abromux`. If one fails, takes longer than `timeoutSeconds` (default 20) or prints something that isn't JSON, the notification goes ahead as if there were no hooks and a toast says which hook failed. `"enabled": false` turns one off. Unlike cmux, only this file's hooks run, not a project's.

## Ghostty config

Terminals read your Ghostty config: `~/.config/ghostty/config` (or `$XDG_CONFIG_HOME`), then `~/Library/Application Support/com.mitchellh.ghostty/config`, plus their `config-file` includes. abromux uses `theme` (from `~/.config/ghostty/themes` or Ghostty.app; `light:A,dark:B` follows the app's appearance), `font-family` (further `font-family` lines are fallbacks, as in Ghostty; without one, JetBrains Mono on macOS and fontconfig's `monospace` on Linux), `font-size`, `foreground`, `background`, `palette`, `cursor-color`, `cursor-text`, `cursor-style`, `selection-background`/`-foreground`, `keybind`s whose action is `text:`, `csi:` or `esc:`, and `clipboard-paste-protection` / `clipboard-paste-bracketed-safe` (on by default, as in Ghostty: pasting text that could run commands, such as a newline when the program hasn't turned on bracketed paste, asks first; Enter pastes, Esc cancels). Programs can copy to the clipboard with OSC 52, as terminal-browser, Neovim and tmux do (`clipboard-write = deny` stops them; there's no prompt, so `ask` refuses too). `copy-on-select = clipboard` copies a selection to the clipboard when you let go of the mouse. On Linux a selection also becomes the primary selection, which a middle click pastes (Ghostty's default `copy-on-select = true`; `false` turns it off); macOS has no primary selection. `mouse-scroll-multiplier` scales scrolling, as one number or `precision:<n>,discrete:<n>` for the trackpad and the mouse wheel. `scrollbar = never` hides the overlay scrollbar. `shell-integration = none` turns off abromux's shell integration (below). Other keys are ignored for now. Changes apply to new terminals.

Ghostty has Nerd Font symbols built in; abromux doesn't, so for the icons Neovim, eza or starship draw it falls back to the Nerd Fonts you have installed (symbols-only and "Mono" ones first), after your `font-family` fallbacks and before macOS's own fonts. Install one (`brew install --cask font-symbols-only-nerd-font`) if icons show as boxes.

## Shell integration

zsh and bash start with a small integration (`src/shell/zsh` and `src/shell/bash`, written to `shell-integration/` in the support folder) that marks the prompt with OSC 133 and wraps `codex`. The shell is `$SHELL`, else your login shell from the user database. bash starts as `bash --posix -l` with `ENV` pointing at the script: in POSIX mode bash reads only `$ENV`, and the script leaves POSIX mode and reads `/etc/profile` and the first of `~/.bash_profile`, `~/.bash_login` and `~/.profile`, as a login bash does (as Ghostty's integration does). Its history file stays `~/.bash_history` unless you set `HISTFILE`. abromux sets `ZDOTDIR` to that folder; its `.zshenv` puts your `ZDOTDIR` back, runs your `.zshenv`, and zsh reads the rest of your config as usual. With the marks, resizing a pane blanks the prompt for zsh to redraw instead of rewrapping it, which used to pile up pieces of the prompt line. One copy can still stay behind when a narrower pane wraps the prompt, until the pane widens again: since Ghostty `dde3d4d6b`, libghostty-vt clears the prompt after the reflow, from its last wrapped row only (Ghostty 1.3.1 cleared first). bash redraws its prompt the same way. A `zsh` or `bash` started inside a pane doesn't get the integration. Other shells have none yet.

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

Targets by name: `--workspace` takes a workspace's name and `--surface` a tab's name, set with `rename-tab` or with `--label` when it's made (`new-split`, `new-surface`, `new-pane`), case aside. A name that fits more than one is an error that lists them. Refs (`workspace:2`, `surface:3`) stay the same across restarts, also in held shells' `$ABROMUX_WORKSPACE_ID` and `$ABROMUX_SURFACE_ID`, so a script that must keep working should keep the ref a command printed rather than a name.

```bash
abromux new-split right --no-focus --label tests --command 'cargo test' --wait   # exits with cargo test's status
abromux read-screen --surface tests --lines 40
abromux send --surface tests 'cargo build\n'; abromux wait tests --timeout 600     # until that command finishes
abromux wait tests --close                  # until the tab closes; its program's exit status
abromux events                              # what happens, one line each, until ⌃C
abromux events --json --events command.finished,surface. --workspace api --for 60
abromux rpc list                            # every socket method
abromux rpc describe surface.wait           # what it does, its parameters and their JSON Schema
```

- `wait [<surface>]` waits for the command running in a terminal to finish and exits with its status (right away, with the last one's, if nothing runs). It knows commands from the shell integration's OSC 133 marks (`C` when one starts, `D;<status>` when it ends); in a terminal without them it waits for the tab to close. `--close` waits for the tab to close: when its program exits by itself, `wait` exits with its status (1 if a signal ended it). `--timeout <seconds>` gives up with status 1. `--wait` on `new-split`, `new-surface` and `new-pane` waits for the `--command` they were given.
- `events` follows the app: `workspace.created`, `.closed`, `.renamed` and `.selected`; `surface.created`, `.closed` (with the program's `exit_code` when it exited by itself), `.moved`, `.title`, `.cwd` and `.focused`; `command.finished` (with `exit_code`), `program.status`, `agent` (a coding agent's hooks: working, needs_input, stopped, ended), `notification` and `bell`. Each has `event`, `time` and the `workspace` and `surface` it concerns. `--events` takes names or prefixes ending in `.`, `--workspace` and `--surface` limit it to one, `--for <seconds>` and `--count <n>` end it. Underneath, the connection asks for `events.subscribe` and then reads one JSON object per line until it closes; structural events are found by comparing the app every 200 ms, only while something listens.
- `rpc list [-q]` and `rpc describe <method> [--json]` describe the control socket, as the running app has it (`system.methods`): each method's purpose, the CLI command that calls it, and its parameters as JSON Schema. `src/api.rs` holds the descriptions, and a test fails when a method the server handles isn't described there.
- Exit status: 0 done, 1 failed, 2 a bad command line, 3 a target not found or a name that fits more than one, 4 abromux isn't running; `wait` and `--wait` exit with the status of what they waited for. Error replies on the socket carry a `code` for these: `target`, `method_not_found`.

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

## Lua scripts

`abromux do` runs a [Lua](https://www.lua.org) 5.4 script against the app, after [Rex's `rex do`](https://www.superlogical.com/rex/docs/automate/lua-scripts). A script can do anything the CLI can, keep state in tables instead of parsing text, react to events, and ask you things in the palette. `~/.config/abromux/init.lua` adds your own palette commands written in Lua.

```lua
-- tests.lua: run a command in a split beside you; zoom it if it fails
local cmd = abromux.args.cmd or "cargo test"
local split, err = abromux.surface.split{
  direction = "right", focus = false, label = "tests",
  command = cmd,
}
if err then error(err, 0) end
local ev = abromux.wait("command.finished", { surface = split.surface.ref }, 600)
if ev == nil then error("timed out waiting for: " .. cmd, 0) end
if ev.exit_code ~= 0 then abromux.pane.zoom{ surface = split.surface.ref, state = "on" } end
return { surface = split.surface.ref, exit_code = ev.exit_code }
```

```bash
abromux do tests.lua cmd='go test ./...'     # prints {"exit_code":1,"surface":"surface:12"}
abromux do -e 'return abromux.workspace.current{}.name'
abromux do - < layout.lua                    # from stdin
abromux do greet.lua --args '{"name": "true"}'   # arguments as JSON (or --args @file.json)
```

- **Arguments** come after the script as `key=value` and are in `abromux.args`: `n=2` is a number, `on=true` a boolean, anything else a string.
- **The result:** what the script returns is printed as JSON (`--json` indents it). `error(msg, 0)` prints `abromux: msg` and exits 1; `abromux.stop(code)` ends at once with that status. `print` writes to stdout, `abromux.log([level,] text)` to stderr.
- **Calling the app:** every socket method is `abromux.<namespace>.<method>{...}` with the parameters `abromux rpc describe <method>` lists (`abromux.surface.split{...}`, `abromux.status.set{...}`, `abromux.browser.snapshot{...}`), and `abromux.call(method, params)` calls one by name. A failed call doesn't raise: it returns `nil` and the error, so check the second value when it matters. A script run in an abromux terminal acts on that terminal by default, like the CLI; `abromux.surface_id` and `abromux.workspace_id` say which.
- **Commands:** `abromux.exec(argv[, opts])` runs a program with its arguments as a list, no shell (`{ "sh", "-c", line }` for one), and returns `code`, `ok`, `stdout`, `stderr`, `signal` and `timed_out`. A non-zero exit is still a result; `nil, err` means it couldn't start. Options: `cwd`, `env` (`false` removes a variable), `clear_env`, `stdin` (text to send), `timeout` (seconds, then SIGTERM, and SIGKILL 2 s later), `stdout`/`stderr` (`false` discards it), and `detach` (start it and return its `pid` at once).
- **Waiting:** `abromux.sleep(seconds)`, and `abromux.wait(event[, filter][, timeout])`, which returns the first such event (and a `ctx` with its `workspace` and `surface`), or `nil, "timeout"`. `filter` is a table of fields the event must have, or a function given the event that returns `true`. Events are the ones `abromux events` prints; the script hears them from the moment it starts, so one that happened before `wait` was called still counts.
- **Handlers and timers:** `abromux.on(event, fn)` calls `fn(ctx, ev)` for each such event (a name, a prefix ending in `.` or `*`, or `*` for all) while the script keeps running. `abromux.every(seconds, fn)` and `abromux.after(seconds, fn)` call `fn` later and return a timer with `cancel()` and `active()`. A script with handlers or timers keeps running after its last line until it calls `abromux.stop`, `--for 10m` passes, abromux quits, or you press ⌃C. Calls inside a handler act on the event's tab. Only one runs at a time, and an error in one is printed and the script carries on (`--fail-fast` ends it instead). Handlers hear the script's own workspace; `--all` hears every one. At most 1,024 timers at once.
- **Questions:** `abromux.ui.input{ title = "New branch", value = "feature/", placeholder = "…", confirm = "Create" }` shows a text field in the command palette and returns what was typed. `abromux.ui.pick{ title = "Open project", items = { … } }` shows a list and returns the chosen item's `id`; an item is a string or `{ id, title?, subtitle?, detail? }`, and with `allow_other = true` typed text that matches none comes back as `{ text = "…" }`. Both return `nil` and a reason when there's no answer: `cancelled`, `timeout` (`timeout`, default 300 s), `busy` (another question is showing) or `no_client`. Any client can ask the same way, with the socket method `ui.prompt`.
- **JSON:** `abromux.json.encode(value[, { indent = true }])` and `abromux.json.decode(text)`. A table with keys 1…n is an array, any other (an empty one too) an object; `null` becomes `nil`.
- `require` loads Lua files next to the script.

### Actions in init.lua

`~/.config/abromux/init.lua` (next to `config.json`) defines actions: each one is a command in the palette, gets a shortcut with `key`, and runs from the CLI with `abromux do <name>`.

⌘⇧P → Open init.lua (also in the AbroMux menu) opens it in your editor (General › Editor: nvim or tode) in a pane to the right, first making one with a commented-out starter if there's none. Give it a shortcut with `openInitLua` in `shortcuts`.

```lua
abromux.action{
  name  = "new_branch",                -- lowercase letters, digits and _
  title = "Git: New Branch…",          -- the palette's entry
  key   = "cmd-alt-b",                 -- optional; as a shortcut in config.json
  description = "Make a branch from the focused terminal's directory",
  keywords = { "git", "switch" },
  args  = { name = "string?" },        -- string, number, integer, boolean, object, array, any; ? for optional
  run   = function(ctx, args)
    local name = args.name or abromux.ui.input{ title = "New branch", value = "feature/" }
    if name == nil then return end
    abromux.surface.send_text{ text = "git switch -c " .. name .. "\n" }
  end,
}
```

```bash
abromux do new_branch name=fix-login      # without the question
abromux do --list                         # the actions it defines
abromux do ~/draft.lua --action new_branch   # try one from another file
```

The app reads init.lua when it starts and on Reload Config (⌘⇧,), with nothing able to call out yet (calls return an error, `exec` and `os.exit` aren't there) and two seconds to finish; an error in it shows like one in `config.json`. Choosing an action runs `abromux do init.lua --action <name>` in the background, acting on the focused tab: `ctx` has its `surface` and `workspace` and `origin` (`palette`, or `cli`). If it fails, a toast shows the error. `abromux.invoke(name, args)` runs another action and returns its result, or `nil` and the error.

An action is also a command of its own: `abromux <name> [words…] [key=value…] [--key value]`, for a name the CLI doesn't have, runs it with the words as `args[1]`, `args[2]`… and `--key value` (or a bare `--flag`, as `true`) as named args; `abromux do <name>` takes words the same way. `palette = false` keeps such a command out of the palette. That's how [workspace folders](#workspace-folders)' `abromux folder add <path>` works.

Modules that come with AbroMux load with `require` too: `require("abromux.workspace_folders").setup{}` turns on [workspace folders](#workspace-folders). Scripts also get `abromux.config_path`, `abromux.workspaces_dir` (the channel's default for workspace folders) and `abromux.json.array(t)` (a table that encodes as an array even when empty).

A command in `config.json` (or a project's file) can run any script: `{ "title": "Start Ticket", "script": "scripts/ticket.lua", "args": { "board": "PAY" } }`, with `"action"` to pick one of its actions. A relative path is the config folder's, or the project's for a project command, which asks before it runs as other project commands do.

## Keyboard and mouse

| | Mouse | Keys |
|---|---|---|
| New workspace | `+` in sidebar header / footer | ⌘N |
| New workspace folder ([the script](#workspace-folders) in `init.lua`) | ⌘⇧P → New Workspace Folder… | ⇧⌘I |
| Review every repository of the workspace in [hunk](https://hunk.dev/): a column right of the focused pane, one pane per repository (its folder's checkouts and those its tabs are in) running `hunk diff --watch` against where the branch forked from the default branch, as ⌥⌘D compares | File › Review Changes in Hunk | ⌘⇧P → Review Changes in Hunk (`reviewInHunk` in `"shortcuts"`) |
| Open the workspace in Neovim: a new terminal tab in its folder (else the focused tab's directory) running `nvim .` | File › Open Workspace in Neovim | ⌘⇧P → Open Workspace in Neovim (`openInNeovim` in `"shortcuts"`) |
| Open the workspace's folder in the editor `editor.program` names (nvim or tode), in a new tab | File › Open Workspace in Editor, or a workspace's right-click menu › Open in Editor | ⌘⇧P → Open Workspace in Editor (`openInEditor` in `"shortcuts"`) |
| Clean up and delete workspace folder ([the script](#workspace-folders)) | ⌘⇧P → Clean Up and Delete Workspace Folder… | ⇧⌘X |
| Rename workspace | double-click its name (Enter saves, Esc cancels) | ⌘⇧R |
| Workspace description | right-click → Add Description… (Return saves, ⇧↩ new line) | ⌥⌘E |
| Workspace color, group, move, close others | right-click the row | |
| Reorder workspaces | drag a row (onto a group to join it) or a group header | ⌘⇧P → Move Workspace Up / Down |
| Fold a group | click its header (double-click renames, right-click for more) | ⌘⇧P → Collapse or Expand Group |
| Reopen workspaces closed since launch | | ⌘⇧O |
| Delete workspace | hover → `×` | ⌘⇧P → Close Workspace (`closeWorkspace` in `"shortcuts"`) |
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
| Switch workspace or tab (Tab moves between the columns) | | ⌘P |
| Notifications list | bell icon in the titlebar | ⌘I (Esc closes) |
| Jump to newest unread notification | click it in the list or the menu-bar bell | ⌘⇧U |
| Mark the focused terminal unread, or read | | ⌥⌘U |
| Mark a workspace unread (its focused tab), or every tab in it read | right-click the workspace › Mark as Unread / Mark as Read | ⌘⇧P → Mark Workspace as Unread, or Read (`toggleWorkspaceUnread` in `"shortcuts"`) |
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

### Keys on Linux

The table above has macOS's keys. Linux has its own defaults: nothing is on Super (the compositor's, as in Hyprland), plain Ctrl-letters stay the shell's, and the rest follows Linux terminals. The palette, tooltips and `abromux config shortcuts` show the keys of the OS they run on (`Ctrl+Shift+P`). ⌘-clicking a link is Ctrl-click.

| | Linux |
|---|---|
| Command palette / workspace switcher | Ctrl+Shift+P / Ctrl+Shift+S |
| New workspace / new tab / close tab | Ctrl+Shift+N / Ctrl+Shift+T / Ctrl+Shift+W |
| Split right / split down | Ctrl+Shift+O / Ctrl+Shift+E |
| Focus pane | Ctrl+Alt+arrows |
| Resize pane | Ctrl+Shift+H/J/K/L, as on macOS |
| Workspace 1–9 / tab 1–9 / link 1–9 | Alt+1–9 / Ctrl+1–9 / Ctrl+Alt+1–9 |
| Next / previous tab | Ctrl+PageDown / Ctrl+PageUp, or Ctrl+Tab / Ctrl+Shift+Tab |
| Next / previous workspace | Ctrl+Alt+PageDown / Ctrl+Alt+PageUp |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V, or Ctrl+Insert / Shift+Insert (Omarchy's Super+C/V send these) |
| Find, next, previous | Ctrl+Shift+F, Ctrl+Shift+G, Ctrl+Alt+G |
| Font size | Ctrl+= / Ctrl+- / Ctrl+0 |
| Toggle sidebar / notifications | Ctrl+Shift+B / Ctrl+Alt+I |
| Settings / reload config / quit | Ctrl+, / Ctrl+Shift+, / Ctrl+Shift+Q |

Most other actions are on Ctrl+Alt with the macOS letter (rename workspace Ctrl+Alt+R, review changes Ctrl+Alt+D); `abromux config shortcuts` lists them all. In the config file, `secondary-` is ⌘ on macOS and Ctrl on Linux, and `cmd-` is Super on Linux.

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

- `crates/abromux-core`: logic with no UI and no OS calls (paths, the OSC 99 scanner, the control-socket protocol, the settings model as it moves there)
- `crates/abromux-term`: `ghostty/`, the libghostty-vt bindings and a safe wrapper (`Terminal`, `Frame` snapshots, key encoding, selection); `pty.rs` and `hold.rs`, the PTYs and the app's side of the server that keeps shells running; `remote.rs`, terminals streamed from another machine's server
- `crates/abromux-server`: `abromux server` / `abromux-server`, which keeps terminals running (no UI, no libghostty: it builds on a headless Linux box); its wire format is in `abromux-core`'s `wire.rs`. The plan it's part of is [server-plan.md](server-plan.md)
- `crates/abromux-platform`: every OS service the app uses (notifications, the menu-bar bell, App Nap, appearance, the dock, window drags, prefs, the channel and its folders), with the Objective-C parts in `src/macos/`
- `crates/abromux-browser`: the browser page behind `PageBackend` (`backend/wkwebview.rs` on macOS), plus automation, find, history and downloads
- `src/terminal.rs`: PTY plumbing plus a custom GPUI `Element` that paints the grid
- `src/browser.rs`: the browser pane (address bar, toolbar, find bar); `src/webauthn.js` and `crates/abromux-platform/src/macos/webauthn.m`: the passkey bridge
- `src/terminal_browser.rs`: browser tabs that run terminal-browser (`browser.engine`)
- `src/editor.rs`: files opened from Finder or `abromux edit`, in a terminal editor (`editor.program`); `abromux default-app`
- `src/workspace.rs`: the workspace, split-tree and surface model
- `src/main.rs`: the app shell (pane tab bars, splits, actions); `src/sidebar.rs`: the sidebar's rows, groups, menus and drag to reorder
- `src/bench.rs`: `abromux bench-vt` and `abromux bench-kitty`, the headless parse benchmarks used by `scripts/perf.py`
- `src/server.rs`: the control socket's method handlers; `src/cli.rs`: the `abromux` commands; `src/ipc.rs` and `crates/abromux-core/src/ipc.rs`: the shared wire format
- `src/plugins.rs`: the plugins built in; `crates/abromux-plugin`: the interface a plugin's tab implements; `crates/abromux-gitlab`: the GitLab review tab
- `dev/gitlab/`: a GitLab in Docker with review fixtures, and scripts to drive and screenshot the plugin (see its README)

## Known POC limits

- Native webviews always draw above GPUI content. Hidden tabs and workspaces are hidden explicitly, and the app's popovers swap a page they cover for a still bitmap of it (see "Popovers over pages"); a popover the app doesn't know about (a new one) would still draw under the page.
- While a page has keyboard focus, keys with ⌘ or ⌃ go to the app first (`abx_page_shortcuts_install` in `crates/abromux-platform/src/macos/browser_native.m`): a shortcut the app handles doesn't reach the page, so a site can't use ⌘T, ⌘W, ⌘L and the like for itself. Other ⌘ keys (⌘C, ⌘A, ⌘B in an editor) reach the page as usual.
