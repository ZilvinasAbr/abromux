//! A terminal pane: a PTY running the user's shell, parsed by libghostty-vt,
//! drawn cell-by-cell with gpui.

use crate::{
    agent::Agent,
    config::{Config, CursorStyle as ConfigCursor},
    ghostty::{
        self, CursorStyle as Caret, Frame, ImageLayer, ImagePixels, MouseAction, MouseGeometry,
        Rgb, ffi,
    },
    link, osc99, sprite,
    theme::*,
};
use futures::{StreamExt, channel::mpsc};
use gpui::{prelude::*, *};
use gpui_component::input::{self, Input, InputEvent, InputState};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// UI monospace font (sidebar metadata); terminals use the Ghostty config's.
pub const FONT_FAMILY: &str = "JetBrains Mono";
const LINE_HEIGHT: f32 = 1.25;
const PADDING: f32 = 6.;

actions!(
    terminal,
    [
        Copy,
        Paste,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        ClearScrollback,
        Find,
        FindNext,
        FindPrevious,
        UseSelectionForFind,
        RestartShell
    ]
);

// Ghostty's default search colours.
const MATCH_BG: Rgb = Rgb(0xff, 0xe0, 0x82);
const MATCH_SELECTED_BG: Rgb = Rgb(0xf2, 0xa5, 0x7e);
const MATCH_FG: Rgb = Rgb(0, 0, 0);

pub enum TerminalEvent {
    TitleChanged,
    Bell,
    /// OSC 9 / 99 / 777 from the program.
    Notification {
        title: String,
        body: String,
    },
    /// ⌘-click on a link.
    OpenLink(link::Target),
    Exited,
    Focused,
    /// The shell was restarted in place (`respawn-pane`).
    Respawned,
}

/// The link under the pointer while ⌘ is held: its viewport row, columns
/// and target.
#[derive(Clone)]
struct HoverLink {
    y: u16,
    x: Range<u16>,
    target: link::Target,
}

pub struct TerminalView {
    term: ghostty::Terminal,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    focus: FocusHandle,
    /// Shared with the paint that draws it, so a paint doesn't copy it.
    frame: Rc<Frame>,
    dirty: bool,
    grid: (u16, u16),
    cell: Size<Pixels>,
    /// The cell size the program is told, in device pixels, so images it
    /// sends are sharp on Retina screens.
    cell_px: (u32, u32),
    scale: f32,
    /// Textures of the Kitty images on screen, by (generation, source rect).
    images: HashMap<(u64, [u32; 4]), Arc<RenderImage>>,
    /// Textures no longer on screen, freed at the next paint.
    stale_images: Vec<Arc<RenderImage>>,
    /// The window whose atlas holds the textures, to free them on close.
    window: Option<AnyWindowHandle>,
    /// A task will repaint when an animated image's next frame is due.
    animation_timer: bool,
    bounds: Bounds<Pixels>,
    pub title: String,
    pub cwd: String,
    selecting: Option<(u16, u16)>,
    config: Config,
    font_size: f32,
    /// ⌘K: the shell's redraw after ^L lands in scrollback, so wipe it again.
    clear_pending: bool,
    /// IME composition in progress (shown at the cursor, not yet sent).
    marked: Option<String>,
    /// Where the cursor cell is on screen, for placing the IME candidate window.
    ime_cursor: Bounds<Pixels>,
    /// Bitmask of mouse buttons held while reporting to the program.
    mouse_buttons: u8,
    /// Sub-line wheel/trackpad deltas carried between events.
    scroll_accum: f32,
    /// Whether the shell reports its cwd itself (OSC 7); if not, it's polled.
    osc7: bool,
    cwd_checked: std::time::Instant,
    /// The find bar's input, kept after closing so ⌘F brings the last search back.
    find_input: Option<Entity<InputState>>,
    find_open: bool,
    /// Match count and selected index (from the newest), for the find bar.
    find_count: (usize, Option<usize>),
    /// A task is stepping the search through scrollback.
    searching: bool,
    /// The coding agent running here, as its hooks last reported it.
    pub agent: Option<Agent>,
    osc99: osc99::Scanner,
    /// Last pointer position over the terminal, for ⌘ pressed without moving.
    pointer: Option<Point<Pixels>>,
    link: Option<HoverLink>,
    /// Set by `rename-tab`; shown instead of the program's title.
    pub custom_title: Option<String>,
    /// Text held back by paste protection until the user confirms it.
    pending_paste: Option<String>,
    /// Kept to start the shell again the same way (`respawn-pane`).
    env: Vec<(String, String)>,
    /// Bumped on respawn so the old shell's output and exit are ignored.
    generation: usize,
    /// A task will repaint once the program's hold on the screen times out.
    hold_timer: bool,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    pub fn new(cwd: Option<String>, env: Vec<(String, String)>, cx: &mut Context<Self>) -> Self {
        let (cols, rows) = (80, 24);
        let cwd = cwd.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
        let (writer, master, child) = spawn_shell(&cwd, &env, (cols, rows), 0, cx);
        let focus = cx.focus_handle();
        let config = cx.try_global::<Config>().cloned().unwrap_or_default();
        cx.on_release(Self::free_images).detach();
        Self {
            term: new_term(cols, rows, &config),
            writer,
            master,
            child,
            focus,
            frame: Rc::default(),
            dirty: true,
            grid: (cols, rows),
            cell: size(px(8.), px(16.)),
            cell_px: (0, 0),
            scale: 1.,
            images: HashMap::new(),
            stale_images: Vec::new(),
            window: None,
            animation_timer: false,
            bounds: Bounds::default(),
            title: String::new(),
            cwd,
            selecting: None,
            font_size: config.font_size,
            config,
            clear_pending: false,
            marked: None,
            ime_cursor: Bounds::default(),
            mouse_buttons: 0,
            scroll_accum: 0.,
            osc7: false,
            cwd_checked: std::time::Instant::now(),
            find_input: None,
            find_open: false,
            find_count: (0, None),
            searching: false,
            agent: None,
            osc99: Default::default(),
            pointer: None,
            link: None,
            custom_title: None,
            pending_paste: None,
            env,
            generation: 0,
            hold_timer: false,
        }
    }

    /// Free the closed terminal's image textures from its window's atlas
    /// (after this update, while the window isn't borrowed).
    fn free_images(&mut self, cx: &mut App) {
        let textures: Vec<_> = self
            .images
            .drain()
            .map(|(_, t)| t)
            .chain(self.stale_images.drain(..))
            .collect();
        if let Some(w) = self.window
            && !textures.is_empty()
        {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| {
                    for t in textures {
                        let _ = window.drop_image(t);
                    }
                });
            });
        }
    }

    /// Kill the shell (and whatever it runs) and start a fresh one in its
    /// cwd, on a clean screen, like `tmux respawn-pane -k`.
    pub fn respawn(&mut self, cx: &mut Context<Self>) {
        let cwd = self.live_cwd();
        let _ = self.child.kill();
        self.generation += 1;
        let (writer, master, child) = spawn_shell(&cwd, &self.env, self.grid, self.generation, cx);
        (self.writer, self.master, self.child) = (writer, master, child);
        self.term = new_term(self.grid.0, self.grid.1, &self.config);
        // Size the new PTY and terminal to the pane, cell pixels included.
        let (cols, rows) = std::mem::take(&mut self.grid);
        self.resize(cols, rows);
        self.cwd = cwd;
        self.osc7 = false;
        self.title.clear();
        self.agent = None;
        self.clear_pending = false;
        self.marked = None;
        self.mouse_buttons = 0;
        self.selecting = None;
        self.link = None;
        self.pending_paste = None;
        self.find_open = false;
        self.find_count = (0, None);
        self.osc99 = Default::default();
        self.dirty = true;
        cx.emit(TerminalEvent::Respawned);
        cx.emit(TerminalEvent::TitleChanged);
        cx.notify();
    }

    fn process(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let kitty = self.osc99.feed(bytes);
        self.term.write(bytes);
        if std::mem::take(&mut self.clear_pending) {
            self.term.write(b"\x1b[3J");
        }
        let fx = self.term.take_effects();
        if !fx.pty_response.is_empty() {
            let _ = self.writer.write_all(&fx.pty_response);
        }
        if fx.title_changed {
            self.title = self.term.title();
            cx.emit(TerminalEvent::TitleChanged);
        }
        if fx.pwd_changed {
            let pwd = self.term.pwd();
            if !pwd.is_empty() {
                self.osc7 = true;
                self.cwd = pwd;
                cx.emit(TerminalEvent::TitleChanged);
            }
        } else if !self.osc7 && self.cwd_checked.elapsed() > std::time::Duration::from_millis(500) {
            // Shells without OSC 7 integration (stock zsh/bash): ask macOS
            // where the shell is, at most twice a second while it's printing.
            self.cwd_checked = std::time::Instant::now();
            if let Some(cwd) = self.child.process_id().and_then(process_cwd)
                && cwd != self.cwd
            {
                self.cwd = cwd;
                cx.emit(TerminalEvent::TitleChanged);
            }
        }
        if fx.bell {
            cx.emit(TerminalEvent::Bell);
        }
        for (title, body) in fx.notifications.into_iter().chain(kitty) {
            cx.emit(TerminalEvent::Notification { title, body });
        }
        self.run_search(cx);
        self.watch_hold(cx);
        self.dirty = true;
        if self.link.is_some() {
            // Output may have moved the text under the pointer.
            self.link = None;
            self.update_link(true);
        }
        cx.notify();
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self.writer.write_all(bytes);
            let _ = self.writer.flush();
        }
    }

    fn input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.term.clear_selection();
        self.term.scroll_to_bottom();
        self.dirty = true;
        self.send(bytes);
        cx.notify();
    }

    /// Paste from the clipboard. Text that could run commands waits for a
    /// confirmation first, as with Ghostty's `clipboard-paste-protection`.
    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.config.paste_protection
            && !self
                .term
                .paste_is_safe(text, self.config.paste_bracketed_safe)
        {
            self.pending_paste = Some(text.to_string());
            cx.notify();
            return;
        }
        self.paste_now(text, cx);
    }

    fn paste_now(&mut self, text: &str, cx: &mut Context<Self>) {
        let bytes = self.term.encode_paste(text);
        self.input(&bytes, cx);
    }

    /// Answer the paste confirmation: paste the held text or drop it.
    fn confirm_paste(&mut self, paste: bool, cx: &mut Context<Self>) {
        if let Some(text) = self.pending_paste.take() {
            if paste {
                self.paste_now(&text, cx);
            }
            cx.notify();
        }
    }

    /// Files dropped on the terminal: type their paths, shell-escaped, as
    /// Ghostty does.
    fn drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let text: Vec<String> = paths
            .paths()
            .iter()
            .map(|p| shell_escape(&p.to_string_lossy()))
            .collect();
        if text.is_empty() {
            return;
        }
        window.focus(&self.focus);
        cx.emit(TerminalEvent::Focused);
        self.paste_now(&format!("{} ", text.join(" ")), cx);
    }

    pub fn copy(&self, cx: &mut App) -> bool {
        match self.term.selection_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let (cw, ch) = (
            (f32::from(self.cell.width) * self.scale).round().max(1.) as u32,
            (f32::from(self.cell.height) * self.scale).round().max(1.) as u32,
        );
        if (cols, rows) == self.grid && (cw, ch) == self.cell_px {
            return;
        }
        self.grid = (cols, rows);
        self.cell_px = (cw, ch);
        self.term.resize(cols, rows, cw, ch);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: cols * cw as u16,
            pixel_height: rows * ch as u16,
        });
        self.dirty = true;
    }

    fn frame(&mut self) -> &Rc<Frame> {
        if self.dirty {
            // Refill in place unless the last paint still holds it.
            match Rc::get_mut(&mut self.frame) {
                Some(frame) => self.term.update_frame(frame),
                None => self.frame = Rc::new(self.term.frame()),
            }
            self.dirty = false;
            self.load_images();
        }
        &self.frame
    }

    /// Make textures for the frame's images that don't have one yet; the
    /// pixels are only readable until the next write to the terminal.
    fn load_images(&mut self) {
        let mut kept = HashMap::new();
        for im in &self.frame.images {
            let key = (im.generation, im.src);
            if kept.contains_key(&key) {
                continue;
            }
            let texture = match self.images.remove(&key) {
                Some(t) => t,
                None => match self.term.image_pixels(im.id, im.src) {
                    Some(p) => render_image(p),
                    None => continue,
                },
            };
            kept.insert(key, texture);
        }
        let gone = std::mem::replace(&mut self.images, kept);
        self.stale_images.extend(gone.into_values());
    }

    fn cell_at(&self, pos: Point<Pixels>) -> (u16, u16) {
        let local = pos - self.bounds.origin - point(px(PADDING), px(PADDING));
        let x = (f32::from(local.x) / f32::from(self.cell.width))
            .floor()
            .max(0.) as u16;
        let y = (f32::from(local.y) / f32::from(self.cell.height))
            .floor()
            .max(0.) as u16;
        (
            x.min(self.grid.0.saturating_sub(1)),
            y.min(self.grid.1.saturating_sub(1)),
        )
    }

    /// Clear the screen and scrollback, like Ghostty's ⌘K: drop history and
    /// have the shell redraw its prompt (^L). No-op in full-screen programs.
    pub fn clear_scrollback(&mut self, cx: &mut Context<Self>) {
        if self.term.alt_screen() {
            return;
        }
        self.term.write(b"\x1b[3J");
        self.clear_pending = true;
        self.input(b"\x0c", cx);
    }

    /// The shell's cwd right now (OSC 7 if it reports one, else asked of macOS).
    pub fn live_cwd(&self) -> String {
        if self.osc7 {
            return self.cwd.clone();
        }
        self.child
            .process_id()
            .and_then(process_cwd)
            .unwrap_or_else(|| self.cwd.clone())
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// The last `lines` of screen and scrollback as VT text, for session
    /// restore. None while a full-screen program owns the screen.
    pub fn scrollback_vt(&self, lines: usize) -> Option<String> {
        if self.term.alt_screen() {
            return None;
        }
        let vt = self.term.screen_vt();
        let all: Vec<&str> = vt.lines().collect();
        let kept = all[all.len().saturating_sub(lines)..].join("\r\n");
        (!kept.trim().is_empty()).then_some(kept)
    }

    /// Replay saved output above the new shell's prompt, reapply zoom, and
    /// resume the agent that was running (typed ahead; the shell reads it
    /// once it starts).
    pub fn restore(
        &mut self,
        scrollback: Option<&str>,
        font_size: Option<f32>,
        agent: Option<Agent>,
        cx: &mut Context<Self>,
    ) {
        if let Some(vt) = scrollback {
            self.term.write(vt.as_bytes());
            self.term.write(b"\x1b[0m\r\n");
            self.dirty = true;
        }
        if let Some(size) = font_size {
            self.set_font_size(size, cx);
        }
        if let Some(agent) = agent
            && let Some(cmd) = agent.resume_command(&self.cwd)
        {
            self.send(format!("{cmd}\r").as_bytes());
            // Kept until its hooks report in, so a quick quit doesn't drop it.
            self.agent = Some(agent);
        }
    }

    fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.font_size = size.clamp(6., 72.);
        self.dirty = true;
        cx.notify();
    }

    /// Visible viewport text, or the whole buffer with `scrollback`;
    /// `lines` keeps only the last n lines.
    pub fn read_text(&mut self, scrollback: bool, lines: Option<usize>) -> String {
        let text = if scrollback {
            self.term.screen_text()
        } else {
            let rows: Vec<String> = self
                .frame()
                .rows
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|c| {
                            if c.text.is_empty() && !c.spacer {
                                " "
                            } else {
                                &c.text
                            }
                        })
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect();
            rows.join("\n").trim_end().to_string()
        };
        match lines {
            Some(n) => {
                let all: Vec<&str> = text.lines().collect();
                all[all.len().saturating_sub(n)..].join("\n")
            }
            None => text,
        }
    }

    /// ⌘F: show the find bar (or focus it) with its last search.
    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match &self.find_input {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
                cx.subscribe_in(
                    &input,
                    window,
                    |this, input, ev: &InputEvent, _, cx| match ev {
                        InputEvent::Change => {
                            let needle = input.read(cx).value();
                            this.term.set_search(&needle);
                            this.run_search(cx);
                        }
                        InputEvent::PressEnter { .. } => this.find_step(true, cx),
                        InputEvent::Focus => cx.emit(TerminalEvent::Focused),
                        _ => {}
                    },
                )
                .detach();
                self.find_input = Some(input.clone());
                input
            }
        };
        if !self.find_open {
            self.find_open = true;
            let needle = input.read(cx).value();
            self.term.set_search(&needle);
            self.run_search(cx);
        }
        input.update(cx, |i, cx| i.focus(window, cx));
        // Runs once the input has focus, so retyping replaces the old search.
        window.dispatch_action(Box::new(input::SelectAll), cx);
        cx.notify();
    }

    /// ⌘E: search for the selected text.
    fn find_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.term.selection_text() else {
            return;
        };
        let text = text.lines().next().unwrap_or_default().to_string();
        self.open_find(window, cx);
        if let Some(input) = &self.find_input {
            input.update(cx, |i, cx| i.set_value(text.clone(), window, cx));
        }
        self.term.set_search(&text);
        self.run_search(cx);
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        self.find_count = (0, None);
        self.term.end_search();
        self.dirty = true;
        window.focus(&self.focus);
        cx.notify();
    }

    /// ⌘G / ⇧⌘G: select the next match up (older) or down the screen.
    fn find_step(&mut self, older: bool, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        self.term.search_select(older);
        self.find_count = self.term.search_count();
        self.dirty = true;
        cx.notify();
    }

    /// While the program holds the screen (synchronized output), repaint
    /// now and then so a hold it never ends still times out and gets drawn.
    /// Show the frame each playing Kitty animation is on, and repaint when
    /// the next one is due.
    fn animate(&mut self, cx: &mut Context<Self>) {
        let Some(delay) = self.term.tick_animations() else {
            return;
        };
        self.dirty = true;
        if self.animation_timer {
            return;
        }
        self.animation_timer = true;
        cx.spawn(async move |this, cx| {
            // No faster than a 120 Hz display draws.
            let delay = delay.max(std::time::Duration::from_millis(8));
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                this.animation_timer = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn watch_hold(&mut self, cx: &mut Context<Self>) {
        if !self.term.held() || self.hold_timer {
            return;
        }
        self.hold_timer = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(ghostty::HOLD_TIMEOUT).await;
                let held = this.update(cx, |this, cx| {
                    this.dirty = true;
                    cx.notify();
                    this.hold_timer = this.term.held();
                    this.hold_timer
                });
                if !held.unwrap_or(false) {
                    return;
                }
            }
        })
        .detach();
    }

    /// Step the search until it has caught up with the terminal, a slice at
    /// a time so a long scrollback doesn't stall the UI.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        if !self.find_open || self.searching {
            return;
        }
        self.searching = true;
        cx.spawn(async move |this, cx| {
            loop {
                let done = this.update(cx, |this, cx| {
                    let done = !this.find_open || this.term.search_step();
                    this.find_count = this.term.search_count();
                    this.searching = !done;
                    this.dirty = true;
                    cx.notify();
                    done
                });
                if done.unwrap_or(true) {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    fn render_find_bar(&self, input: Entity<InputState>, cx: &mut Context<Self>) -> Stateful<Div> {
        let count = match self.find_count {
            (total, Some(i)) => format!("{}/{total}", i + 1),
            (total, None) => format!("-/{total}"),
        };
        div()
            .id("find-bar")
            .key_context("FindBar")
            .absolute()
            .top(px(PADDING))
            .right(px(PADDING * 2.))
            .w(px(300.))
            .h(px(32.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded(px(8.))
            .bg(rgb(TABBAR_BG))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_md()
            .cursor_default()
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            // Keep clicks from reaching the terminal (selection, mouse reports).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_action(
                cx.listener(|this, _: &input::Escape, window, cx| this.close_find(window, cx)),
            )
            .child(icon("search"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&input).appearance(false).text_size(px(12.))),
            )
            .child(div().flex_none().text_color(rgb(MUTED)).child(count))
            .child(
                icon_button("find-up", "chevron-up")
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(true, cx))),
            )
            .child(
                icon_button("find-down", "chevron-down")
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(false, cx))),
            )
            .child(
                icon_button("find-close", "x")
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
    }

    /// The paste protection prompt, over the top of the terminal.
    fn render_paste_prompt(&self, text: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let lines: Vec<&str> = text.lines().collect();
        let mut preview = lines
            .iter()
            .take(6)
            .map(|l| l.chars().take(200).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        if lines.len() > 6 {
            preview.push_str(&format!("\n… {} more lines", lines.len() - 6));
        }
        let title = match lines.len() {
            0 | 1 => "Paste this text?".to_string(),
            n => format!("Paste {n} lines?"),
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(4.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)))
                .when(!primary, |d| {
                    d.border_1()
                        .border_color(rgb(BORDER))
                        .hover(|d| d.bg(rgba(HOVER)))
                })
                .child(label)
        };
        div()
            .id("paste-prompt")
            .absolute()
            .top(px(PADDING * 2.))
            .left(px(PADDING * 2.))
            .right(px(PADDING * 2.))
            .max_w(px(520.))
            .mx_auto()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .rounded(px(8.))
            .bg(rgb(TABBAR_BG))
            .border_1()
            .border_color(rgb(BORDER))
            .shadow_md()
            .cursor_default()
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("It could run commands as soon as it lands in the shell."),
            )
            .child(
                div()
                    .p_2()
                    .rounded(px(5.))
                    .bg(rgba(0x0000004d))
                    .font_family(self.config.font_family.clone())
                    .text_size(px(11.))
                    .max_h(px(140.))
                    .overflow_hidden()
                    .children(preview.lines().map(|l| {
                        div().w_full().overflow_hidden().whitespace_nowrap().child(
                            if l.is_empty() {
                                " ".into()
                            } else {
                                l.to_string()
                            },
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("paste-cancel", "Cancel", false)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_paste(false, cx))),
                    )
                    .child(
                        button("paste-confirm", "Paste", true)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_paste(true, cx))),
                    ),
            )
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed into the find bar bubble up here; they aren't the shell's.
        if !self.focus.is_focused(window) {
            return;
        }
        let ks = &ev.keystroke;
        // The paste confirmation takes the keyboard while it shows.
        if self.pending_paste.is_some() {
            match ks.key.as_str() {
                "enter" => self.confirm_paste(true, cx),
                "escape" => self.confirm_paste(false, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        // Plain text is left to the input handler (TerminalInput) so IMEs and
        // dead keys can compose it, unless a keybind or the kitty keyboard
        // protocol needs the key itself.
        let m = ks.modifiers;
        let text_like = ks.key_char.is_some()
            && !matches!(ks.key.as_str(), "enter" | "tab")
            && !m.control
            && !m.platform
            && !m.function;
        if text_like && self.config.keybind(ks).is_none() && self.term.kitty_flags() == 0 {
            return;
        }
        if self.send_keystroke(ks, cx) {
            cx.stop_propagation();
        }
    }

    /// Encode a keystroke for the shell (Ghostty keybinds first) and write
    /// it. False if it maps to nothing, including unbound ⌘-shortcuts, which
    /// belong to the app.
    pub fn send_keystroke(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        if let Some(bytes) = self.config.keybind(ks).map(<[u8]>::to_vec) {
            self.input(&bytes, cx);
            return true;
        }
        if ks.modifiers.platform {
            return false;
        }
        let Some((key, unshifted)) = map_key(&ks.key) else {
            return false;
        };
        let m = ks.modifiers;
        let mods = mods(m);
        // Like Ghostty's default (macos-option-as-alt = false), Option composes
        // characters; named keys (enter, arrows, …) carry no text.
        let printable = ks.key.chars().count() == 1 || ks.key == "space";
        let text = ks.key_char.as_deref().filter(|_| printable && !m.control);
        let bytes = self.term.encode_key(key, mods, text, unshifted);
        if bytes.is_empty() {
            return false;
        }
        self.input(&bytes, cx);
        true
    }

    /// Run a synthetic mouse event through the handlers (ABROMUX_SCRIPT tests).
    pub fn simulate_mouse(
        &mut self,
        ev: PlatformInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            PlatformInput::MouseDown(e) => self.on_mouse_down(&e, window, cx),
            PlatformInput::MouseUp(e) => self.on_mouse_up(&e, window, cx),
            PlatformInput::MouseMove(e) => self.on_mouse_move(&e, window, cx),
            PlatformInput::ScrollWheel(e) => self.on_scroll(&e, window, cx),
            _ => {}
        }
    }

    /// Whether mouse events go to the program; Shift forces local selection
    /// and scrollback, like Ghostty.
    fn reporting(&self, m: Modifiers) -> bool {
        !m.shift && self.term.mouse_tracking()
    }

    /// Find the link under the pointer while ⌘ is held (`cmd`), or clear it.
    /// True if the underline changed.
    fn update_link(&mut self, cmd: bool) -> bool {
        let found = self.pointer.filter(|_| cmd).and_then(|pos| {
            let (x, y) = self.cell_at(pos);
            if let Some(uri) = self.term.hyperlink(x, y) {
                // An OSC 8 link spans the neighbouring cells with the same URI.
                let same = |x: u16| self.term.hyperlink(x, y).as_ref() == Some(&uri);
                let start = (0..x).rev().take_while(|&x| same(x)).last().unwrap_or(x);
                let end = (x + 1..self.grid.0)
                    .take_while(|&x| same(x))
                    .last()
                    .unwrap_or(x)
                    + 1;
                return Some(HoverLink {
                    y,
                    x: start..end,
                    target: link::Target::Url(uri),
                });
            }
            let cwd = self.live_cwd();
            let row = self.frame().rows.get(y as usize)?;
            link::find(row, x, &cwd).map(|(x, target)| HoverLink { y, x, target })
        });
        let changed = found.as_ref().map(|l| (l.y, l.x.clone()))
            != self.link.as_ref().map(|l| (l.y, l.x.clone()));
        self.link = found;
        changed
    }

    fn report_mouse(
        &mut self,
        action: MouseAction,
        button: Option<u8>,
        m: Modifiers,
        pos: Point<Pixels>,
    ) {
        let (geometry, at) = mouse_space(
            pos - self.bounds.origin,
            self.cell,
            self.cell_px,
            self.scale,
            self.grid,
        );
        let bytes = self.term.encode_mouse(
            action,
            button,
            mods(m),
            at,
            self.mouse_buttons != 0,
            geometry,
        );
        self.send(&bytes);
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta =
            f32::from(ev.delta.pixel_delta(self.cell.height).y) / f32::from(self.cell.height);
        if delta * self.scroll_accum < 0. {
            self.scroll_accum = 0.;
        }
        self.scroll_accum += delta;
        let lines = self.scroll_accum.trunc() as isize;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        if self.reporting(ev.modifiers) {
            // Wheel up/down are buttons 4/5, one press per line.
            for _ in 0..lines.unsigned_abs() {
                self.report_mouse(
                    MouseAction::Press,
                    Some(if lines > 0 { 4 } else { 5 }),
                    ev.modifiers,
                    ev.position,
                );
            }
        } else if self.term.alt_screen() && !ev.modifiers.shift {
            // Alternate scroll (mode 1007, on by default in Ghostty): pagers
            // and other full-screen apps without mouse support get arrows.
            let key = Keystroke::parse(if lines > 0 { "up" } else { "down" }).unwrap();
            for _ in 0..lines.unsigned_abs() {
                self.send_keystroke(&key, cx);
            }
        } else {
            self.term.scroll(-lines);
            self.dirty = true;
            cx.notify();
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        cx.emit(TerminalEvent::Focused);
        if ev.button == MouseButton::Left && ev.modifiers.platform {
            self.pointer = Some(ev.position);
            self.update_link(true);
            if let Some(l) = self.link.take() {
                cx.emit(TerminalEvent::OpenLink(l.target));
                cx.notify();
                return;
            }
        }
        if self.reporting(ev.modifiers)
            && let Some(b) = button_code(ev.button)
        {
            self.mouse_buttons |= 1 << b;
            self.report_mouse(MouseAction::Press, Some(b), ev.modifiers, ev.position);
            return;
        }
        if ev.button != MouseButton::Left {
            return;
        }
        let cell = self.cell_at(ev.position);
        if ev.click_count >= 2 {
            self.term.select_word(cell.0, cell.1);
            self.selecting = None;
        } else {
            self.term.clear_selection();
            self.selecting = Some(cell);
        }
        self.dirty = true;
        cx.notify();
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = None;
        if let Some(b) = button_code(ev.button)
            && (self.mouse_buttons & (1 << b) != 0 || self.reporting(ev.modifiers))
        {
            self.mouse_buttons &= !(1 << b);
            self.report_mouse(MouseAction::Release, Some(b), ev.modifiers, ev.position);
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pointer = Some(ev.position);
        if self.update_link(ev.modifiers.platform && ev.pressed_button.is_none()) {
            cx.notify();
        }
        if self.reporting(ev.modifiers) {
            // The encoder drops motion the program's tracking mode doesn't want.
            let held = ev
                .pressed_button
                .and_then(button_code)
                .filter(|b| self.mouse_buttons & (1 << b) != 0);
            if ev.pressed_button.is_none() || held.is_some() {
                self.report_mouse(MouseAction::Motion, held, ev.modifiers, ev.position);
            }
            return;
        }
        if let (Some(start), Some(MouseButton::Left)) = (self.selecting, ev.pressed_button) {
            let end = self.cell_at(ev.position);
            if end != start {
                self.term.select(start, end);
                self.dirty = true;
                cx.notify();
            }
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = self.frame.background;
        let find_bar = self
            .find_input
            .clone()
            .filter(|_| self.find_open)
            .map(|input| self.render_find_bar(input, cx));
        let paste_prompt = self
            .pending_paste
            .as_deref()
            .map(|text| self.render_paste_prompt(text, cx));
        div()
            .id("terminal")
            .relative()
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .bg(rgb_to_hsla(if bg == Rgb::default() {
                Rgb(0x1a, 0x19, 0x18)
            } else {
                bg
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                this.copy(cx);
            }))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, _, cx| {
                this.set_font_size(this.font_size + 1., cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, _, cx| {
                this.set_font_size(this.font_size - 1., cx)
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, _, cx| {
                this.set_font_size(this.config.font_size, cx)
            }))
            .on_action(cx.listener(|this, _: &ClearScrollback, _, cx| this.clear_scrollback(cx)))
            .on_action(cx.listener(|this, _: &RestartShell, _, cx| this.respawn(cx)))
            .on_drop(cx.listener(Self::drop_paths))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(rgba(0x1f6fe522)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(true, cx)))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find_step(false, cx)))
            .on_action(cx.listener(|this, _: &UseSelectionForFind, window, cx| {
                this.find_selection(window, cx)
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                let Some(item) = cx.read_from_clipboard() else {
                    return;
                };
                if let Some(text) = item.text() {
                    this.paste(&text, cx);
                } else if item
                    .entries()
                    .iter()
                    .any(|e| matches!(e, ClipboardEntry::Image(_)))
                {
                    // No text to paste, so hand the app a raw Ctrl+V: TUIs like
                    // Claude Code read the image from the system clipboard on it.
                    this.input(b"\x16", cx);
                }
            }))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.pointer = None;
                    if this.update_link(false) {
                        cx.notify();
                    }
                }
            }))
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, _, cx| {
                if this.update_link(ev.modifiers.platform) {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .cursor(if self.link.is_some() {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::IBeam
            })
            .child(TerminalElement { view: cx.entity() })
            .children(find_bar)
            .children(paste_prompt)
    }
}

/// Paints the libghostty-vt render snapshot.
struct TerminalElement {
    view: Entity<TerminalView>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct Prepaint {
    marked: Option<String>,
    frame: Rc<Frame>,
    /// Each of the frame's images with its texture, in drawing order.
    images: Vec<(ghostty::Image, Arc<RenderImage>)>,
    /// Logical pixels per image pixel, across and down.
    image_scale: (f32, f32),
    cell: Size<Pixels>,
    font: Font,
    font_size: Pixels,
    focused: bool,
    selection: (Rgb, Option<Rgb>),
    cursor_text: Option<Rgb>,
    /// The ⌘-hovered link's row and columns, underlined.
    link: Option<(u16, Range<u16>)>,
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size = Size::full();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let view = self.view.read(cx);
        let font = font(view.config.font_family.clone());
        let font_size = px(view.font_size);
        let selection = (
            view.config
                .selection_background
                .unwrap_or(Rgb(0x5a, 0x3a, 0x2c)),
            view.config.selection_foreground,
        );
        let cursor_text = view.config.cursor_text;
        let link = view.link.as_ref().map(|l| (l.y, l.x.clone()));
        let ts = window.text_system();
        let font_id = ts.resolve_font(&font);
        let cell_w = ts
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(px(8.));
        let cell = size(cell_w, (font_size * LINE_HEIGHT).round());
        let cols = ((f32::from(bounds.size.width) - PADDING * 2.) / f32::from(cell.width))
            .floor()
            .max(2.) as u16;
        let rows = ((f32::from(bounds.size.height) - PADDING * 2.) / f32::from(cell.height))
            .floor()
            .max(1.) as u16;
        let focused = self.view.read(cx).focus.is_focused(window);
        let (frame, marked, images, image_scale) = self.view.update(cx, |v, cx| {
            v.bounds = bounds;
            v.cell = cell;
            v.scale = window.scale_factor();
            v.window = Some(window.window_handle());
            v.resize(cols, rows);
            v.animate(cx);
            let frame = v.frame().clone();
            for texture in v.stale_images.drain(..) {
                let _ = window.drop_image(texture);
            }
            let images = frame
                .images
                .iter()
                .filter_map(|im| Some((*im, v.images.get(&(im.generation, im.src))?.clone())))
                .collect::<Vec<_>>();
            let image_scale = (
                f32::from(cell.width) / v.cell_px.0 as f32,
                f32::from(cell.height) / v.cell_px.1 as f32,
            );
            if let Some(c) = frame.cursor {
                let origin = bounds.origin + point(px(PADDING), px(PADDING));
                v.ime_cursor = Bounds::new(
                    point(
                        origin.x + cell.width * c.x as f32,
                        origin.y + cell.height * c.y as f32,
                    ),
                    cell,
                );
            }
            (frame, v.marked.clone(), images, image_scale)
        });
        Prepaint {
            marked,
            frame,
            images,
            image_scale,
            cell,
            font,
            font_size,
            focused,
            selection,
            cursor_text,
            link,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        p: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Prepaint {
            marked,
            frame,
            images,
            image_scale,
            cell,
            font,
            font_size,
            focused,
            selection,
            cursor_text,
            link,
        } = p;
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(&focus, TerminalInput(self.view.clone()), cx);
        let font_size = *font_size;
        let origin = bounds.origin + point(px(PADDING), px(PADDING));
        let default_fg = frame.foreground;
        let default_bg = frame.background;
        let (selection_bg, selection_fg) = *selection;

        let paint_images = |layers: &[ImageLayer], window: &mut Window| {
            for (im, texture) in images.iter().filter(|(im, _)| layers.contains(&im.layer)) {
                let (sx, sy) = *image_scale;
                let b = Bounds::new(
                    point(
                        origin.x + cell.width * im.col as f32 + px(im.offset.0 as f32 * sx),
                        origin.y + cell.height * im.row as f32 + px(im.offset.1 as f32 * sy),
                    ),
                    size(px(im.size.0 as f32 * sx), px(im.size.1 as f32 * sy)),
                );
                let _ = window.paint_image(b, Corners::default(), texture.clone(), 0, false);
            }
        };

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, rgb_to_hsla(default_bg)));
            paint_images(&[ImageLayer::BelowBackground], window);

            // Two passes: all cell backgrounds, then images meant to sit
            // between backgrounds and text, then all text.
            for text_pass in [false, true] {
                if text_pass {
                    paint_images(&[ImageLayer::BelowText], window);
                }
                for (y, row) in frame.rows.iter().enumerate() {
                    let row_y = origin.y + cell.height * y as f32;
                    // Some(selected) for cells inside a find-bar match.
                    let mut hits = vec![None; row.len()];
                    for m in frame.matches.iter().filter(|m| m.y as usize == y) {
                        for h in hits
                            .iter_mut()
                            .take(m.x.end as usize)
                            .skip(m.x.start as usize)
                        {
                            *h = Some(m.selected);
                        }
                    }
                    let colors = |x: usize, c: &ghostty::Cell| {
                        let (mut fg, mut bg) = (c.fg.unwrap_or(default_fg), c.bg);
                        if c.inverse {
                            (fg, bg) = (bg.unwrap_or(default_bg), Some(fg));
                        }
                        if let Some(selected) = hits[x] {
                            bg = Some(if selected {
                                MATCH_SELECTED_BG
                            } else {
                                MATCH_BG
                            });
                            fg = MATCH_FG;
                        }
                        if c.selected {
                            bg = Some(selection_bg);
                            fg = selection_fg.unwrap_or(fg);
                        }
                        (fg, bg)
                    };

                    // Backgrounds: merge runs of equal colour into one quad.
                    let mut x = 0;
                    while !text_pass && x < row.len() {
                        let (_, bg) = colors(x, &row[x]);
                        let start = x;
                        while x < row.len() && colors(x, &row[x]).1 == bg {
                            x += 1;
                        }
                        if let Some(bg) = bg {
                            let b = Bounds::new(
                                point(origin.x + cell.width * start as f32, row_y),
                                size(cell.width * (x - start) as f32, cell.height),
                            );
                            window.paint_quad(fill(b, rgb_to_hsla(bg)));
                        }
                    }
                    if !text_pass {
                        continue;
                    }

                    // Text: one shaped line per chunk, each glyph snapped onto the
                    // cell grid (fallback fonts have other advances, e.g. ⎿ or ⏺).
                    // Snapping goes by glyph index, so wide cells and multi-codepoint
                    // cells get their own chunk.
                    let mut text = String::new();
                    let mut runs: Vec<TextRun> = Vec::new();
                    let mut chunk_col = 0usize;
                    // `span` is set for a cell shaped alone: its glyphs are centred
                    // across that many columns as a whole.
                    let flush = |text: &mut String,
                                 runs: &mut Vec<TextRun>,
                                 col: usize,
                                 span: Option<usize>,
                                 window: &mut Window,
                                 cx: &mut App| {
                        if text.trim().is_empty() {
                            text.clear();
                            runs.clear();
                            return;
                        }
                        let mut line = window.text_system().shape_line(
                            SharedString::from(std::mem::take(text)),
                            font_size,
                            runs,
                            None,
                        );
                        runs.clear();
                        *line = std::sync::Arc::new(snap_to_grid(&line, cell.width, span));
                        let _ = line.paint(
                            point(origin.x + cell.width * col as f32, row_y),
                            cell.height,
                            window,
                            cx,
                        );
                    };
                    for (x, c) in row.iter().enumerate() {
                        if c.spacer {
                            continue;
                        }
                        // Box drawing and blocks are drawn, not shaped, so they
                        // fill the cell and join up with their neighbours.
                        if let Some(ch) = sprite_char(c) {
                            flush(&mut text, &mut runs, chunk_col, None, window, cx);
                            chunk_col = x + 1;
                            let (fg, bg) = colors(x, c);
                            let mut color = rgb_to_hsla(fg);
                            if c.faint {
                                color.a *= 0.6;
                            }
                            let b =
                                Bounds::new(point(origin.x + cell.width * x as f32, row_y), *cell);
                            sprite::paint(
                                ch,
                                b,
                                font_size,
                                color,
                                rgb_to_hsla(bg.unwrap_or(default_bg)),
                                window,
                            );
                            continue;
                        }
                        let alone = c.wide || c.text.chars().nth(1).is_some();
                        if alone {
                            flush(&mut text, &mut runs, chunk_col, None, window, cx);
                            chunk_col = x;
                        }
                        let glyph: &str = if c.text.is_empty() || c.invisible {
                            " "
                        } else {
                            &c.text
                        };
                        let (fg, _) = colors(x, c);
                        let mut color = rgb_to_hsla(fg);
                        if c.faint {
                            color.a *= 0.6;
                        }
                        let mut f = font.clone();
                        if c.bold {
                            f.weight = FontWeight::BOLD;
                        }
                        if c.italic {
                            f.style = FontStyle::Italic;
                        }
                        let run = TextRun {
                            len: glyph.len(),
                            font: f,
                            color,
                            background_color: None,
                            underline: (c.underline
                                || link.as_ref().is_some_and(|(ly, lx)| {
                                    *ly as usize == y && lx.contains(&(x as u16))
                                }))
                            .then(|| UnderlineStyle {
                                thickness: px(1.),
                                color: Some(color),
                                wavy: false,
                            }),
                            strikethrough: c.strikethrough.then(|| StrikethroughStyle {
                                thickness: px(1.),
                                color: Some(color),
                            }),
                        };
                        match runs.last_mut() {
                            Some(last)
                                if last.font == run.font
                                    && last.color == run.color
                                    && last.underline == run.underline
                                    && last.strikethrough == run.strikethrough =>
                            {
                                last.len += run.len
                            }
                            _ => runs.push(run),
                        }
                        text.push_str(glyph);
                        if alone {
                            let span = if c.wide { 2 } else { 1 };
                            flush(&mut text, &mut runs, chunk_col, Some(span), window, cx);
                            chunk_col = x + span;
                        }
                    }
                    flush(&mut text, &mut runs, chunk_col, None, window, cx);
                }
            }
            paint_images(&[ImageLayer::AboveText], window);

            if let Some(cur) = frame.cursor {
                let color = rgb_to_hsla(frame.cursor_color.unwrap_or(default_fg));
                let pos = point(
                    origin.x + cell.width * cur.x as f32,
                    origin.y + cell.height * cur.y as f32,
                );
                let wide = frame
                    .rows
                    .get(cur.y as usize)
                    .and_then(|r| r.get(cur.x as usize))
                    .is_some_and(|c| c.wide);
                let w = if wide { cell.width * 2. } else { cell.width };
                let full = Bounds::new(pos, size(w, cell.height));
                let style = if *focused {
                    cur.style
                } else {
                    Caret::BlockHollow
                };
                match style {
                    Caret::Block => {
                        window.paint_quad(fill(full, color));
                        let under = frame
                            .rows
                            .get(cur.y as usize)
                            .and_then(|r| r.get(cur.x as usize));
                        if let Some(ch) = under.and_then(sprite_char) {
                            sprite::paint(
                                ch,
                                full,
                                font_size,
                                rgb_to_hsla(cursor_text.unwrap_or(default_bg)),
                                color,
                                window,
                            );
                        } else if let Some(c) = under.filter(|c| !c.text.is_empty()) {
                            let run = TextRun {
                                len: c.text.len(),
                                font: font.clone(),
                                color: rgb_to_hsla(cursor_text.unwrap_or(default_bg)),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let line = window.text_system().shape_line(
                                c.text.to_string().into(),
                                font_size,
                                &[run],
                                None,
                            );
                            let _ = line.paint(pos, cell.height, window, cx);
                        }
                    }
                    Caret::BlockHollow => {
                        window.paint_quad(outline(full, color, BorderStyle::Solid))
                    }
                    Caret::Bar => {
                        window.paint_quad(fill(Bounds::new(pos, size(px(2.), cell.height)), color))
                    }
                    Caret::Underline => window.paint_quad(fill(
                        Bounds::new(point(pos.x, pos.y + cell.height - px(2.)), size(w, px(2.))),
                        color,
                    )),
                }
                // IME preedit, underlined, over the cursor.
                if let Some(m) = marked.as_ref().filter(|m| !m.is_empty()) {
                    let fg = rgb_to_hsla(default_fg);
                    let underline = Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(fg),
                        wavy: false,
                    });
                    let run = TextRun {
                        len: m.len(),
                        font: font.clone(),
                        color: fg,
                        background_color: None,
                        underline,
                        strikethrough: None,
                    };
                    let line =
                        window
                            .text_system()
                            .shape_line(m.clone().into(), font_size, &[run], None);
                    window.paint_quad(fill(
                        Bounds::new(pos, size(line.width.max(cell.width), cell.height)),
                        rgb_to_hsla(default_bg),
                    ));
                    let _ = line.paint(pos, cell.height, window, cx);
                }
            }
        });
    }
}

/// The encoder's geometry and a pointer position (relative to the element)
/// in it. The encoder takes whole-pixel cells, as the PTY's pixel size does,
/// but real cells are fractional (7.8 px), so a truncated width drifts a
/// column every few dozen. Scale the position into a grid of whole-pixel
/// cells instead, so it lands in the same cell it does on screen.
fn render_image(p: ImagePixels) -> Arc<RenderImage> {
    let buf = image::RgbaImage::from_raw(p.width, p.height, p.bgra).expect("w*h*4 bytes");
    Arc::new(RenderImage::new(vec![image::Frame::new(buf)]))
}

/// The pointer in the device-pixel grid the program was told about
/// (`cell_px`), so SGR-pixel reports (mode 1016) match its image coordinates.
fn mouse_space(
    local: Point<Pixels>,
    cell: Size<Pixels>,
    (cw, ch): (u32, u32),
    scale: f32,
    (cols, rows): (u16, u16),
) -> (MouseGeometry, (f32, f32)) {
    let (cw, ch) = (cw.max(1), ch.max(1));
    let pad = (PADDING * scale).round() as u32;
    let geometry = MouseGeometry {
        width: cols as u32 * cw + pad * 2,
        height: rows as u32 * ch + pad * 2,
        cell_width: cw,
        cell_height: ch,
        padding: pad,
    };
    let scale = |v: Pixels, real: Pixels, whole: u32| {
        (f32::from(v) - PADDING) * whole as f32 / f32::from(real) + pad as f32
    };
    (
        geometry,
        (
            scale(local.x, cell.width, cw),
            scale(local.y, cell.height, ch),
        ),
    )
}

/// The character of a visible cell drawn by `sprite` rather than the font.
/// Puts glyph `i` exactly at column `i`, centred in it when its own advance
/// differs from the cell's. GPUI's `force_width` only moves glyphs that are
/// more than a pixel off, so a fallback glyph left the rest of its chunk a
/// fraction of a pixel out, and Claude Code's spinner, cycling through
/// glyphs of different widths, made the text after it shuffle sideways.
/// With `span`, the line is one cell of that many columns instead, and is
/// centred in it as a whole.
fn snap_to_grid(layout: &LineLayout, cell_w: Pixels, span: Option<usize>) -> LineLayout {
    let shaped: Vec<Pixels> = layout
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| g.position.x))
        .collect();
    let (xs, width) = match span {
        Some(span) => {
            let dx = (cell_w * span as f32 - layout.width) / 2.;
            (
                shaped.iter().map(|&x| x + dx).collect(),
                cell_w * span as f32,
            )
        }
        None => (
            grid_xs(&shaped, layout.width, cell_w),
            cell_w * shaped.len() as f32,
        ),
    };
    let mut xs = xs.into_iter();
    let runs = layout
        .runs
        .iter()
        .map(|r| ShapedRun {
            font_id: r.font_id,
            glyphs: r
                .glyphs
                .iter()
                .map(|g| ShapedGlyph {
                    position: point(xs.next().unwrap(), g.position.y),
                    ..g.clone()
                })
                .collect(),
        })
        .collect();
    LineLayout {
        runs,
        width,
        ..*layout
    }
}

/// Where glyphs shaped at `xs` (in a line `width` wide) go on the grid: glyph
/// `i` in column `i`, centred by its own advance.
fn grid_xs(xs: &[Pixels], width: Pixels, cell_w: Pixels) -> Vec<Pixels> {
    xs.iter()
        .enumerate()
        .map(|(i, &x)| {
            let advance = xs.get(i + 1).copied().unwrap_or(width) - x;
            cell_w * i as f32 + (cell_w - advance) / 2.
        })
        .collect()
}

fn sprite_char(c: &ghostty::Cell) -> Option<char> {
    let mut chars = c.text.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && !c.invisible && sprite::covers(ch)).then_some(ch)
}

/// Start the user's login shell on a new PTY of `size`, and drain its output
/// into the view on the main thread. Output and the exit of a shell from
/// an older `generation` (one `respawn` replaced) are dropped.
#[allow(clippy::type_complexity)]
fn spawn_shell(
    cwd: &str,
    env: &[(String, String)],
    (cols, rows): (u16, u16),
    generation: usize,
    cx: &mut Context<TerminalView>,
) -> (
    Box<dyn Write + Send>,
    Box<dyn MasterPty + Send>,
    Box<dyn portable_pty::Child + Send + Sync>,
) {
    let pty = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-l");
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "abromux");
    for (k, v) in env {
        cmd.env(k, v);
    }
    if cx
        .try_global::<Config>()
        .is_none_or(|c| c.shell_integration)
    {
        crate::shell::inject(&mut cmd, &shell, env);
    }
    cmd.cwd(cwd);
    let child = pty.slave.spawn_command(cmd).expect("spawn shell");
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    let writer = pty.master.take_writer().expect("pty writer");
    let (tx, mut rx) = mpsc::unbounded::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    crate::activity::output();
                    if tx.unbounded_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Drain PTY output on the main thread; batch whatever is queued so a
    // flood of output costs one repaint.
    cx.spawn(async move |this, cx| {
        while let Some(first) = rx.next().await {
            let mut chunk = first;
            while let Ok(more) = rx.try_recv() {
                chunk.extend_from_slice(&more);
            }
            let live = this.update(cx, |this, cx| {
                let live = this.generation == generation;
                if live {
                    this.process(&chunk, cx);
                }
                live
            });
            if !matches!(live, Ok(true)) {
                return;
            }
        }
        let _ = this.update(cx, |this, cx| {
            if this.generation == generation {
                cx.emit(TerminalEvent::Exited)
            }
        });
    })
    .detach();
    (writer, pty.master, child)
}

/// A libghostty-vt terminal with the config's colors and cursor.
fn new_term(cols: u16, rows: u16, config: &Config) -> ghostty::Terminal {
    let mut term = ghostty::Terminal::new(cols, rows).expect("libghostty-vt terminal");
    term.set_colors(
        config.foreground,
        config.background,
        config.cursor_color,
        &config.palette,
    );
    // DECSCUSR as the starting cursor; programs can still change it.
    match config.cursor_style {
        Some(ConfigCursor::Block) => term.write(b"\x1b[2 q"),
        Some(ConfigCursor::Underline) => term.write(b"\x1b[4 q"),
        Some(ConfigCursor::Bar) => term.write(b"\x1b[6 q"),
        _ => {}
    }
    term
}

/// A path as a shell word: Ghostty's backslash escapes for the characters
/// shells treat specially. Control characters (a newline in a file name)
/// become `?`, which globs back to the same file instead of running it.
fn shell_escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_control() {
            out.push('?');
            continue;
        }
        if " \t\\'\"`!#$&;|*?()[]{}<>~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A process's current directory, via libproc's PROC_PIDVNODEPATHINFO.
fn process_cwd(pid: u32) -> Option<String> {
    unsafe extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }
    const PROC_PIDVNODEPATHINFO: i32 = 9;
    // struct proc_vnodepathinfo { vnode_info_path cdir, rdir }, where
    // vnode_info_path is a 152-byte vnode_info followed by char[MAXPATHLEN].
    const VNODE_INFO: usize = 152;
    const MAXPATHLEN: usize = 1024;
    let mut buf = [0u8; 2 * (VNODE_INFO + MAXPATHLEN)];
    let n = unsafe {
        proc_pidinfo(
            pid as i32,
            PROC_PIDVNODEPATHINFO,
            0,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if n as usize != buf.len() {
        return None;
    }
    let path = &buf[VNODE_INFO..VNODE_INFO + MAXPATHLEN];
    let end = path.iter().position(|&b| b == 0)?;
    (end > 0).then(|| String::from_utf8_lossy(&path[..end]).into_owned())
}

fn mods(m: Modifiers) -> ffi::GhosttyMods {
    (m.shift as u16) * ffi::GHOSTTY_MODS_SHIFT as u16
        | (m.control as u16) * ffi::GHOSTTY_MODS_CTRL as u16
        | (m.alt as u16) * ffi::GHOSTTY_MODS_ALT as u16
        | (m.platform as u16) * ffi::GHOSTTY_MODS_SUPER as u16
}

fn button_code(b: MouseButton) -> Option<u8> {
    match b {
        MouseButton::Left => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Middle => Some(3),
        _ => None,
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// macOS text input for a terminal: committed text goes to the PTY, marked
/// (composing) text is drawn at the cursor until the IME commits it.
/// Implemented directly rather than via `ElementInputHandler` so press-and-hold
/// can be turned off: held keys repeat instead of opening the accent menu.
struct TerminalInput(Entity<TerminalView>);

impl InputHandler for TerminalInput {
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        let end = self.0.read(cx).marked.as_deref().map_or(0, utf16_len);
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.0.read(cx).marked.as_deref().map(|m| 0..utf16_len(m))
    }

    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        let marked: Vec<u16> = self.0.read(cx).marked.as_deref()?.encode_utf16().collect();
        let range = range.start.min(marked.len())..range.end.min(marked.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&marked[range]))
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.0.update(cx, |t, cx| {
            t.marked = None;
            if t.pending_paste.is_none() {
                t.input(text.as_bytes(), cx);
            }
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.0.update(cx, |t, cx| {
            t.marked = (!text.is_empty()).then(|| text.to_string());
            cx.notify();
        });
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        self.0.update(cx, |t, cx| {
            t.marked = None;
            cx.notify();
        });
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        Some(self.0.read(cx).ime_cursor)
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}

pub fn rgb_to_hsla(c: Rgb) -> Hsla {
    Rgba {
        r: c.0 as f32 / 255.,
        g: c.1 as f32 / 255.,
        b: c.2 as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// Map a gpui key name to a libghostty physical key + unshifted codepoint.
fn map_key(key: &str) -> Option<(ffi::GhosttyKey, u32)> {
    use ffi::*;
    let named = match key {
        "enter" => GHOSTTY_KEY_ENTER,
        "tab" => GHOSTTY_KEY_TAB,
        "backspace" => GHOSTTY_KEY_BACKSPACE,
        "escape" => GHOSTTY_KEY_ESCAPE,
        "space" => return Some((GHOSTTY_KEY_SPACE, ' ' as u32)),
        "up" => GHOSTTY_KEY_ARROW_UP,
        "down" => GHOSTTY_KEY_ARROW_DOWN,
        "left" => GHOSTTY_KEY_ARROW_LEFT,
        "right" => GHOSTTY_KEY_ARROW_RIGHT,
        "home" => GHOSTTY_KEY_HOME,
        "end" => GHOSTTY_KEY_END,
        "pageup" => GHOSTTY_KEY_PAGE_UP,
        "pagedown" => GHOSTTY_KEY_PAGE_DOWN,
        "delete" => GHOSTTY_KEY_DELETE,
        "insert" => GHOSTTY_KEY_INSERT,
        k if k.len() > 1
            && k.starts_with('f')
            && k[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            GHOSTTY_KEY_F1 + (k[1..].parse::<u32>().unwrap() - 1) as GhosttyKey
        }
        _ => {
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let k = match ch {
                'a'..='z' => GHOSTTY_KEY_A + (ch as u32 - 'a' as u32) as GhosttyKey,
                '0'..='9' => GHOSTTY_KEY_DIGIT_0 + (ch as u32 - '0' as u32) as GhosttyKey,
                '`' => GHOSTTY_KEY_BACKQUOTE,
                '\\' => GHOSTTY_KEY_BACKSLASH,
                '[' => GHOSTTY_KEY_BRACKET_LEFT,
                ']' => GHOSTTY_KEY_BRACKET_RIGHT,
                ',' => GHOSTTY_KEY_COMMA,
                '=' => GHOSTTY_KEY_EQUAL,
                '-' => GHOSTTY_KEY_MINUS,
                '.' => GHOSTTY_KEY_PERIOD,
                '\'' => GHOSTTY_KEY_QUOTE,
                ';' => GHOSTTY_KEY_SEMICOLON,
                '/' => GHOSTTY_KEY_SLASH,
                _ => GHOSTTY_KEY_UNIDENTIFIED,
            };
            return Some((k, ch as u32));
        }
    };
    Some((named, 0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn dropped_paths_are_shell_words() {
        assert_eq!(super::shell_escape("/tmp/a b/c.txt"), "/tmp/a\\ b/c.txt");
        assert_eq!(
            super::shell_escape("/x/it's (1)$.md"),
            "/x/it\\'s\\ \\(1\\)\\$.md"
        );
        assert_eq!(super::shell_escape("/x/a\nb"), "/x/a?b");
    }

    #[test]
    fn reads_a_process_cwd() {
        let here = std::env::current_dir().unwrap().canonicalize().unwrap();
        let got = super::process_cwd(std::process::id()).map(std::path::PathBuf::from);
        assert_eq!(got.map(|p| p.canonicalize().unwrap()), Some(here));
    }

    #[test]
    fn mouse_reports_land_in_the_cell_under_the_pointer() {
        use crate::ghostty::{MouseAction, Terminal};
        use gpui::{point, px, size};
        // Cells are 7.9 px wide on screen; truncated to 7 px the encoder
        // used to report column ~170 for a click on column 150.
        let cell = size(px(7.9), px(17.6));
        let mut t = Terminal::new(200, 40).unwrap();
        t.write(b"\x1b[?1000h\x1b[?1006h");
        for (col, row) in [(0u16, 0u16), (150, 30), (199, 39)] {
            let at = point(
                px(super::PADDING + 7.9 * (col as f32 + 0.9)),
                px(super::PADDING + 17.6 * (row as f32 + 0.9)),
            );
            let (g, pos) = super::mouse_space(at, cell, (8, 18), 1., (200, 40));
            let got = t.encode_mouse(MouseAction::Press, Some(1), 0, pos, false, g);
            assert_eq!(
                got,
                format!("\x1b[<0;{};{}M", col + 1, row + 1).into_bytes()
            );
        }
    }

    #[test]
    fn pixel_mouse_reports_use_the_device_pixels_the_program_was_told() {
        use crate::ghostty::{MouseAction, Terminal};
        use gpui::{point, px, size};
        // On a 2x screen 7.9x17.6 pt cells are 16x35 px to the program
        // (terminal-browser draws its page in those pixels).
        let cell = size(px(7.9), px(17.6));
        let mut t = Terminal::new(200, 40).unwrap();
        t.write(b"\x1b[?1003h\x1b[?1016h");
        let at = point(
            px(super::PADDING + 7.9 * 100.),
            px(super::PADDING + 17.6 * 20.),
        );
        let (g, pos) = super::mouse_space(at, cell, (16, 35), 2., (200, 40));
        let got = t.encode_mouse(MouseAction::Motion, None, 0, pos, false, g);
        assert_eq!(got, b"\x1b[<35;1600;700M");
    }

    #[test]
    fn glyphs_after_a_fallback_glyph_sit_exactly_on_the_grid() {
        use gpui::px;
        // A spinner glyph 8.6 px wide in 7.8 px cells: the text after it is
        // 0.8 px out, under the 1 px GPUI's force_width would correct.
        let got = super::grid_xs(&[px(0.), px(8.6), px(16.4)], px(24.2), px(7.8));
        let got: Vec<f32> = got
            .into_iter()
            .map(|x| (f32::from(x) * 100.).round() / 100.)
            .collect();
        assert_eq!(got, [-0.4, 7.8, 15.6]);
    }
}
