//! Thin safe wrapper over libghostty-vt (the C API built from vendor/ghostty).
//!
//! libghostty-vt owns all terminal state: VT parsing, screen/scrollback,
//! modes, selection and key encoding. We only snapshot it for drawing.

#[allow(
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    dead_code
)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/ghostty_vt.rs"));
}

use ffi::*;
use smol_str::SmolStr;
use std::time::{Duration, Instant};
use std::{ffi::c_void, mem, ptr};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<GhosttyColorRgb> for Rgb {
    fn from(c: GhosttyColorRgb) -> Self {
        Rgb(c.r, c.g, c.b)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    /// Empty for blank cells and wide-char spacer tails. Inline up to 23
    /// bytes, so a cell's text doesn't allocate.
    pub text: SmolStr,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub selected: bool,
    pub wide: bool,
    pub spacer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub style: CursorStyle,
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub cols: u16,
    pub rows: Vec<Vec<Cell>>,
    /// Per row, libghostty's row id: a row that isn't dirty is carried
    /// over from the previous frame by id instead of being read again.
    row_ids: Vec<[u64; 2]>,
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor_color: Option<Rgb>,
    pub cursor: Option<Cursor>,
    /// Find-bar matches in the viewport, one entry per row a match covers.
    pub matches: Vec<Match>,
}

/// The columns `x` of viewport row `y` that a search match covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub y: u16,
    pub x: std::ops::Range<u16>,
    /// The match ⌘G last moved to.
    pub selected: bool,
}

/// Effects raised by the terminal while processing output. Callbacks only
/// record into this struct; the owner drains it after each write.
#[derive(Default)]
pub struct Effects {
    pub pty_response: Vec<u8>,
    pub title_changed: bool,
    pub pwd_changed: bool,
    pub bell: bool,
    /// Desktop notifications (OSC 9 / 777) as (title, body).
    /// OSC 99 is parsed but not passed on by libghostty-vt; see osc99.rs.
    pub notifications: Vec<(String, String)>,
}

/// What the terminal's callbacks write to (its userdata).
struct Userdata {
    fx: Effects,
    /// The render state, so a hold can capture the frame it starts on.
    render: GhosttyRenderState,
    /// When the program started holding the screen (synchronized output).
    held_since: Option<Instant>,
}

/// How long a program may hold the screen before it's drawn anyway, in
/// case it never ends the hold.
pub const HOLD_TIMEOUT: Duration = Duration::from_secs(1);

pub struct Terminal {
    raw: GhosttyTerminal,
    render: GhosttyRenderState,
    row_iter: GhosttyRenderStateRowIterator,
    row_cells: GhosttyRenderStateRowCells,
    key_encoder: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
    mouse_encoder: GhosttyMouseEncoder,
    mouse_event: GhosttyMouseEvent,
    /// The find bar's search; null while it's closed.
    search: GhosttySearch,
    ud: Box<Userdata>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// Geometry the mouse encoder needs to turn pixels into cells.
#[derive(Clone, Copy, Debug)]
pub struct MouseGeometry {
    pub width: u32,
    pub height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding: u32,
}

macro_rules! sized {
    ($t:ty) => {{
        #[allow(unused_unsafe)]
        let mut v: $t = unsafe { mem::zeroed() };
        v.size = mem::size_of::<$t>();
        v
    }};
}

unsafe extern "C" fn on_write_pty(
    _: GhosttyTerminal,
    ud: *mut c_void,
    data: *const u8,
    len: usize,
) {
    let fx = unsafe { &mut (*(ud as *mut Userdata)).fx };
    fx.pty_response
        .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
}
unsafe extern "C" fn on_title(_: GhosttyTerminal, ud: *mut c_void) {
    unsafe { (*(ud as *mut Userdata)).fx.title_changed = true };
}
unsafe extern "C" fn on_pwd(_: GhosttyTerminal, ud: *mut c_void) {
    unsafe { (*(ud as *mut Userdata)).fx.pwd_changed = true };
}
unsafe extern "C" fn on_bell(_: GhosttyTerminal, ud: *mut c_void) {
    unsafe { (*(ud as *mut Userdata)).fx.bell = true };
}
unsafe fn ghostty_str(s: &GhosttyString) -> String {
    if s.ptr.is_null() {
        String::new()
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(s.ptr, s.len) }).into_owned()
    }
}
unsafe extern "C" fn on_notification(
    _: GhosttyTerminal,
    ud: *mut c_void,
    n: *const GhosttyTerminalDesktopNotification,
) {
    let (fx, n) = unsafe { (&mut (*(ud as *mut Userdata)).fx, &*n) };
    fx.notifications
        .push(unsafe { (ghostty_str(&n.title), ghostty_str(&n.body)) });
}
/// Synchronized output began or ended. At the start the terminal holds
/// the last finished frame, so capture it and draw only that until the end.
unsafe extern "C" fn on_render_hold(t: GhosttyTerminal, ud: *mut c_void, held: bool) {
    let ud = unsafe { &mut *(ud as *mut Userdata) };
    if held {
        unsafe { ghostty_render_state_update(ud.render, t) };
        ud.held_since = Some(Instant::now());
    } else {
        ud.held_since = None;
    }
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> anyhow::Result<Self> {
        unsafe {
            let mut raw = ptr::null_mut();
            check(ghostty_terminal_new(ptr::null(), &mut raw, cols, rows))?;
            let mut render = ptr::null_mut();
            check(ghostty_render_state_new(ptr::null(), &mut render))?;
            let mut row_iter = ptr::null_mut();
            check(ghostty_render_state_row_iterator_new(
                ptr::null(),
                &mut row_iter,
            ))?;
            let mut row_cells = ptr::null_mut();
            check(ghostty_render_state_row_cells_new(
                ptr::null(),
                &mut row_cells,
            ))?;
            let mut key_encoder = ptr::null_mut();
            check(ghostty_key_encoder_new(ptr::null(), &mut key_encoder))?;
            let mut key_event = ptr::null_mut();
            check(ghostty_key_event_new(ptr::null(), &mut key_event))?;
            let mut mouse_encoder = ptr::null_mut();
            check(ghostty_mouse_encoder_new(ptr::null(), &mut mouse_encoder))?;
            let mut mouse_event = ptr::null_mut();
            check(ghostty_mouse_event_new(ptr::null(), &mut mouse_event))?;
            let track_last_cell = true;
            ghostty_mouse_encoder_setopt(
                mouse_encoder,
                GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
                &track_last_cell as *const _ as *const c_void,
            );

            let mut ud = Box::new(Userdata {
                fx: Effects::default(),
                render,
                held_since: None,
            });
            let ud_ptr: *mut Userdata = &mut *ud;
            ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_USERDATA, ud_ptr as *const c_void);
            let set_cb = |opt, f: *const c_void| ghostty_terminal_set(raw, opt, f);
            set_cb(
                GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                on_write_pty as *const c_void,
            );
            set_cb(
                GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                on_title as *const c_void,
            );
            set_cb(GHOSTTY_TERMINAL_OPT_PWD_CHANGED, on_pwd as *const c_void);
            set_cb(GHOSTTY_TERMINAL_OPT_BELL, on_bell as *const c_void);
            set_cb(
                GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
                on_notification as *const c_void,
            );
            set_cb(
                GHOSTTY_TERMINAL_OPT_RENDER_HOLD,
                on_render_hold as *const c_void,
            );

            let t = Self {
                raw,
                render,
                row_iter,
                row_cells,
                key_encoder,
                key_event,
                mouse_encoder,
                mouse_event,
                search: ptr::null_mut(),
                ud,
            };
            t.apply_theme();
            Ok(t)
        }
    }

    /// A dark palette in the spirit of Ghostty's default theme.
    fn apply_theme(&self) {
        const ANSI: [u32; 16] = [
            0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6,
            0x666666, 0xd54e53, 0xb9ca4a, 0xe7c547, 0x7aa6da, 0xc397d8, 0x70c0b1, 0xeaeaea,
        ];
        let rgb = |v: u32| GhosttyColorRgb {
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
        };
        let mut palette = [GhosttyColorRgb::default(); 256];
        for (i, c) in ANSI.iter().enumerate() {
            palette[i] = rgb(*c);
        }
        // xterm 6x6x6 cube + grayscale ramp.
        let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
        for i in 0..216u32 {
            let (r, g, b) = (level(i / 36), level((i / 6) % 6), level(i % 6));
            palette[16 + i as usize] = rgb((r << 16) | (g << 8) | b);
        }
        for i in 0..24u32 {
            let v = 8 + i * 10;
            palette[232 + i as usize] = rgb((v << 16) | (v << 8) | v);
        }
        unsafe {
            let fg = rgb(0xdcdfe4);
            let bg = rgb(0x16171a);
            let cursor = rgb(0xdcdfe4);
            ghostty_terminal_set(
                self.raw,
                GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                &fg as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.raw,
                GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                &bg as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.raw,
                GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                &cursor as *const _ as *const c_void,
            );
            ghostty_terminal_set(
                self.raw,
                GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                palette.as_ptr() as *const c_void,
            );
        }
    }

    /// Override the default colors (e.g. from the user's Ghostty config);
    /// None keeps the built-in default.
    pub fn set_colors(
        &self,
        fg: Option<Rgb>,
        bg: Option<Rgb>,
        cursor: Option<Rgb>,
        palette: &[(u8, Rgb)],
    ) {
        let rgb = |c: Rgb| GhosttyColorRgb {
            r: c.0,
            g: c.1,
            b: c.2,
        };
        unsafe {
            for (opt, c) in [
                (GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, fg),
                (GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, bg),
                (GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, cursor),
            ] {
                if let Some(c) = c.map(rgb) {
                    ghostty_terminal_set(self.raw, opt, &c as *const _ as *const c_void);
                }
            }
            if !palette.is_empty() {
                let mut all = [GhosttyColorRgb::default(); 256];
                ghostty_terminal_get(
                    self.raw,
                    GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT,
                    all.as_mut_ptr() as *mut c_void,
                );
                for (i, c) in palette {
                    all[*i as usize] = rgb(*c);
                }
                ghostty_terminal_set(
                    self.raw,
                    GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                    all.as_ptr() as *const c_void,
                );
            }
        }
    }

    /// Whether the running program asked for mouse reports (modes 9/1000/1002/1003).
    pub fn mouse_tracking(&self) -> bool {
        let mut on = false;
        unsafe {
            ghostty_terminal_get(
                self.raw,
                GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                &mut on as *mut _ as *mut c_void,
            )
        };
        on
    }

    /// Whether the program turned on bracketed paste (mode 2004).
    pub fn bracketed_paste(&self) -> bool {
        // GHOSTTY_MODE_BRACKETED_PASTE: ghostty_mode_new(2004, false) is an
        // inline function, so bindgen doesn't see it.
        let mut q = GhosttyTerminalModeConfig {
            mode: 2004,
            value: false,
        };
        unsafe {
            ghostty_terminal_get(
                self.raw,
                GHOSTTY_TERMINAL_DATA_MODE,
                &mut q as *mut _ as *mut c_void,
            )
        };
        q.value
    }

    /// Whether pasting `text` could run commands the user didn't mean to,
    /// as Ghostty judges it: a newline outside a bracketed paste (with
    /// `bracketed_safe`, as Ghostty's `clipboard-paste-bracketed-safe`), or
    /// the bracketed paste terminator anywhere.
    pub fn paste_is_safe(&self, text: &str, bracketed_safe: bool) -> bool {
        if unsafe { ghostty_paste_is_safe(text.as_ptr() as *const _, text.len()) } {
            return true;
        }
        bracketed_safe && self.bracketed_paste() && !text.contains("\x1b[201~")
    }

    /// `text` as the program should receive it: wrapped in bracketed paste
    /// if it asked for that, else with newlines as carriage returns, and
    /// control bytes that could inject escapes blanked out.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let mut data = text.as_bytes().to_vec();
        let bracketed = self.bracketed_paste();
        let mut out = vec![0u8; data.len() + 16];
        let mut written = 0usize;
        unsafe {
            let r = ghostty_paste_encode(
                data.as_mut_ptr() as *mut _,
                data.len(),
                bracketed,
                out.as_mut_ptr() as *mut _,
                out.len(),
                &mut written,
            );
            if r == GHOSTTY_OUT_OF_SPACE {
                out.resize(written, 0);
                ghostty_paste_encode(
                    data.as_mut_ptr() as *mut _,
                    data.len(),
                    bracketed,
                    out.as_mut_ptr() as *mut _,
                    out.len(),
                    &mut written,
                );
            }
        }
        out.truncate(written);
        out
    }

    /// Kitty keyboard protocol flags the program enabled (0 = legacy keys).
    pub fn kitty_flags(&self) -> u8 {
        let mut flags: u8 = 0;
        unsafe {
            ghostty_terminal_get(
                self.raw,
                GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                &mut flags as *mut _ as *mut c_void,
            )
        };
        flags
    }

    /// Encode a mouse event for the program, in whatever tracking mode and
    /// format it asked for. `button` 1-3 are left/right/middle, 4/5 the
    /// wheel; `pos` is in pixels from the element's top-left. Empty when the
    /// mode doesn't report this event.
    pub fn encode_mouse(
        &mut self,
        action: MouseAction,
        button: Option<u8>,
        mods: GhosttyMods,
        pos: (f32, f32),
        any_pressed: bool,
        g: MouseGeometry,
    ) -> Vec<u8> {
        unsafe {
            let enc = self.mouse_encoder;
            ghostty_mouse_encoder_setopt_from_terminal(enc, self.raw);
            let mut size = sized!(GhosttyMouseEncoderSize);
            size.screen_width = g.width;
            size.screen_height = g.height;
            size.cell_width = g.cell_width.max(1);
            size.cell_height = g.cell_height.max(1);
            (
                size.padding_top,
                size.padding_bottom,
                size.padding_left,
                size.padding_right,
            ) = (g.padding, g.padding, g.padding, g.padding);
            ghostty_mouse_encoder_setopt(
                enc,
                GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                &size as *const _ as *const c_void,
            );
            ghostty_mouse_encoder_setopt(
                enc,
                GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                &any_pressed as *const _ as *const c_void,
            );

            let ev = self.mouse_event;
            ghostty_mouse_event_set_action(
                ev,
                match action {
                    MouseAction::Press => GHOSTTY_MOUSE_ACTION_PRESS,
                    MouseAction::Release => GHOSTTY_MOUSE_ACTION_RELEASE,
                    MouseAction::Motion => GHOSTTY_MOUSE_ACTION_MOTION,
                },
            );
            match button {
                Some(b) => ghostty_mouse_event_set_button(ev, b as GhosttyMouseButton),
                None => ghostty_mouse_event_clear_button(ev),
            }
            ghostty_mouse_event_set_mods(ev, mods);
            ghostty_mouse_event_set_position(ev, GhosttyMousePosition { x: pos.0, y: pos.1 });
            let mut buf = [0u8; 64];
            let mut len = 0usize;
            let r = ghostty_mouse_encoder_encode(
                enc,
                ev,
                buf.as_mut_ptr() as *mut _,
                buf.len(),
                &mut len,
            );
            if r == GHOSTTY_SUCCESS {
                buf[..len].to_vec()
            } else {
                Vec::new()
            }
        }
    }

    /// Whether a full-screen program (vim, htop, …) has the alternate screen.
    pub fn alt_screen(&self) -> bool {
        let mut screen: GhosttyTerminalScreen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe {
            ghostty_terminal_get(
                self.raw,
                GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                &mut screen as *mut _ as *mut c_void,
            )
        };
        screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE
    }

    /// Feed PTY output into the parser.
    pub fn write(&mut self, bytes: &[u8]) {
        unsafe { ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) }
    }

    pub fn take_effects(&mut self) -> Effects {
        mem::take(&mut self.ud.fx)
    }

    /// Whether the program is holding the screen mid-redraw (synchronized
    /// output), so drawing shows the frame from before it.
    pub fn held(&self) -> bool {
        self.ud.held_since.is_some()
    }

    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u32, cell_h: u32) {
        unsafe {
            ghostty_terminal_resize(self.raw, cols.max(1), rows.max(1), cell_w, cell_h);
        }
    }

    pub fn scroll(&mut self, delta_rows: isize) {
        let mut v = GhosttyTerminalScrollViewport {
            tag: GHOSTTY_SCROLL_VIEWPORT_DELTA,
            ..Default::default()
        };
        v.value.delta = delta_rows;
        unsafe { ghostty_terminal_scroll_viewport(self.raw, v) }
    }

    pub fn scroll_to_bottom(&mut self) {
        let v = GhosttyTerminalScrollViewport {
            tag: GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            ..Default::default()
        };
        unsafe { ghostty_terminal_scroll_viewport(self.raw, v) }
    }

    fn string_data(&self, key: GhosttyTerminalData) -> String {
        unsafe {
            let mut s = GhosttyString::default();
            if ghostty_terminal_get(self.raw, key, &mut s as *mut _ as *mut c_void)
                != GHOSTTY_SUCCESS
                || s.ptr.is_null()
            {
                return String::new();
            }
            String::from_utf8_lossy(std::slice::from_raw_parts(s.ptr, s.len)).into_owned()
        }
    }

    pub fn title(&self) -> String {
        self.string_data(GHOSTTY_TERMINAL_DATA_TITLE)
    }

    pub fn pwd(&self) -> String {
        let pwd = self.string_data(GHOSTTY_TERMINAL_DATA_PWD);
        // OSC 7 reports file://host/path
        match pwd.strip_prefix("file://") {
            Some(rest) => rest
                .find('/')
                .map(|i| rest[i..].to_string())
                .unwrap_or_default(),
            None => pwd,
        }
    }

    /// Encode a key press via libghostty's encoder (honours kitty keyboard,
    /// application cursor keys, etc). Returns bytes for the PTY.
    pub fn encode_key(
        &mut self,
        key: GhosttyKey,
        mods: GhosttyMods,
        text: Option<&str>,
        unshifted: u32,
    ) -> Vec<u8> {
        unsafe {
            ghostty_key_encoder_setopt_from_terminal(self.key_encoder, self.raw);
            let ev = self.key_event;
            ghostty_key_event_set_action(ev, GHOSTTY_KEY_ACTION_PRESS);
            ghostty_key_event_set_key(ev, key);
            ghostty_key_event_set_mods(ev, mods);
            ghostty_key_event_set_consumed_mods(ev, 0);
            ghostty_key_event_set_composing(ev, false);
            ghostty_key_event_set_unshifted_codepoint(ev, unshifted);
            match text {
                Some(t) => ghostty_key_event_set_utf8(ev, t.as_ptr() as *const _, t.len()),
                None => ghostty_key_event_set_utf8(ev, ptr::null(), 0),
            }
            let mut buf = [0u8; 128];
            let mut len = 0usize;
            let r = ghostty_key_encoder_encode(
                self.key_encoder,
                ev,
                buf.as_mut_ptr() as *mut _,
                buf.len(),
                &mut len,
            );
            if r == GHOSTTY_SUCCESS {
                buf[..len].to_vec()
            } else {
                Vec::new()
            }
        }
    }

    fn grid_ref(&self, x: u16, y: u16) -> Option<GhosttyGridRef> {
        let mut point = GhosttyPoint {
            tag: GHOSTTY_POINT_TAG_VIEWPORT,
            ..Default::default()
        };
        point.value.coordinate = GhosttyPointCoordinate { x, y: y as u32 };
        let mut r = sized!(GhosttyGridRef);
        (unsafe { ghostty_terminal_grid_ref(self.raw, point, &mut r) } == GHOSTTY_SUCCESS)
            .then_some(r)
    }

    /// The OSC 8 hyperlink on a viewport cell, if it has one.
    pub fn hyperlink(&self, x: u16, y: u16) -> Option<String> {
        let r = self.grid_ref(x, y)?;
        let mut len = 0usize;
        unsafe {
            let first = ghostty_grid_ref_hyperlink_uri(&r, ptr::null_mut(), 0, &mut len);
            if len == 0 || (first != GHOSTTY_SUCCESS && first != GHOSTTY_OUT_OF_SPACE) {
                return None;
            }
            let mut buf = vec![0u8; len];
            if ghostty_grid_ref_hyperlink_uri(&r, buf.as_mut_ptr(), buf.len(), &mut len)
                != GHOSTTY_SUCCESS
            {
                return None;
            }
            buf.truncate(len);
            Some(String::from_utf8_lossy(&buf).into_owned()).filter(|u| !u.is_empty())
        }
    }

    /// Select from one viewport cell to another (inclusive).
    pub fn select(&mut self, from: (u16, u16), to: (u16, u16)) {
        let (Some(start), Some(end)) = (self.grid_ref(from.0, from.1), self.grid_ref(to.0, to.1))
        else {
            return;
        };
        let mut sel = sized!(GhosttySelection);
        sel.start = start;
        sel.end = end;
        unsafe {
            ghostty_terminal_set(
                self.raw,
                GHOSTTY_TERMINAL_OPT_SELECTION,
                &sel as *const _ as *const c_void,
            );
        }
    }

    pub fn select_word(&mut self, x: u16, y: u16) {
        let Some(r) = self.grid_ref(x, y) else { return };
        unsafe {
            let mut opts = sized!(GhosttyTerminalSelectWordOptions);
            opts.ref_ = r;
            let mut sel = sized!(GhosttySelection);
            if ghostty_terminal_select_word(self.raw, &opts, &mut sel) == GHOSTTY_SUCCESS {
                ghostty_terminal_set(
                    self.raw,
                    GHOSTTY_TERMINAL_OPT_SELECTION,
                    &sel as *const _ as *const c_void,
                );
            }
        }
    }

    pub fn clear_selection(&mut self) {
        unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, ptr::null()) };
    }

    pub fn selection_text(&self) -> Option<String> {
        let mut opts = sized!(GhosttyTerminalSelectionFormatOptions);
        opts.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
        opts.unwrap = true;
        opts.trim = true;
        unsafe {
            let mut out: *mut u8 = ptr::null_mut();
            let mut len = 0usize;
            if ghostty_terminal_selection_format_alloc(
                self.raw,
                ptr::null(),
                opts,
                &mut out,
                &mut len,
            ) != GHOSTTY_SUCCESS
            {
                return None;
            }
            let text = (!out.is_null()).then(|| {
                String::from_utf8_lossy(std::slice::from_raw_parts(out, len)).into_owned()
            });
            ghostty_free(ptr::null(), out, len);
            text.filter(|t| !t.is_empty())
        }
    }

    /// Plain text of the whole screen, scrollback included.
    pub fn screen_text(&self) -> String {
        self.format_screen(GHOSTTY_FORMATTER_FORMAT_PLAIN)
    }

    /// The whole screen as VT text (styles kept), for replaying into a new
    /// terminal.
    pub fn screen_vt(&self) -> String {
        self.format_screen(GHOSTTY_FORMATTER_FORMAT_VT)
    }

    fn format_screen(&self, emit: GhosttyFormatterFormat) -> String {
        let mut opts = sized!(GhosttyFormatterTerminalOptions);
        opts.emit = emit;
        opts.unwrap = true;
        opts.trim = true;
        opts.extra.size = mem::size_of::<GhosttyFormatterTerminalExtra>();
        opts.extra.screen.size = mem::size_of::<GhosttyFormatterScreenExtra>();
        unsafe {
            let mut f = ptr::null_mut();
            if ghostty_formatter_terminal_new(ptr::null(), &mut f, self.raw, opts)
                != GHOSTTY_SUCCESS
            {
                return String::new();
            }
            let mut out: *mut u8 = ptr::null_mut();
            let mut len = 0usize;
            let ok = ghostty_formatter_format_alloc(f, ptr::null(), &mut out, &mut len)
                == GHOSTTY_SUCCESS;
            let text = if ok && !out.is_null() {
                String::from_utf8_lossy(std::slice::from_raw_parts(out, len)).into_owned()
            } else {
                String::new()
            };
            ghostty_free(ptr::null(), out, len);
            ghostty_formatter_free(f);
            text
        }
    }

    /// Search the screen and scrollback for `needle` (ASCII letters match
    /// either case). Empty clears the search. Results come in as
    /// `search_step` catches up.
    pub fn set_search(&mut self, needle: &str) {
        unsafe {
            if self.search.is_null()
                && ghostty_search_new(ptr::null(), &mut self.search, self.raw) != GHOSTTY_SUCCESS
            {
                self.search = ptr::null_mut();
                return;
            }
            let s = GhosttyString {
                ptr: needle.as_ptr(),
                len: needle.len(),
            };
            ghostty_search_set(
                self.search,
                GHOSTTY_SEARCH_OPT_NEEDLE,
                &s as *const _ as *const c_void,
            );
        }
    }

    pub fn end_search(&mut self) {
        unsafe { ghostty_search_free(self.search) };
        self.search = ptr::null_mut();
    }

    /// Do a bounded amount of searching, reading new output too. True once
    /// the search has caught up with the terminal.
    pub fn search_step(&mut self) -> bool {
        if self.search.is_null() {
            return true;
        }
        let mut status = GHOSTTY_SEARCH_STATUS_COMPLETE;
        unsafe {
            for _ in 0..32 {
                ghostty_search_feed(self.search);
                for _ in 0..64 {
                    ghostty_search_tick(self.search, &mut status);
                    if status != GHOSTTY_SEARCH_STATUS_RUNNING {
                        break;
                    }
                }
                if status == GHOSTTY_SEARCH_STATUS_COMPLETE {
                    break;
                }
            }
        }
        status == GHOSTTY_SEARCH_STATUS_COMPLETE
    }

    /// Move to the next match up the screen (`older`) or down, wrapping and
    /// scrolling it into view. False when there are no matches.
    pub fn search_select(&mut self, older: bool) -> bool {
        if self.search.is_null() {
            return false;
        }
        let opt = if older {
            GHOSTTY_SEARCH_OPT_SELECT_NEXT
        } else {
            GHOSTTY_SEARCH_OPT_SELECT_PREV
        };
        unsafe { ghostty_search_set(self.search, opt, ptr::null()) == GHOSTTY_SUCCESS }
    }

    /// Total matches, and the selected one's index counting from the newest.
    pub fn search_count(&self) -> (usize, Option<usize>) {
        if self.search.is_null() {
            return (0, None);
        }
        let (mut total, mut index) = (0usize, 0usize);
        unsafe {
            ghostty_search_get(
                self.search,
                GHOSTTY_SEARCH_DATA_TOTAL_MATCHES,
                &mut total as *mut _ as *mut c_void,
            );
            let selected = ghostty_search_get(
                self.search,
                GHOSTTY_SEARCH_DATA_SELECTED_INDEX,
                &mut index as *mut _ as *mut c_void,
            ) == GHOSTTY_SUCCESS;
            (total, selected.then_some(index))
        }
    }

    /// A match's endpoints as viewport cells, start first. None if either
    /// is off screen.
    fn viewport_span(&self, s: &GhosttySelection) -> Option<((u16, u32), (u16, u32))> {
        let point = |r: &GhosttyGridRef| {
            let mut p = GhosttyPointCoordinate::default();
            (unsafe {
                ghostty_terminal_point_from_grid_ref(
                    self.raw,
                    r,
                    GHOSTTY_POINT_TAG_VIEWPORT,
                    &mut p,
                )
            } == GHOSTTY_SUCCESS)
                .then_some((p.x, p.y))
        };
        let (a, b) = (point(&s.start)?, point(&s.end)?);
        Some(if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        })
    }

    /// The search's matches in the viewport. Feeds first, so the list
    /// follows scrolling and new output.
    fn viewport_matches(&self, cols: u16, rows: u32) -> Vec<Match> {
        if self.search.is_null() {
            return Vec::new();
        }
        unsafe {
            ghostty_search_feed(self.search);
            let mut buf = GhosttySelectionBuffer {
                ptr: ptr::null_mut(),
                cap: 0,
                len: 0,
            };
            ghostty_search_get(
                self.search,
                GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES,
                &mut buf as *mut _ as *mut c_void,
            );
            if buf.len == 0 {
                return Vec::new();
            }
            let mut sels = vec![sized!(GhosttySelection); buf.len];
            buf = GhosttySelectionBuffer {
                ptr: sels.as_mut_ptr(),
                cap: sels.len(),
                len: 0,
            };
            if ghostty_search_get(
                self.search,
                GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES,
                &mut buf as *mut _ as *mut c_void,
            ) != GHOSTTY_SUCCESS
            {
                return Vec::new();
            }
            sels.truncate(buf.len);
            let mut sel = sized!(GhosttySelection);
            let selected = (ghostty_search_get(
                self.search,
                GHOSTTY_SEARCH_DATA_SELECTED_MATCH,
                &mut sel as *mut _ as *mut c_void,
            ) == GHOSTTY_SUCCESS)
                .then(|| self.viewport_span(&sel))
                .flatten();
            let mut out = Vec::new();
            for s in &sels {
                // Matches on the viewport's pages can lie just past it.
                let Some(span @ ((sx, sy), (ex, ey))) =
                    self.viewport_span(s).filter(|(_, end)| end.1 < rows)
                else {
                    continue;
                };
                for y in sy..=ey {
                    let x = if y == sy { sx } else { 0 }..if y == ey { ex + 1 } else { cols };
                    out.push(Match {
                        y: y as u16,
                        x,
                        selected: Some(span) == selected,
                    });
                }
            }
            out
        }
    }

    /// Snapshot the visible viewport for drawing.
    pub fn frame(&mut self) -> Frame {
        let mut frame = Frame::default();
        self.update_frame(&mut frame);
        frame
    }

    /// Snapshot the visible viewport into `frame`. Only rows libghostty
    /// marks dirty are read again; the rest are moved over from what
    /// `frame` held, by row id.
    pub fn update_frame(&mut self, frame: &mut Frame) {
        let old_cols = frame.cols;
        frame.cols = 0;
        frame.cursor = None;
        frame.cursor_color = None;
        frame.matches.clear();
        unsafe {
            if let Some(t) = self.ud.held_since
                && t.elapsed() >= HOLD_TIMEOUT
            {
                // Given up on: drop the hold and draw what's there.
                let mode = GhosttyTerminalModeConfig {
                    mode: 2026,
                    value: false,
                };
                ghostty_terminal_set(
                    self.raw,
                    GHOSTTY_TERMINAL_OPT_MODE,
                    &mode as *const _ as *const c_void,
                );
                self.ud.held_since = None;
            }
            // While held, the render state keeps the frame captured when the
            // hold began.
            if self.ud.held_since.is_none()
                && ghostty_render_state_update(self.render, self.raw) != GHOSTTY_SUCCESS
            {
                frame.rows.clear();
                frame.row_ids.clear();
                return;
            }
            let rs = self.render;
            let mut colors = sized!(GhosttyRenderStateColors);
            ghostty_render_state_get(
                rs,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            );
            frame.background = colors.background.into();
            frame.foreground = colors.foreground.into();
            frame.cursor_color = colors.cursor_has_value.then(|| colors.cursor.into());

            let mut cols: u16 = 0;
            ghostty_render_state_get(
                rs,
                GHOSTTY_RENDER_STATE_DATA_COLS,
                &mut cols as *mut _ as *mut c_void,
            );
            frame.cols = cols;

            let mut cursor = sized!(GhosttyRenderStateCursor);
            ghostty_render_state_get(
                rs,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cursor as *mut _ as *mut c_void,
            );
            if cursor.visible && cursor.viewport_has_value {
                frame.cursor = Some(Cursor {
                    x: cursor.viewport_x,
                    y: cursor.viewport_y,
                    style: match cursor.visual_style {
                        GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorStyle::Bar,
                        GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                            CursorStyle::Underline
                        }
                        GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
                            CursorStyle::BlockHollow
                        }
                        _ => CursorStyle::Block,
                    },
                });
            }

            ghostty_render_state_get(
                rs,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.row_iter as *mut _ as *mut c_void,
            );
            let mut dirty: GhosttyRenderStateDirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
            ghostty_render_state_get(
                rs,
                GHOSTTY_RENDER_STATE_DATA_DIRTY,
                &mut dirty as *mut _ as *mut c_void,
            );
            let full = dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL || cols != old_cols;
            let mut old_rows = mem::take(&mut frame.rows);
            let mut old_ids = mem::take(&mut frame.row_ids);

            let mut utf8 = [0u8; 64];
            while ghostty_render_state_row_iterator_next(self.row_iter) {
                let mut id = GhosttyRenderStateRowId { bits: [0; 2] };
                ghostty_render_state_row_get(
                    self.row_iter,
                    GHOSTTY_RENDER_STATE_ROW_DATA_ID,
                    &mut id as *mut _ as *mut c_void,
                );
                let mut row_dirty = true;
                ghostty_render_state_row_get(
                    self.row_iter,
                    GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                    &mut row_dirty as *mut _ as *mut c_void,
                );
                // Rows usually stay put or shift by a few, so look near
                // where this one sits now first.
                let at = frame.rows.len();
                let kept = (!full && !row_dirty && id.bits != [0; 2])
                    .then(|| {
                        (at..old_ids.len())
                            .chain(0..at.min(old_ids.len()))
                            .find(|&j| old_ids[j] == id.bits)
                    })
                    .flatten();
                frame.row_ids.push(id.bits);
                if let Some(j) = kept {
                    old_ids[j] = [0; 2];
                    frame.rows.push(mem::take(&mut old_rows[j]));
                    continue;
                }
                frame.rows.push(Vec::with_capacity(cols as usize));
                let row = frame.rows.last_mut().unwrap();
                ghostty_render_state_row_get(
                    self.row_iter,
                    GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    &mut self.row_cells as *mut _ as *mut c_void,
                );
                let cells = self.row_cells;
                while ghostty_render_state_row_cells_next(cells) {
                    let mut cell = Cell::default();
                    let mut raw: GhosttyCell = 0;
                    ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                        &mut raw as *mut _ as *mut c_void,
                    );
                    let mut wide: GhosttyCellWide = GHOSTTY_CELL_WIDE_NARROW;
                    ghostty_cell_get(
                        raw,
                        GHOSTTY_CELL_DATA_WIDE,
                        &mut wide as *mut _ as *mut c_void,
                    );
                    cell.wide = wide == GHOSTTY_CELL_WIDE_WIDE;
                    cell.spacer = wide == GHOSTTY_CELL_WIDE_SPACER_TAIL
                        || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD;

                    let mut buf = GhosttyBuffer {
                        ptr: utf8.as_mut_ptr(),
                        cap: utf8.len(),
                        len: 0,
                    };
                    if ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                        &mut buf as *mut _ as *mut c_void,
                    ) == GHOSTTY_SUCCESS
                        && buf.len > 0
                    {
                        cell.text = SmolStr::new(String::from_utf8_lossy(&utf8[..buf.len]));
                    }

                    let mut c = GhosttyColorRgb::default();
                    if ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                        &mut c as *mut _ as *mut c_void,
                    ) == GHOSTTY_SUCCESS
                    {
                        cell.fg = Some(c.into());
                    }
                    if ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                        &mut c as *mut _ as *mut c_void,
                    ) == GHOSTTY_SUCCESS
                    {
                        cell.bg = Some(c.into());
                    }
                    let mut styled = false;
                    ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
                        &mut styled as *mut _ as *mut c_void,
                    );
                    if styled {
                        let mut style = sized!(GhosttyStyle);
                        ghostty_render_state_row_cells_get(
                            cells,
                            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                            &mut style as *mut _ as *mut c_void,
                        );
                        cell.bold = style.bold;
                        cell.italic = style.italic;
                        cell.faint = style.faint;
                        cell.underline = style.underline != 0;
                        cell.strikethrough = style.strikethrough;
                        cell.inverse = style.inverse;
                        cell.invisible = style.invisible;
                    }
                    ghostty_render_state_row_cells_get(
                        cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED,
                        &mut cell.selected as *mut _ as *mut c_void,
                    );
                    row.push(cell);
                }
            }
            ghostty_render_state_clean(rs);
            frame.matches = self.viewport_matches(cols, frame.rows.len() as u32);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            ghostty_search_free(self.search);
            ghostty_mouse_event_free(self.mouse_event);
            ghostty_mouse_encoder_free(self.mouse_encoder);
            ghostty_key_event_free(self.key_event);
            ghostty_key_encoder_free(self.key_encoder);
            ghostty_render_state_row_cells_free(self.row_cells);
            ghostty_render_state_row_iterator_free(self.row_iter);
            ghostty_render_state_free(self.render);
            ghostty_terminal_free(self.raw);
        }
    }
}

fn check(r: GhosttyResult) -> anyhow::Result<()> {
    if r == GHOSTTY_SUCCESS {
        Ok(())
    } else {
        anyhow::bail!("libghostty-vt error {r}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_output_and_answers_queries() {
        let mut t = Terminal::new(20, 3).unwrap();
        t.write(b"hi \x1b[1;32mworld\x1b[0m\r\n\x1b]0;title\x07\x1b[c");
        let fx = t.take_effects();
        assert!(fx.title_changed);
        assert_eq!(t.title(), "title");
        assert!(
            fx.pty_response.starts_with(b"\x1b[?"),
            "DA1 reply goes back to the pty"
        );
        assert!(
            t.screen_text().starts_with("hi world"),
            "plain text, styles stripped"
        );
        let f = t.frame();
        let row: String = f.rows[0]
            .iter()
            .map(|c| if c.text.is_empty() { " " } else { &c.text })
            .collect();
        assert_eq!(row.trim_end(), "hi world");
        assert!(f.rows[0][3].bold && f.rows[0][3].fg.is_some());
        assert_eq!(f.cursor.map(|c| (c.x, c.y)), Some((0, 1)));
    }

    #[test]
    fn synchronized_output_draws_only_finished_frames() {
        let mut t = Terminal::new(10, 2).unwrap();
        let first = |t: &mut Terminal| t.frame().rows[0][0].text.to_string();
        t.write(b"old");
        assert_eq!(first(&mut t), "o");
        // The hold starts after "new" in the same write: that frame is kept.
        t.write(b"\x1b[Hnew\x1b[?2026h\x1b[2J\x1b[Hx");
        assert!(t.held());
        assert_eq!(first(&mut t), "n", "half-drawn frame isn't shown");
        t.write(b"yz\x1b[?2026l");
        assert!(!t.held());
        assert_eq!(first(&mut t), "x");
        // A resize ends the hold, so the new size is drawn right away.
        t.write(b"\x1b[?2026h");
        t.resize(8, 2, 10, 20);
        assert!(!t.held());
    }

    #[test]
    fn mouse_reports_follow_the_programs_mode() {
        let mut t = Terminal::new(20, 5).unwrap();
        let g = MouseGeometry {
            width: 200,
            height: 100,
            cell_width: 10,
            cell_height: 20,
            padding: 0,
        };
        let press =
            |t: &mut Terminal| t.encode_mouse(MouseAction::Press, Some(1), 0, (35., 45.), false, g);
        assert!(!t.mouse_tracking());
        assert!(
            press(&mut t).is_empty(),
            "no reports until the program asks"
        );
        t.write(b"\x1b[?1000h\x1b[?1006h");
        assert!(t.mouse_tracking());
        assert_eq!(
            press(&mut t),
            b"\x1b[<0;4;3M",
            "SGR, 1-based cell of (35,45)"
        );
        let wheel = t.encode_mouse(MouseAction::Press, Some(4), 0, (5., 5.), false, g);
        assert_eq!(wheel, b"\x1b[<64;1;1M");
        let motion = t.encode_mouse(MouseAction::Motion, None, 0, (5., 5.), false, g);
        assert!(motion.is_empty(), "mode 1000 doesn't report motion");
    }

    #[test]
    fn config_colors_and_alt_screen() {
        let mut t = Terminal::new(10, 2).unwrap();
        t.set_colors(
            Some(Rgb(1, 2, 3)),
            Some(Rgb(4, 5, 6)),
            None,
            &[(1, Rgb(9, 9, 9))],
        );
        t.write(b"\x1b[31mx");
        let f = t.frame();
        assert_eq!((f.foreground, f.background), (Rgb(1, 2, 3), Rgb(4, 5, 6)));
        assert_eq!(
            f.rows[0][0].fg,
            Some(Rgb(9, 9, 9)),
            "palette override applies to SGR 31"
        );
        assert!(!t.alt_screen());
        t.write(b"\x1b[?1049h");
        assert!(t.alt_screen());
    }

    #[test]
    fn search_finds_and_selects_matches() {
        let mut t = Terminal::new(20, 3).unwrap();
        // Five lines in three rows: two matches scroll off into history.
        t.write(b"error one\r\nok\r\nERROR two\r\nfine\r\nan error");
        t.set_search("error");
        while !t.search_step() {}
        assert_eq!(
            t.search_count(),
            (3, None),
            "ASCII letters match either case"
        );
        let visible = |selected: Option<usize>| {
            vec![
                Match {
                    y: 0,
                    x: 0..5,
                    selected: selected == Some(1),
                },
                Match {
                    y: 2,
                    x: 3..8,
                    selected: selected == Some(0),
                },
            ]
        };
        assert_eq!(t.frame().matches, visible(None), "not the one in history");

        assert!(t.search_select(true));
        assert_eq!(t.search_count(), (3, Some(0)), "the newest match first");
        assert_eq!(t.frame().matches, visible(Some(0)));
        assert!(t.search_select(true));
        assert_eq!(t.frame().matches, visible(Some(1)));
        assert!(t.search_select(true));
        assert_eq!(
            t.frame().matches[0],
            Match {
                y: 0,
                x: 0..5,
                selected: true
            },
            "scrolled up to error one"
        );
        assert!(t.search_select(false));
        assert_eq!(t.search_count().1, Some(1));

        t.set_search("");
        assert_eq!(t.search_count(), (0, None));
        t.end_search();
        assert!(t.frame().matches.is_empty());
    }

    #[test]
    fn reports_notifications_and_hyperlinks() {
        let mut t = Terminal::new(40, 3).unwrap();
        t.write(b"\x1b]9;Build done\x07\x1b]777;notify;Claude;Needs input\x1b\\");
        let fx = t.take_effects();
        assert_eq!(
            fx.notifications,
            [
                ("".to_string(), "Build done".to_string()),
                ("Claude".into(), "Needs input".into())
            ]
        );
        t.write(b"see \x1b]8;;https://example.com/a\x1b\\link\x1b]8;;\x1b\\ here");
        assert_eq!(t.hyperlink(5, 0).as_deref(), Some("https://example.com/a"));
        assert_eq!(t.hyperlink(1, 0), None);
    }

    #[test]
    fn incremental_frame_matches_a_fresh_one() {
        let mut t = Terminal::new(20, 4).unwrap();
        let mut inc = Frame::default();
        let steps: &[&dyn Fn(&mut Terminal)] = &[
            &|t| t.write(b"one\r\ntwo\r\nthree"),
            &|t| t.write(b"\r\nfour\r\nfive\r\nsix"),
            &|t| t.write(b"\x1b[2;1H\x1b[31mred\x1b[0m"),
            &|t| t.select((0, 0), (3, 1)),
            &|t| t.write(b"\x1b[2;3r\x1b[2;1H\x1bM"),
            &|t| t.write(b"\x1b[r\x1b[4;1H\r\nseven\r\neight"),
            &|t| t.write(b"\x1b[2J\x1b[H"),
            &|t| t.resize(12, 3, 8, 16),
            &|t| t.write(b"after\r\nresize"),
        ];
        for (i, step) in steps.iter().enumerate() {
            step(&mut t);
            t.update_frame(&mut inc);
            let fresh = t.frame();
            assert_eq!(inc.rows, fresh.rows, "after step {i}");
        }

        // Rows nothing touched are carried over (same buffer), not read
        // again. (Row 1 is dirty too: the cursor left it.)
        let before: Vec<_> = inc.rows.iter().map(|r| r.as_ptr()).collect();
        t.write(b"\x1b[1;1Hx");
        t.update_frame(&mut inc);
        assert_ne!(inc.rows[0].as_ptr(), before[0]);
        assert_eq!(inc.rows[2].as_ptr(), before[2]);
    }

    #[test]
    fn selection_and_keys() {
        let mut t = Terminal::new(20, 3).unwrap();
        t.write(b"hello world");
        t.select((6, 0), (10, 0));
        assert_eq!(t.selection_text().as_deref(), Some("world"));
        assert_eq!(
            t.encode_key(ffi::GHOSTTY_KEY_ARROW_UP, 0, None, 0),
            b"\x1b[A"
        );
        assert_eq!(
            t.encode_key(
                ffi::GHOSTTY_KEY_C,
                ffi::GHOSTTY_MODS_CTRL as u16,
                None,
                'c' as u32
            ),
            b"\x03"
        );
    }

    /// What zsh does on SIGWINCH: it still thinks its prompt is one row, so
    /// it goes to column 0, erases down and prints the prompt again.
    fn zsh_resize(t: &mut Terminal, cols: u16, prompt: &str) {
        t.resize(cols, 6, 10, 20);
        t.write(b"\r\x1b[J");
        t.write(prompt.as_bytes());
    }

    /// The prompt as `src/shell/zsh/abromux.zsh` marks it.
    fn marked(cmd: &str) -> String {
        format!("\x1b]133;A;redraw=1\x07~/repos ❯ \x1b]133;B\x07{cmd}")
    }

    #[test]
    fn unmarked_prompt_leaves_a_copy_on_resize() {
        let line = "~/repos ❯ echo zilvinas";
        let mut t = Terminal::new(30, 6).unwrap();
        t.write(line.as_bytes());
        zsh_resize(&mut t, 18, line);
        assert_eq!(t.screen_text().matches("~/repos").count(), 2);
    }

    #[test]
    fn marked_prompt_is_redrawn_on_wider_resize() {
        let mut t = Terminal::new(18, 6).unwrap();
        t.write(b"some output\r\n");
        t.write(marked("echo zilvinas").as_bytes());
        zsh_resize(&mut t, 30, &marked("echo zilvinas"));
        assert_eq!(
            t.screen_text().trim_end(),
            "some output\n~/repos ❯ echo zilvinas"
        );
    }

    #[test]
    fn command_output_still_reflows() {
        let mut t = Terminal::new(30, 6).unwrap();
        t.write(marked("ls").as_bytes());
        t.write(b"\r\n\x1b]133;C\x07a-file-name-that-is-long");
        t.resize(18, 6, 10, 20);
        assert!(t.screen_text().contains("a-file-name-that-is-long"));
    }
}
