//! `surface.stream`: a terminal's screen, then its output as it comes, for
//! a client that draws the terminal itself (the phone app, `remote.rs`);
//! or a Claude tab's conversation, then its page's events as they come.
//!
//! The start is the viewport as VT text (`frame_vt`): cursor moves and SGR
//! that paint every row of it on a blank terminal of the same size. After
//! that the client gets exactly the bytes the program printed, the same
//! ones abromux parses, so its terminal stays in step with abromux's.
//! Typing goes back through `surface.send_text`/`send_key`, encoded for
//! the modes abromux's terminal is in.
//!
//! A tap is a bounded queue per subscriber. A client that falls too far
//! behind is cut off (its queue full), and reconnects to a fresh start;
//! abromux never waits on it. While nobody is tapped, `output` costs one
//! atomic load.

use abromux_term::ghostty::{Cell, CursorStyle, Frame, Rgb};
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
};

/// What a stream carries after its start.
#[derive(Clone, Debug, PartialEq)]
pub enum Chunk {
    Output(Vec<u8>),
    Resize(u16, u16),
    /// The client that owns the terminal's size now; None is the Mac
    /// (size_owner.rs).
    Owner(Option<String>),
    /// The terminal is gone (closed, or its program ended).
    Closed,
    /// A Claude tab's page events (claude_chat.rs), as a JSON array: the
    /// same batch its page gets.
    Events(String),
}

/// Chunks a subscriber may be behind by before it's cut off. A chunk is one
/// read of the program's output (up to a few hundred KB in a flood).
const QUEUE: usize = 2048;

#[derive(Default)]
struct Taps {
    /// Per terminal (its view's entity id), the subscribers' queues.
    by_term: HashMap<u64, Vec<SyncSender<Chunk>>>,
    /// Receivers made on the main thread, waiting for the connection
    /// thread that asked to take them (`take`), by token.
    pending: HashMap<u64, Receiver<Chunk>>,
    next: u64,
}

static TAPS: Mutex<Option<Taps>> = Mutex::new(None);
static ANY: AtomicBool = AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut Taps) -> R) -> R {
    let mut taps = TAPS.lock().unwrap();
    let taps = taps.get_or_insert_with(Taps::default);
    let r = f(taps);
    ANY.store(!taps.by_term.is_empty(), Ordering::Relaxed);
    r
}

/// Tap terminal `term`. Call on the main thread in the same turn as taking
/// its start, so no output falls between the two. The receiver waits under
/// the returned token for `take`.
pub fn open(term: u64) -> u64 {
    let (tx, rx) = mpsc::sync_channel(QUEUE);
    with(|t| {
        t.by_term.entry(term).or_default().push(tx);
        t.next += 1;
        t.pending.insert(t.next, rx);
        t.next
    })
}

/// A key for a stream no view owns: a terminal's Claude Code session
/// followed from its transcript (`claude_chat::follow`), one per
/// subscriber. Far above any view's entity id.
pub fn private_key() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 62);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Someone still follows `key`: its subscriber hasn't hung up or fallen
/// behind (noticed on the next push).
pub fn tapped(key: u64) -> bool {
    with(|t| t.by_term.contains_key(&key))
}

/// The receiver `open` made under `token`.
pub fn take(token: u64) -> Option<Receiver<Chunk>> {
    with(|t| t.pending.remove(&token))
}

/// Output terminal `term` printed. Cheap while nobody is tapped.
pub fn output(term: u64, bytes: &[u8]) {
    if ANY.load(Ordering::Relaxed) {
        push(term, Chunk::Output(bytes.to_vec()));
    }
}

/// Terminal `term` is now `cols`×`rows`.
pub fn resized(term: u64, cols: u16, rows: u16) {
    if ANY.load(Ordering::Relaxed) {
        push(term, Chunk::Resize(cols, rows));
    }
}

/// Client `owner` (None: the Mac) sets terminal `term`'s size now.
pub fn owner(term: u64, owner: Option<&str>) {
    if ANY.load(Ordering::Relaxed) {
        push(term, Chunk::Owner(owner.map(String::from)));
    }
}

/// Page events from Claude tab `tab` (its browser view's entity id), as
/// the JSON array its page is handed.
pub fn events(tab: u64, batch: &str) {
    if ANY.load(Ordering::Relaxed) {
        push(tab, Chunk::Events(batch.to_string()));
    }
}

/// Terminal `term` is gone: its subscribers hear so, and are dropped.
pub fn closed(term: u64) {
    if ANY.load(Ordering::Relaxed) {
        push(term, Chunk::Closed);
        with(|t| t.by_term.remove(&term));
    }
}

fn push(term: u64, chunk: Chunk) {
    with(|t| {
        let Some(subs) = t.by_term.get_mut(&term) else {
            return;
        };
        // Full (too far behind) or hung up: either way it goes. A full one
        // sees its queue end after what it has, and starts over.
        subs.retain(|s| match s.try_send(chunk.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        });
        if subs.is_empty() {
            t.by_term.remove(&term);
        }
    });
}

/// The text style of a cell, as far as SGR goes.
#[derive(Clone, Copy, Default, PartialEq)]
struct Style {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    bold: bool,
    italic: bool,
    faint: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
    invisible: bool,
}

impl Style {
    fn of(c: &Cell) -> Self {
        Self {
            fg: c.fg,
            bg: c.bg,
            bold: c.bold,
            italic: c.italic,
            faint: c.faint,
            underline: c.underline,
            strikethrough: c.strikethrough,
            inverse: c.inverse,
            invisible: c.invisible,
        }
    }

    /// SGR setting exactly this style (from a reset).
    fn sgr(&self, out: &mut String) {
        out.push_str("\x1b[0");
        for (on, code) in [
            (self.bold, "1"),
            (self.faint, "2"),
            (self.italic, "3"),
            (self.underline, "4"),
            (self.inverse, "7"),
            (self.invisible, "8"),
            (self.strikethrough, "9"),
        ] {
            if on {
                out.push(';');
                out.push_str(code);
            }
        }
        if let Some(Rgb(r, g, b)) = self.fg {
            let _ = write!(out, ";38;2;{r};{g};{b}");
        }
        if let Some(Rgb(r, g, b)) = self.bg {
            let _ = write!(out, ";48;2;{r};{g};{b}");
        }
        out.push('m');
    }
}

/// A blank cell in the default style: trailing ones needn't be written.
fn blank(c: &Cell) -> bool {
    (c.text.is_empty() || c.text == " ") && Style::of(c) == Style::default()
}

/// VT text that paints `frame` on a blank terminal of its size: the
/// alternate screen first if the program is on it, each row by position,
/// then the cursor where it is (`cursor`: x, y, shown).
pub fn frame_vt(frame: &Frame, alt: bool, cursor: Option<(u16, u16, bool)>) -> String {
    let mut out = String::new();
    if alt {
        out.push_str("\x1b[?1049h");
    }
    out.push_str("\x1b[0m\x1b[H\x1b[2J");
    for (y, row) in frame.rows.iter().enumerate() {
        let end = row.iter().rposition(|c| !blank(c)).map_or(0, |i| i + 1);
        if end == 0 {
            continue;
        }
        let _ = write!(out, "\x1b[{};1H", y + 1);
        let mut style = Style::default();
        for cell in &row[..end] {
            if cell.spacer {
                continue;
            }
            let s = Style::of(cell);
            if s != style {
                s.sgr(&mut out);
                style = s;
            }
            if cell.text.is_empty() {
                out.push(' ');
            } else {
                out.push_str(&cell.text);
            }
        }
        if style != Style::default() {
            out.push_str("\x1b[0m");
        }
    }
    let style = frame.cursor.map(|c| c.style);
    match cursor {
        Some((x, y, shown)) => {
            let _ = write!(out, "\x1b[{};{}H", y + 1, x + 1);
            out.push_str(if shown { "\x1b[?25h" } else { "\x1b[?25l" });
        }
        None => out.push_str("\x1b[?25l"),
    }
    out.push_str(match style {
        Some(CursorStyle::Bar) => "\x1b[6 q",
        Some(CursorStyle::Underline) => "\x1b[4 q",
        _ => "\x1b[2 q",
    });
    out
}

/// `#rrggbb`.
pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(t: &str) -> Cell {
        Cell {
            text: t.into(),
            ..Default::default()
        }
    }

    #[test]
    fn paints_rows_by_position_with_styles() {
        let red = Cell {
            fg: Some(Rgb(255, 0, 0)),
            bold: true,
            ..cell("!")
        };
        let mut frame = Frame::default();
        frame.cols = 4;
        frame.rows = vec![
            vec![cell("h"), cell("i"), red, cell("")],
            vec![cell(""), cell(""), cell(""), cell("")],
            vec![cell("$"), cell(" "), cell(""), cell("")],
        ];
        let vt = frame_vt(&frame, false, Some((2, 2, true)));
        assert_eq!(
            vt,
            "\x1b[0m\x1b[H\x1b[2J\x1b[1;1Hhi\x1b[0;1;38;2;255;0;0m!\x1b[0m\x1b[3;1H$\x1b[3;3H\x1b[?25h\x1b[2 q"
        );
        assert!(frame_vt(&frame, true, None).starts_with("\x1b[?1049h"));
    }

    #[test]
    fn wide_spacers_are_skipped() {
        let wide = Cell {
            wide: true,
            ..cell("世")
        };
        let spacer = Cell {
            spacer: true,
            ..cell("")
        };
        let mut frame = Frame::default();
        frame.cols = 3;
        frame.rows = vec![vec![wide, spacer, cell("x")]];
        assert!(frame_vt(&frame, false, None).contains("\x1b[1;1H世x"));
    }

    #[test]
    fn taps_get_output_and_drop_when_closed() {
        let token = open(9_000_001);
        let rx = take(token).unwrap();
        output(9_000_001, b"abc");
        resized(9_000_001, 100, 30);
        closed(9_000_001);
        assert_eq!(rx.recv().unwrap(), Chunk::Output(b"abc".to_vec()));
        assert_eq!(rx.recv().unwrap(), Chunk::Resize(100, 30));
        assert_eq!(rx.recv().unwrap(), Chunk::Closed);
        assert!(rx.recv().is_err());
        assert!(take(token).is_none());
    }
}
