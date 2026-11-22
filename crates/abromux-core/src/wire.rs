//! The server's wire format (docs/server-plan.md, "Wire protocol"): a
//! connection starts with JSON lines, and a streamed terminal's connection
//! then switches to frames, `[type: u8][len: u32 big-endian][payload]`.

use std::io::{self, Read, Write};

/// The protocol revision `hello` reports. A server that answers `hello`
/// with an error is revision 1: the holder before it became the server.
pub const PROTOCOL: u64 = 2;

/// Server → app: the PTY's output.
pub const OUTPUT: u8 = 1;
/// Server → app: the program ended; JSON `{"code": n | null}`.
pub const EXIT: u8 = 2;
/// App → server: bytes for the PTY.
pub const INPUT: u8 = 3;
/// App → server: [`Size`], 8 bytes.
pub const RESIZE: u8 = 4;
/// App → server: the screen as the app last saw it, for the next attach.
pub const SNAPSHOT: u8 = 5;
/// App → server: end the program and forget the terminal.
pub const KILL: u8 = 6;
/// App → server: the app quits; send the rest to the backlog.
pub const DETACH: u8 = 7;
/// Server → app: the last frame before a detach; output after it is in the
/// backlog, so the screen the app saves and the backlog meet exactly.
pub const DETACHED: u8 = 8;

/// The largest frame either side accepts. Snapshots are the big ones.
pub const MAX_FRAME: usize = 64 << 20;

/// A terminal's size in cells, and in pixels when known (else 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub width: u16,
    pub height: u16,
}

impl Size {
    pub fn encode(self) -> [u8; 8] {
        let mut b = [0u8; 8];
        for (i, v) in [self.cols, self.rows, self.width, self.height]
            .into_iter()
            .enumerate()
        {
            b[i * 2..i * 2 + 2].copy_from_slice(&v.to_be_bytes());
        }
        b
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        let v = |i: usize| {
            Some(u16::from_be_bytes(
                b.get(i * 2..i * 2 + 2)?.try_into().ok()?,
            ))
        };
        Some(Self {
            cols: v(0)?,
            rows: v(1)?,
            width: v(2)?,
            height: v(3)?,
        })
    }
}

/// One frame's bytes: its header and payload together, for a single write.
pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    w.write_all(&frame(kind, payload))
}

/// The next frame, or None at a clean end of the stream.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0u8; 5];
    match r.read_exact(&mut head[..1]) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    r.read_exact(&mut head[1..])?;
    let len = u32::from_be_bytes(head[1..].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok(Some((head[0], payload)))
}

/// Frames out of bytes that arrive in pieces (a non-blocking reader).
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame; Err for one over [`MAX_FRAME`].
    pub fn next_frame(&mut self) -> Result<Option<(u8, Vec<u8>)>, &'static str> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[1..5].try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            return Err("frame too large");
        }
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        let kind = self.buf[0];
        let payload = self.buf[5..5 + len].to_vec();
        self.buf.drain(..5 + len);
        Ok(Some((kind, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_whole_and_in_pieces() {
        let mut bytes = frame(OUTPUT, b"hello");
        bytes.extend(frame(
            RESIZE,
            &Size {
                cols: 120,
                rows: 40,
                width: 0,
                height: 0,
            }
            .encode(),
        ));
        bytes.extend(frame(KILL, b""));

        let mut r = &bytes[..];
        assert_eq!(
            read_frame(&mut r).unwrap(),
            Some((OUTPUT, b"hello".to_vec()))
        );
        let (k, p) = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(k, RESIZE);
        assert_eq!(Size::decode(&p).unwrap().cols, 120);
        assert_eq!(read_frame(&mut r).unwrap(), Some((KILL, vec![])));
        assert_eq!(read_frame(&mut r).unwrap(), None);

        let mut d = Decoder::default();
        let mut got = Vec::new();
        for b in &bytes {
            d.push(std::slice::from_ref(b));
            while let Some(f) = d.next_frame().unwrap() {
                got.push(f.0);
            }
        }
        assert_eq!(got, [OUTPUT, RESIZE, KILL]);
    }

    #[test]
    fn oversized_frames_are_refused() {
        let mut head = vec![OUTPUT];
        head.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(read_frame(&mut &head[..]).is_err());
        let mut d = Decoder::default();
        d.push(&head);
        assert!(d.next_frame().is_err());
    }
}
