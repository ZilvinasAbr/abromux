# Linux support plan

Goal: AbroMux runs on Linux as a native Wayland app, with **Omarchy** (Arch +
Hyprland) as the reference desktop, while the codebase splits into a
platform-free core and thin per-OS parts. macOS keeps working exactly as it
does today at every step.

## Decisions

| Question | Decision |
|---|---|
| Display server | **Wayland only** to target and test. GPUI's X11 backend stays compiled in as a free fallback, but X11 isn't tested or supported. |
| Reference desktop | **Omarchy** (Hyprland, Waybar, Mako, walker). If it works there, it ships. |
| Built-in browser on Linux | **Out of scope for now.** Browser tabs use `terminalBrowser`, or the new `system` engine (the default browser). |
| Shortcuts on Linux | Linux terminal conventions: `ctrl-shift-*` for anything that would steal a key from the shell, plain `ctrl-*` where it's safe, and never `super-*` (Hyprland owns it). |
| Architecture rule | `cfg(target_os)`, `objc` and other OS calls appear **only** in `abromux-platform` and the browser backends. CI checks this. |

## Where the code stands

No code is gated by OS today; it all assumes macOS. Most of it is portable
already:

- **Portable as is:** libghostty-vt (`src/ghostty`), the PTY and holder
  (`pty.rs`, `hold.rs`: POSIX `setsid`, `SCM_RIGHTS`, `poll`), the control
  socket and CLI (`ipc.rs`, `server.rs`, `cli.rs`), agent hooks, session,
  layouts, links, folders, status, notification hooks, OSC 99, settings
  (already under `$XDG_CONFIG_HOME`), the GitLab plugin, and the GPUI UI
  itself.
- **macOS-only:**

| Area | Files | Uses |
|---|---|---|
| Browser pane | `browser.rs`, `browser_native.{rs,m}`, `cursor.{rs,m}`, `webauthn.{rs,m,js}`, `downloads.rs` | WKWebView through wry plus about 40 `msg_send!` calls |
| Notifications | `banner.rs`, `menubar.rs` | UserNotifications, the dock badge, NSStatusItem |
| Dialogs and prefs | `alert.rs`, `quit.rs` | NSAlert, NSUserDefaults |
| System | `activity.rs`, `theme.rs`, `icon.rs`, `titlebar.rs`, `debug.rs`, `menu.rs` | App Nap, NSAppearance, the dock icon, traffic lights, window snapshots, the global menu bar |
| Paths and channel | `channel.rs`, `cmux.rs`, `agent.rs` (`/Applications/AbroMux.app`), `browser.rs` (Safari version) | Info.plist, `~/Library/Application Support` |
| Updater | `update.rs` | `.dmg`, `codesign`, swapping the `.app` |
| Shortcuts | `keymap.rs` (106 `cmd-*` bindings), `terminal.rs` (`m.platform`) | ⌘ |
| Build | `build.rs`, `scripts/build-libghostty.sh`, `Cargo.toml` | ObjC compiled against AppKit/WebKit, Zig hardcoded to `aarch64-macos`, GPUI built with `default-features = false` (no Wayland) |

## Target structure

```
crates/
  abromux-core/       Pure Rust: no gpui, no OS calls. Unit-tested on both OSes.
                      ipc protocol, settings model + parsing, session model,
                      layout (cmux.json), agent-hook normalising, notify hooks,
                      osc99, status, links, naming, link detection, keymap
                      tables (per OS), cmux session parsing.
  abromux-term/       libghostty-vt wrapper, pty, hold. POSIX: mac + linux.
  abromux-platform/   Every OS service, behind one API:
    src/lib.rs          Notifier, Tray, Prefs, Power, Appearance, Opener,
                        Dirs, Channel, Updater, WindowChrome
    src/macos/          today's ObjC (+ its own build.rs and .m files)
    src/linux/          freedesktop / Wayland implementations
  abromux-browser/    The browser pane's UI and automation (address bar,
                      history, find, injected JS, downloads list) over a
                      `PageBackend` trait:
    backend/wkwebview.rs   macOS
    backend/terminal.rs    terminal-browser, every OS
    backend/system.rs      the default browser, every OS
  abromux-plugin/     (unchanged)
  abromux-gitlab/     (unchanged)
src/                  The app: main.rs, workspace, sidebar, palette,
                      settings window, menus. Talks to the OS only through
                      abromux-platform.
```

Dependency direction: `app → browser, platform, term, core, plugins`;
`browser → platform, core`; `platform → core`; `term → core`. `core` depends
on nothing of ours.

### Platform API (first cut)

```rust
pub trait Notifier { fn post(&self, n: Banner); fn set_badge(&self, unread: usize); }
pub trait Tray     { fn set(&self, unread: usize, items: Vec<TrayItem>); }   // the menu-bar bell
pub trait Power    { fn busy(&self) -> Guard; }                             // App Nap; no-op on Linux
pub trait Opener   { fn open(&self, target: &OsStr) -> io::Result<()>; }   // open / xdg-open
pub struct Dirs    { config, data, state, runtime, cache }                   // per OS
pub struct Channel { name, app_id, support_suffix }                          // stable / nightly
pub trait Updater  { fn check(..); fn install(..); }                         // dmg / AppImage
pub mod appearance { fn system_is_dark() -> bool; fn watch(..); }
pub mod chrome     { fn window_options(..) -> WindowOptions; fn titlebar(..) -> AnyElement; }
pub struct Prefs   // JSON file in Dirs::state; replaces NSUserDefaults on both OSes
```

Confirmation dialogs (`alert.rs`) become a GPUI modal used on both OSes, so
they need no platform code at all.

## Phases

Each phase lands as several small PRs. macOS has to behave the same after
every one (`scripts/perf.py run` doesn't regress, nightly checked by hand).

### Phase 0: refactor on macOS, no behaviour change

- [ ] ~~Add CI (GitHub Actions, macOS): `cargo fmt --check`, `clippy -D warnings`, `cargo test`.~~
      Skipped for now: no CI; the checks run locally.
- [x] Create `abromux-platform`; move `browser_native.m`, `cursor.m`,
      `webauthn.m` and their `cc` builds and framework links into its
      `build.rs`, so the root `build.rs` only does the version and libghostty.
- [x] Move `banner`, `menubar`, `activity`, `icon`, `titlebar`, the
      appearance half of `theme`, and the snapshot half of `debug` behind the
      platform API.
- [x] Replace NSUserDefaults (`alert.rs`, `quit.rs`) with `Prefs` (JSON). Copy
      existing "don't ask again" values over once.
- [x] Turn NSAlert sheets into a GPUI modal.
- [x] `Dirs` and `Channel`: `channel::support_dir()` and `app_name()` move
      into the platform crate; drop the hardcoded `/Applications/...` path
      (`agent.rs`) in favour of `current_exe()`. (`agent.rs` already used
      `current_exe()`; the path there is test data.)
- [x] `Opener` replaces `Command::new("open")` (`browser.rs`).
- [x] Introduce `PageBackend`: every WKWebView call in `browser.rs` goes
      behind it (eval in frame, cookies, zoom, interaction state, snapshot,
      loading, stop, inspector, hover, downloads, focus check, user agent).
      Move the pane into `abromux-browser`, with `wkwebview.rs` as its first
      backend; fold `terminal_browser.rs` in as the `terminal` backend.
      Done differently: `abromux-browser` has the trait, `backend/wkwebview.rs`,
      automation, find, history and downloads; the pane's view stays in
      `src/browser.rs` until what it draws with (theme, find bar) leaves the
      app. `terminal_browser.rs` stays as it is: it's a terminal app in a
      terminal tab, not a page under the pane, so it's left for Phase 3's
      engines.
- [x] Pull out `abromux-core` and `abromux-term`, one module group per PR.
      Settings and the keymap tables stay in the app for now: settings
      parsing checks shortcuts with gpui's keystroke parser against gpui
      actions, and holds gpui colours. They move with Phase 2's per-OS
      keymap tables.
- [x] CI guard: fail if `cfg(target_os` / `objc::` / `msg_send!` appears outside
      `crates/abromux-platform` and `crates/abromux-browser/src/backend`.
      A local script, `scripts/check-os-calls.sh`, since there's no CI.

**Done when:** the macOS app is unchanged, CI is green, and the guard passes.

### Phase 1: Linux builds and the tests pass

- [x] `Cargo.toml`: `gpui` gets the `wayland` feature (plus `x11` as the
      fallback) on Linux; gpui-component's `webview` and `lb-wry` become
      macOS-only dependencies; `objc` and `block` move to `abromux-platform`
      as macOS-only. (`objc` and `block` were already gone from the app.)
      Also `[patch.crates-io] xattr` (`patches/`): libc 0.2.191 dropped the
      Linux `ENOATTR` that gpui's `xattr` 0.2 needs.
- [x] `scripts/build-libghostty.sh`: pick the Zig download from `uname -sm`
      (`x86_64-linux`, `aarch64-linux`, `aarch64-macos`).
- [x] Linux platform stubs that compile and do nothing; cmux import stays
      macOS-only (cmux doesn't exist on Linux). `opener` (`xdg-open`),
      `process::cwd` (`/proc`) and the support folder
      (`$XDG_DATA_HOME/abromux`) are real already. The browser has a `none`
      backend that says there's no built-in browser. cmux import compiles
      and finds nothing on Linux, so it needed no gate.
- [ ] CI: an `ubuntu-latest` job (fmt, clippy, test) and an
      `archlinux` container job for the build Omarchy will use. System
      packages: `wayland`, `libxkbcommon`, `vulkan-icd-loader`, `fontconfig`,
      `freetype2`, `clang` (bindgen), plus the `-dev` equivalents on Ubuntu.
- [x] `docs/guide.md`: how to build on Linux, and how to run it headless
      (sway + lavapipe + grim) on a server.
- [x] Keymap tests compare against gpui's spelling for this OS (`super`
      for `cmd` on Linux) instead of macOS's.

**Done when:** `cargo test` passes on Linux and `abromux --help` and
`abromux bench-vt` run there.

### Phase 2: a usable terminal app on Omarchy

**Window and chrome (Hyprland)**
- [x] Set `app_id` to `dev.abromux.app` (`dev.abromux.nightly` for nightly),
      so Hyprland window rules can match it (`channel::app_id()`, the same
      ids as the macOS bundle).
- [x] Server-side decorations: the window has no title bar of its own, and
      the sidebar and tab strip start at the top. Hyprland and sway draw
      none; the empty titlebar areas move the window with
      `start_window_move` and a double click maximizes. No traffic-light
      gap on Linux.
- [ ] Deferred (not Omarchy): on compositors without server-side
      decorations (GNOME), gpui falls back to client-side ones and the app
      should draw resize edges and close / maximise / minimise when floating
      (`Decorations::Client { tiling }`).
- [x] `WindowBackgroundAppearance::Opaque` instead of `Blurred` (Hyprland does
      its own blur when the user wants it).
- [x] Remember the window's size only: nothing to change. On Wayland gpui
      reports the origin as 0,0 and can't place the window, so only the
      size comes back.
- [x] Fractional scaling: at 1.25 and 1.5 (sway, headless), the terminal
      cells and `sprite.rs` box drawing line up with no gaps between rows.

**Shortcuts**
- [x] Per-OS default tables: `keymap::LINUX` next to `ACTIONS` (still in the
      app, not `abromux-core`: the actions are gpui's), picked by
      `Act::defaults()` through `abromux_platform::MACOS`. Tests check it
      covers every action in the same contexts, uses no Super, leaves plain
      Ctrl-letters to the shell (outside the browser), and takes no key twice
      or from `PTY_KEYS`. Linux mapping:
  - Terminal-context actions that would clash with the shell use
    `ctrl-shift-*`: new tab `ctrl-shift-t`, close `ctrl-shift-w`, copy and
    paste `ctrl-shift-c`/`v`, find `ctrl-shift-f`, palette `ctrl-shift-p`,
    switcher `ctrl-shift-s` (not `ctrl-shift-k`: that's Resize Up, as on
    macOS), splits `ctrl-shift-o`/`e` as in Ghostty.
  - Less common app actions are `ctrl-alt-<the macOS letter>`; notifications
    `ctrl-alt-i` (`ctrl-shift-i` goes to the terminal), jump to unread
    `ctrl-alt-j` (input methods take `ctrl-shift-u`).
  - App actions with no clash stay `ctrl-*`: settings `ctrl-comma`,
    reload config `ctrl-shift-comma`; font size `ctrl-=`/`-`/`0`; workspaces
    `alt-1..9`, tabs `ctrl-1..9`, links `ctrl-alt-1..9`.
  - Nothing binds `super-*`.
  - Omarchy's universal copy and paste (Super+C/V, sent to apps as
    `ctrl-insert`/`shift-insert`) work in terminals (checked with
    `wl-copy`).
- [x] `terminal.rs`: gpui dispatches bound keys before the terminal's key
      handler, so bound ctrl-shift keys reach the app and unbound ones the
      shell; nothing to change there. Link clicks and hover use
      `secondary()` (⌘ on macOS, Ctrl on Linux).
- [x] The palette, tooltips, hint pills and `abromux config shortcuts` print
      `Ctrl+Shift+P`, not `⌘⇧P` (`palette::key_label`,
      `keymap::default_label`). Hints show while the digit keys' modifier is
      held (Alt for workspaces on Linux). The CLI help fills in each OS's
      keys too (`{key:<action>}` in `HELP`).
- [x] Config-file shortcuts: gpui already reads `secondary-` as ⌘ on macOS
      and Ctrl elsewhere, and `cmd-`/`super-` as Super on Linux.

**Paths and channel**
- [x] Config: `$XDG_CONFIG_HOME/abromux/` (already the case).
- [x] Session, history, folders and caches: `$XDG_DATA_HOME/abromux[-nightly]/`
      (Phase 1).
- [x] Control socket and holder socket: `$XDG_RUNTIME_DIR/abromux[-nightly]/`
      (`dirs::runtime_dir`; the support folder without `XDG_RUNTIME_DIR`).
- [x] Prefs and logs: `$XDG_STATE_HOME/abromux[-nightly]/` (`dirs::state_dir`:
      `prefs.json`, `update.json`, `update.log`). On macOS both are the
      support folder, as before.
- [x] Channel set at build time (`ABROMUX_CHANNEL=nightly` or
      `nightly-<name>`), since there's no Info.plist; a bundle's Info.plist
      still wins on macOS.

**Desktop services**
- [x] Notifications: `org.freedesktop.Notifications` via `notify-rust`
      (shown by Mako on Omarchy), with a `default` action so clicking jumps to
      the terminal, like banner clicks today. notify-rust is pinned to 4.18,
      the last on zbus 5, which gpui's portal code already uses. The click
      also brings the window forward through Hyprland's or sway's IPC
      (`hyprctl dispatch focuswindow` / `swaymsg focus`, by `app_id`):
      Wayland only lets an app raise itself with an activation token, which
      the notification server sends but notify-rust doesn't pass on.
- [x] Unread badge: `com.canonical.Unity.LauncherEntry` `Update` signals
      over D-Bus for `<app_id>.desktop` (best effort; nothing shows it on
      Omarchy).
- [x] Tray bell: StatusNotifierItem via `ksni` (on zbus 5 and async-io, no
      tokio), with the macOS bell's menu and a drawn bell icon (icon theme
      names differ). Omarchy's Waybar shows it once `tray` is in the bar's
      modules. Off by default on Linux (`notifications.showInMenuBar`).
- [x] Appearance: gpui already follows xdg-desktop-portal's
      `org.freedesktop.appearance color-scheme` on Linux and reports changes
      as window appearance changes. `app.appearance: light|dark` overrides
      it (`theme::resolve`), since there's no app-wide appearance to force
      as on macOS.
- [x] Power (App Nap): nothing to do.
- [x] Opener: `xdg-open` (Phase 1).
- [x] Menus: Wayland has no global menu bar, so the menu-only items (About
      and the Help menu) are in the palette too, on both OSes. Hide, Hide
      Others and Minimize are macOS's and keep their keys there only. No
      menu button in the sidebar: everything is in the palette.

**Terminal**
- [x] Shell: use `$SHELL`, else the user's shell from `getpwuid`, else
      `/bin/sh` (`abromux_core::paths::user_shell`). Omarchy's default is
      bash.
- [x] Shell integration for bash (OSC 133 prompt marks and the `codex`
      wrapper), next to the zsh one in `src/shell/`, started as Ghostty does:
      `bash --posix -l` with `ENV` pointing at the script, which leaves POSIX
      mode and reads the login startup files. `redraw=1`, as for zsh: a test
      squeezing a pane shows at most 2 prompt copies with it, 5 without.
- [x] Fonts: the Ghostty config's `font-family` (Omarchy ships one) as
      before; without one, fontconfig's `monospace` (`fc-match`), and its
      `sans-serif` for the UI (`abromux_platform::fonts`).
- [x] Primary selection: a selection becomes the primary selection
      (`copy-on-select = true`, Ghostty's default) and a middle click pastes
      it (`abromux_platform::primary`). gpui sets it with the last key
      press's serial, so a selection made before any key press since launch
      isn't taken by the compositor. The clipboard already worked.
- [x] `ABROMUX_SNAPSHOT` / `scripts/drive.sh`: skipped on Linux; the window
      calls answer "not supported on Linux".

**Agents:** nothing Linux-specific. `abromux hooks install` writes the same
dot-folder configs (`~/.claude`, `~/.codex`, ...), and the hook command
reports to the app as on macOS: checked by feeding Claude Code's
`SessionStart`, `UserPromptSubmit`, `Notification` and `Stop` payloads to
`abromux hooks claude-event` in an app terminal, which showed Running,
Needs input and then nothing in the sidebar.

**Done when:** on a fresh Omarchy install, someone can install it, open
workspaces, split panes, run Claude Code and Codex with Running/Needs input in
the sidebar, get Mako notifications, quit, and get their shells back when it
starts again, all with shortcuts that don't clash with Hyprland or the shell.

### Phase 3: browser tabs on Linux, without a built-in browser

- [x] `browser.engine` gains `system`: opening a page (⌘/Ctrl-clicking a
      link, `url` commands, tracked links, `new-surface --type browser
      --url`) hands the URL to the default browser instead of making a tab.
      It works on both OSes. New Browser Tab still makes a tab whose address
      bar opens what's typed in the default browser; restored and layout
      tabs show their address and an Open in browser button, and open
      nothing by themselves (the `none` page backend).
- [x] Linux default is `terminalBrowser` when it's installed, else `system`
      (the setting is now optional; `settings::browser_engine`). `builtIn`
      is a config error on Linux, with a clear message, and the Settings
      window doesn't offer it there.
- [x] Linked pages (Jira, merge requests in `links.rs`) follow the engine:
      a terminal-browser tab, or an external open.
- [x] Markdown tabs (`abromux-md://`) on Linux: rendered once to an HTML
      file in the runtime folder (`markdown::export`, a `<base>` at the
      file's folder, no scripts) and opened in terminal-browser or the
      default browser. Decided against a GPUI renderer for now: one look
      for Markdown everywhere, and links and images work. No live reload.
- [x] `abromux browser snapshot|click|fill|cookies|find|...` answer that
      they need the built-in browser, which only macOS has.
- [x] `docs/guide.md` lists which browser features need the built-in engine.

**Later (not planned):** a built-in Linux engine as another `PageBackend`,
most likely WPE WebKit drawn offscreen into a GPUI texture, which works on
Wayland. Nothing in the core changes for it.

### Phase 4: packaging and updates

- [x] An AUR package `abromux-bin` (`packaging/aur/abromux-bin/PKGBUILD`)
      with a `.desktop` file, a hicolor icon and `abromux` on `PATH`, from
      the release tarball. This is how Omarchy users install things.
- [x] A tarball and an AppImage on GitHub releases for other distributions:
      `scripts/package-linux.sh --dist` (from a release tag; appimagetool is
      fetched into `vendor/`). The binary needs the glibc it was built
      against, so release builds belong on the oldest distribution they
      should support.
- [x] Updater: an AppImage downloads its release AppImage, checks the size
      and SHA-256 (GitHub's asset digest), replaces itself and relaunches;
      packages (`/usr`, `/opt`) and other copies only say there's an update
      and how to get it; macOS keeps the dmg path. Linux checks on its own
      only for an AppImage or a release build.
- [x] No Flatpak: its sandbox would hide the host shells and agents this app runs.
- [x] `scripts/nightly.sh` (through `package-linux.sh --nightly`) and
      `scripts/perf.py` on Linux (memory from `/proc/<pid>/smaps_rollup`'s
      PSS; no `caffeinate`), so performance can be compared there too.
- [x] README: Linux badge, install steps, which features are macOS-only.

## Not on Linux (for now)

The built-in browser (WKWebView) and everything that needs it: page
automation, cookies, find in page, the inspector, downloads, passkeys
(WebAuthn). Also cmux session import, and the dock (only a best-effort badge).

## Risks

| Risk | Mitigation |
|---|---|
| GPUI 0.2.2 Wayland quirks on Hyprland (IME, fractional scale, focus) | Test on Omarchy from Phase 1; GPUI is pinned, so changes are deliberate; report or patch upstream. |
| Shortcut clashes (shell, Hyprland, Omarchy's Super bindings) | Per-OS tables, nothing on `super-*`, a shortcut review on Omarchy before release. |
| The Phase 0 refactor breaks macOS | Small PRs, CI from the first one, the nightly checked by hand and `perf.py` after each one. |
| libghostty-vt on Linux | Ghostty itself is built for Linux; only the Zig download needed changing. Check it early in Phase 1. |
| Tray on desktops other than Omarchy (GNOME) | Off by default; in-app notifications cover it. |
