//! Kitty's desktop notifications (OSC 99). libghostty-vt parses them but
//! has no callback for them, so the PTY stream is scanned for them here,
//! next to the parser. Only the title and body are used; the other keys
//! (actions, urgency, icons, …) are ignored.

use std::collections::HashMap;

/// Longest payload kept; longer ones are dropped.
const MAX: usize = 64 * 1024;

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Esc,
    /// After `ESC ]`: the OSC number so far.
    Number(Vec<u8>),
    /// Inside an OSC 99: everything after `99;`.
    Payload(Vec<u8>),
    /// An ESC inside the payload, which `\` turns into the terminator.
    PayloadEsc(Vec<u8>),
}

#[derive(Default)]
pub struct Scanner {
    state: State,
    /// Chunked notifications (`d=0`) by id: title and body so far.
    pending: HashMap<String, (String, String)>,
}

impl Scanner {
    /// Feed PTY output; returns the notifications it completes, as (title, body).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        let mut done = vec![];
        let mut i = 0;
        while i < bytes.len() {
            // Most output is outside any escape: skip to the next ESC.
            if matches!(self.state, State::Ground) {
                match bytes[i..].iter().position(|&b| b == 0x1b) {
                    Some(n) => i += n,
                    None => break,
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
                (State::Number(n), b';') if n == b"99" => State::Payload(vec![]),
                (State::Number(mut n), d) if d.is_ascii_digit() && n.len() < 4 => {
                    n.push(d);
                    State::Number(n)
                }
                (State::Number(_), 0x1b) => State::Esc,
                (State::Number(_), _) => State::Ground,
                (State::Payload(p), 0x07) => {
                    done.extend(self.finish(&p));
                    State::Ground
                }
                (State::Payload(p), 0x1b) => State::PayloadEsc(p),
                (State::Payload(mut p), b) => {
                    if p.len() < MAX {
                        p.push(b);
                        State::Payload(p)
                    } else {
                        State::Ground
                    }
                }
                (State::PayloadEsc(p), b'\\') => {
                    done.extend(self.finish(&p));
                    State::Ground
                }
                // Any other escape cancels the OSC and starts a new sequence.
                (State::PayloadEsc(_), b']') => State::Number(vec![]),
                (State::PayloadEsc(_), _) => State::Ground,
            };
        }
        done
    }

    /// One `metadata;payload` sequence. A notification may come in several
    /// (`d=0` on all but the last), with `p=` saying which part each holds.
    fn finish(&mut self, seq: &[u8]) -> Option<(String, String)> {
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
        let text = if base64 { String::from_utf8_lossy(&decode_base64(payload)).into_owned() } else { payload.to_string() };
        let mut anonymous = Default::default();
        let entry = if id.is_empty() { &mut anonymous } else { self.pending.entry(id.to_string()).or_default() };
        match part {
            "title" => entry.0.push_str(&text),
            "body" => entry.1.push_str(&text),
            // Queries (`?`), icons, buttons, closes: nothing to show.
            _ => return None,
        }
        if more {
            return None;
        }
        let (title, body) = if id.is_empty() { anonymous } else { self.pending.remove(id).unwrap_or_default() };
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

    #[test]
    fn finds_notifications_split_across_reads_and_chunks() {
        let mut s = Scanner::default();
        assert_eq!(s.feed(b"out\x1b]99;;Hello\x07more"), [("Hello".to_string(), String::new())]);
        // Title and body in two sequences, the second split across reads.
        assert!(s.feed(b"\x1b]99;i=7:d=0;Build\x1b\\\x1b]99;i=7:p=bo").is_empty());
        assert_eq!(s.feed(b"dy;ok\x1b\\"), [("Build".to_string(), "ok".to_string())]);
        assert_eq!(s.feed(b"\x1b]99;e=1:p=body;aMOpIQ==\x07"), [(String::new(), "hé!".to_string())]);
        assert!(s.feed(b"\x1b]9;not kitty\x07\x1b]990;x\x07\x1b]99;p=?;\x07").is_empty());
        assert!(s.feed(b"\x1b]99;;cut \x1b[0m").is_empty(), "another escape cancels it");
    }
}
