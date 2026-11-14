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
- [ ] Pull out `abromux-core` and `abromux-term`, one module group per PR.
- [x] CI guard: fail if `cfg(target_os` / `objc::` / `msg_send!` appears outside
      `crates/abromux-platform` and `crates/abromux-browser/src/backend`.
      A local script, `scripts/check-os-calls.sh`, since there's no CI.

**Done when:** the macOS app is unchanged, CI is green, and the guard passes.

### Phase 1: Linux builds and the tests pass

- [ ] `Cargo.toml`: `gpui` gets the `wayland` feature (plus `x11` as the
      fallback) on Linux; gpui-component's `webview` and `lb-wry` become
      macOS-only dependencies; `objc` and `block` move to `abromux-platform`
      as macOS-only.
- [ ] `scripts/build-libghostty.sh`: pick the Zig download from `uname -sm`
      (`x86_64-linux`, `aarch64-linux`, `aarch64-macos`).
- [ ] Linux platform stubs that compile and do nothing; cmux import stays
      macOS-only (cmux doesn't exist on Linux).
- [ ] CI: an `ubuntu-latest` job (fmt, clippy, test) and an
      `archlinux` container job for the build Omarchy will use. System
      packages: `wayland`, `libxkbcommon`, `vulkan-icd-loader`, `fontconfig`,
      `freetype2`, `clang` (bindgen), plus the `-dev` equivalents on Ubuntu.
- [ ] `docs/guide.md`: how to build on Linux.

**Done when:** `cargo test` passes on Linux and `abromux --help` and
`abromux bench-vt` run there.

### Phase 2: a usable terminal app on Omarchy

**Window and chrome (Hyprland)**
- [ ] Set `app_id` to `dev.abromux.app` (`dev.abromux.nightly` for nightly),
      so Hyprland window rules can match it.
- [ ] Client-side decorations off by default: in a tiling compositor the
      window has no title bar; the sidebar and tab strip start at the top.
      When the window is floating, draw a slim strip with close, maximise and
      minimise, using GPUI's `Decorations::Client { tiling }` to know which.
- [ ] `WindowBackgroundAppearance::Opaque` instead of `Blurred` (Hyprland does
      its own blur when the user wants it).
- [ ] Remember the window's size only; Wayland doesn't let an app place itself.
- [ ] Fractional scaling (Omarchy laptops are often at 1.25 or 1.5): check
      that terminal cell metrics and `sprite.rs` box drawing line up.

**Shortcuts**
- [ ] Per-OS default tables in `abromux-core::keymap`. Linux mapping:
  - Terminal-context actions that would clash with the shell use
    `ctrl-shift-*`: new tab `ctrl-shift-t`, close `ctrl-shift-w`, copy and
    paste `ctrl-shift-c`/`v`, find `ctrl-shift-f`, palette `ctrl-shift-p`,
    switcher `ctrl-shift-k`.
  - App actions with no clash stay `ctrl-*`: settings `ctrl-comma`,
    reload config `ctrl-shift-comma`.
  - `alt-*` bindings stay as they are; nothing binds `super-*`.
  - Omarchy's universal copy and paste (Super+C/V, sent to apps as
    `ctrl-insert`/`shift-insert`) must work in terminals.
- [ ] `terminal.rs`: `m.platform` means Super on Linux. Make "should the app
      keep this key" ask the keymap instead of checking the platform modifier.
- [ ] The palette, menus and `abromux` CLI help print `Ctrl+Shift+P`, not `⌘⇧P`.
- [ ] Config-file shortcuts accept `cmd-` on macOS and `ctrl-`/`super-` on Linux; `secondary-` means either.

**Paths and channel**
- [ ] Config: `$XDG_CONFIG_HOME/abromux/` (already the case).
- [ ] Session, history, folders and caches: `$XDG_DATA_HOME/abromux[-nightly]/`.
- [ ] Control socket and holder socket: `$XDG_RUNTIME_DIR/abromux[-nightly]/`.
- [ ] Prefs and logs: `$XDG_STATE_HOME/abromux[-nightly]/`.
- [ ] Channel set at build time (`ABROMUX_CHANNEL=nightly`), since there's no
      Info.plist.

**Desktop services**
- [ ] Notifications: `org.freedesktop.Notifications` via `notify-rust`
      (shown by Mako on Omarchy), with a `default` action so clicking jumps to
      the terminal, like banner clicks today.
- [ ] Unread badge: `com.canonical.Unity.LauncherEntry` over D-Bus (best
      effort; nothing shows it on Omarchy, but it costs little).
- [ ] Tray bell: StatusNotifierItem via `ksni`; Omarchy's Waybar shows it
      once `tray` is in the bar's modules. Off by default on Linux.
- [ ] Appearance: `org.freedesktop.appearance color-scheme` from
      xdg-desktop-portal, watched for changes, so switching Omarchy's theme
      flips the app. `app.appearance: light|dark` overrides it.
- [ ] Power (App Nap): nothing to do.
- [ ] Opener: `xdg-open`.
- [ ] Menus: Wayland has no global menu bar, so every menu item is reachable
      from the palette, plus a menu button in the sidebar header.

**Terminal**
- [ ] Shell: use `$SHELL`, else the user's shell from `getpwuid`. Omarchy's
      default is bash.
- [ ] Shell integration for bash (OSC 133 prompt marks and the `codex`
      wrapper), next to the existing zsh one in `src/shell/`. Ghostty's bash
      integration is the model.
- [ ] Fonts: honour the Ghostty config (Omarchy ships one); otherwise use
      fontconfig's `monospace`, which Omarchy sets to a Nerd Font.
- [ ] Clipboard and primary selection (middle-click paste) through GPUI.
- [ ] `ABROMUX_SNAPSHOT` / `scripts/drive.sh`: GPUI-side render, or skip on Linux.

**Done when:** on a fresh Omarchy install, someone can install it, open
workspaces, split panes, run Claude Code and Codex with Running/Needs input in
the sidebar, get Mako notifications, quit, and get their shells back when it
starts again, all with shortcuts that don't clash with Hyprland or the shell.

### Phase 3: browser tabs on Linux, without a built-in browser

- [ ] `browser.engine` gains `system`: opening a browser tab or ⌘/Ctrl-clicking
      a link hands the URL to the default browser. It works on both OSes.
- [ ] Linux default is `terminalBrowser` when it's installed, else `system`.
      `webkit` (the built-in browser) is a config error on Linux, with a clear
      message.
- [ ] Linked pages (Jira, merge requests in `links.rs`) follow the engine:
      a terminal-browser tab, or an external open.
- [ ] Markdown tabs (`abromux-md://`) on Linux: render in a GPUI view
      (gpui-component already has a Markdown renderer), or open in the
      system browser. Decide during the phase.
- [ ] `abromux browser snapshot|click|fill|cookies|find|...` answer
      `"not supported with browser engine <x>"` instead of failing silently.
- [ ] `docs/guide.md` lists which browser features need the built-in engine.

**Later (not planned):** a built-in Linux engine as another `PageBackend`,
most likely WPE WebKit drawn offscreen into a GPUI texture, which works on
Wayland. Nothing in the core changes for it.

### Phase 4: packaging and updates

- [ ] An AUR package `abromux-bin` (PKGBUILD) with a `.desktop` file, a hicolor icon
      and `abromux` on `PATH`. This is how Omarchy users install things.
- [ ] A tarball and an AppImage on GitHub releases for other distributions.
- [ ] Updater: when installed from the AUR, only tell the user there's an
      update (pacman owns the files); AppImage replaces itself; macOS keeps the dmg path.
- [ ] No Flatpak: its sandbox would hide the host shells and agents this app runs.
- [ ] `scripts/nightly.sh` and `scripts/perf.py` on Linux, so performance can be compared there too.
- [ ] README: Linux badge, install steps, which features are macOS-only.

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
