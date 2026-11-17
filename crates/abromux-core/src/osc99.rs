//! OSCs libghostty-vt parses but has no callback for, scanned for in the
//! PTY stream next to the parser:
//!
//! - Kitty's desktop notifications (OSC 99). Only the title and body are
//!   used; the other keys (actions, urgency, icons, …) are ignored.
//! - The Program Status Protocol (OSC 7501, `program_status`), and
//!   ConEmu's progress (OSC 9;4) that maps onto it.
//! - Prompt starts (OSC 133 A), which end a program's working status.

use crate::program_status::{self, Record, Report};
use std::collections::HashMap;

/// What the scanner found, in stream order.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// A desktop notification: title, body.
    Notification(String, String),
    /// An OSC 7501 report.
    Status(Report),
    /// OSC 9;4: a record for the root, or None to clear it.
    Progress(Option<Record>),
    /// OSC 133 A: a shell prompt starts.
    Prompt,
    /// OSC 133 C: a command's output starts.
    CommandStart,
    /// OSC 133 D: a command finished, with its exit status if the shell said.
    CommandDone(Option<i32>),
}

/// Which OSC a payload belongs to.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Osc {
    Kitty,
    Status,
    /// OSC 9: only 9;4 is ours (the rest are notifications libghostty-vt reports).
    ConEmu,
    Prompt,
}

impl Osc {
    fn of(n: &[u8]) -> Option<Self> {
        Some(match n {
            b"99" => Osc::Kitty,
            b"7501" => Osc::Status,
            b"9" => Osc::ConEmu,
            b"133" => Osc::Prompt,
            _ => return None,
        })
    }

    fn max(self) -> usize {
        match self {
            Osc::Status => program_status::MAX_SEQUENCE,
            Osc::Prompt => 256,
            _ => MAX,
        }
    }
}

/// Longest payload kept; longer ones are dropped.
const MAX: usize = 64 * 1024;

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Esc,
    /// After `ESC ]`: the OSC number so far.
    Number(Vec<u8>),
    /// Inside one of ours: everything after `99;` (or `7501;`, …).
    Payload(Osc, Vec<u8>),
    /// An ESC inside the payload, which `\` turns into the terminator.
    PayloadEsc(Osc, Vec<u8>),
}

#[derive(Default)]
pub struct Scanner {
    state: State,
    /// Chunked notifications (`d=0`) by id: title and body so far.
    pending: HashMap<String, (String, String)>,
}

impl Scanner {
    /// Feed PTY output; returns the sequences it completes.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Found> {
        let mut done = vec![];
        let mut i = 0;
        while i < bytes.len() {
            // Most output is outside any OSC: skip to the next `ESC ]`,
            // passing over other escapes (colours, cursor moves) in place.
            if matches!(self.state, State::Ground) {
                loop {
                    match memchr::memchr(0x1b, &bytes[i..]) {
                        Some(n) => i += n,
                        None => return done,
                    }
                    match bytes.get(i + 1) {
                        Some(b']') | None => break,
                        Some(_) => i += 1,
                    }
                }
            }
            let b = bytes[i];
            i += 1;
            self.state = match (std::mem::take(&mut self.state), b) {
                (State::Ground, 0x1b) => State::Esc,
                (State::Ground, _) => State::Ground,
                (State::Esc, b']') => State::Number(vec![]),
                (State::Esc, 0x1b) => State::Esc,
                (State::Esc, _) => State::Ground,
                (State::Number(n), b';') => match Osc::of(&n) {
                    Some(osc) => State::Payload(osc, vec![]),
                    None => State::Ground,
                },
                (State::Number(mut n), d) if d.is_ascii_digit() && n.len() < 4 => {
                    n.push(d);
                    State::Number(n)
                }
                (State::Number(_), 0x1b) => State::Esc,
                (State::Number(_), _) => State::Ground,
                (State::Payload(osc, p), 0x07) => {
                    done.extend(self.finish(osc, &p));
                    State::Ground
                }
                (State::Payload(osc, p), 0x1b) => State::PayloadEsc(osc, p),
                (State::Payload(osc, mut p), b) => {
                    if p.len() < osc.max() {
                        p.push(b);
                        State::Payload(osc, p)
                    } else {
                        State::Ground
                    }
                }
                (State::PayloadEsc(osc, p), b'\\') => {
                    done.extend(self.finish(osc, &p));
                    State::Ground
                }
                // Any other escape cancels the OSC and starts a new sequence.
                (State::PayloadEsc(..), b']') => State::Number(vec![]),
                (State::PayloadEsc(..), _) => State::Ground,
            };
        }
        done
    }

    fn finish(&mut self, osc: Osc, seq: &[u8]) -> Option<Found> {
        match osc {
            Osc::Kitty => self
                .kitty(seq)
                .map(|(title, body)| Found::Notification(title, body)),
            Osc::Status => program_status::parse(seq).map(Found::Status),
            Osc::ConEmu => seq
                .strip_prefix(b"4;")
                .or_else(|| (seq == b"4").then_some(b"0".as_slice()))
                .and_then(program_status::conemu_progress)
                .map(Found::Progress),
            Osc::Prompt => {
                let mut it = seq.split(|&b| b == b';');
                match it.next()? {
                    b"A" => Some(Found::Prompt),
                    b"C" => Some(Found::CommandStart),
                    b"D" => Some(Found::CommandDone(
                        it.next()
                            .and_then(|c| std::str::from_utf8(c).ok()?.parse().ok()),
                    )),
                    _ => None,
                }
            }
        }
    }

    /// One `metadata;payload` sequence. A notification may come in several
    /// (`d=0` on all but the last), with `p=` saying which part each holds.
    fn kitty(&mut self, seq: &[u8]) -> Option<(String, String)> {
        let seq = String::from_utf8_lossy(seq);
        let (meta, payload) = seq.split_once(';').unwrap_or((&seq, ""));
        let (mut id, mut more, mut part, mut base64) = ("", false, "title", false);
        for kv in meta.split(':') {
            match kv.split_once('=') {
                Some(("i", v)) => id = v,
                Some(("d", v)) => more = v == "0",
                Some(("p", v)) => part = v,
                Some(("e", v)) => base64 = v == "1",
                _ => {}
            }
        }
        let text = if base64 {
            String::from_utf8_lossy(&decode_base64(payload)).into_owned()
        } else {
            payload.to_string()
        };
        let mut anonymous = Default::default();
        let entry = if id.is_empty() {
            &mut anonymous
        } else {
            self.pending.entry(id.to_string()).or_default()
        };
        match part {
            "title" => entry.0.push_str(&text),
            "body" => entry.1.push_str(&text),
            // Queries (`?`), icons, buttons, closes: nothing to show.
            _ => return None,
        }
        if more {
            return None;
        }
        let (title, body) = if id.is_empty() {
            anonymous
        } else {
            self.pending.remove(id).unwrap_or_default()
        };
        (!title.is_empty() || !body.is_empty()).then_some((title, body))
    }
}

fn decode_base64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (vec![], 0u32, 0);
    for v in s.bytes().filter_map(val) {
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program_status::State;

    fn n(title: &str, body: &str) -> Found {
        Found::Notification(title.into(), body.into())
    }

    #[test]
    fn finds_notifications_split_across_reads_and_chunks() {
        let mut s = Scanner::default();
        assert_eq!(s.feed(b"out\x1b]99;;Hello\x07more"), [n("Hello", "")]);
        // Title and body in two sequences, the second split across reads.
        assert!(
            s.feed(b"\x1b]99;i=7:d=0;Build\x1b\\\x1b]99;i=7:p=bo")
                .is_empty()
        );
        assert_eq!(s.feed(b"dy;ok\x1b\\"), [n("Build", "ok")]);
        assert_eq!(s.feed(b"\x1b]99;e=1:p=body;aMOpIQ==\x07"), [n("", "hé!")]);
        assert!(
            s.feed(b"\x1b]9;not kitty\x07\x1b]990;x\x07\x1b]99;p=?;\x07")
                .is_empty()
        );
        assert!(
            s.feed(b"\x1b]99;;cut \x1b[0m").is_empty(),
            "another escape cancels it"
        );
        // Colours right before one, and an ESC that ends a read.
        assert!(s.feed(b"\x1b[31mred\x1b[0m\x1b").is_empty());
        assert_eq!(
            s.feed(b"]99;;Split\x07\x1b\x1b]99;;Again\x07"),
            [n("Split", ""), n("Again", "")]
        );
    }

    #[test]
    fn finds_program_status_progress_and_prompts() {
        let mut s = Scanner::default();
        let found = s.feed(b"x\x1b]7501;state=working:app=cargo\x1b\\y\x1b]7501;?\x07");
        assert!(
            matches!(&found[0], Found::Status(Report::Set { record, .. })
            if record.state == State::Working && record.app.as_deref() == Some("cargo"))
        );
        assert_eq!(found[1], Found::Status(Report::Query));
        // Split across reads.
        assert!(s.feed(b"\x1b]9;4;1;").is_empty());
        assert!(matches!(&s.feed(b"40\x07")[..],
            [Found::Progress(Some(r))] if r.progress == Some(40)));
        assert_eq!(s.feed(b"\x1b]9;4;0;0\x07"), [Found::Progress(None)]);
        // Other OSC 9s are notifications, libghostty-vt's to report.
        assert!(s.feed(b"\x1b]9;hello\x07").is_empty());
        assert_eq!(
            s.feed(b"\x1b]133;A;cl=m\x07$ \x1b]133;B\x07"),
            [Found::Prompt]
        );
        assert_eq!(
            s.feed(b"\x1b]133;C\x07out\x1b]133;D;2\x07\x1b]133;D\x07"),
            [
                Found::CommandStart,
                Found::CommandDone(Some(2)),
                Found::CommandDone(None)
            ]
        );
        // Too long for a report: dropped.
        let long = format!("\x1b]7501;state=done:msg={}\x07", "A".repeat(5000));
        assert!(s.feed(long.as_bytes()).is_empty());
    }
}
