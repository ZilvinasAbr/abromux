//! WebSocket (RFC 6455), server side, as much as the phone app uses: the
//! handshake, text frames each way (fragmented ones reassembled), ping,
//! pong and close. Reading and writing are separate halves, so a reader
//! thread can block while other threads write.

use base64::Engine as _;
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
};

/// Messages larger than this are refused (the phone sends calls and text).
const MAX_MESSAGE: usize = 1 << 20;

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    let mut sha = sha1_smol::Sha1::new();
    sha.update(key.trim().as_bytes());
    sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(sha.digest().bytes())
}

/// Answer the handshake for a request carrying `key`.
pub fn handshake(mut stream: &TcpStream, key: &str) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

/// The writing half; clones share the socket, one frame at a time.
#[derive(Clone)]
pub struct Writer(Arc<Mutex<TcpStream>>);

impl Writer {
    pub fn new(stream: TcpStream) -> Self {
        Self(Arc::new(Mutex::new(stream)))
    }

    pub fn text(&self, s: &str) -> io::Result<()> {
        self.frame(0x1, s.as_bytes())
    }

    pub fn pong(&self, data: &[u8]) -> io::Result<()> {
        self.frame(0xA, data)
    }

    pub fn ping(&self) -> io::Result<()> {
        self.frame(0x9, b"")
    }

    pub fn close(&self) {
        let _ = self.frame(0x8, &1000u16.to_be_bytes());
        if let Ok(s) = self.0.lock() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn frame(&self, opcode: u8, data: &[u8]) -> io::Result<()> {
        let mut out = Vec::with_capacity(data.len() + 10);
        out.extend(encode_head(opcode, data.len()));
        out.extend_from_slice(data);
        let mut s = self.0.lock().unwrap();
        s.write_all(&out)?;
        s.flush()
    }
}

/// A final, unmasked frame's header for `len` bytes.
fn encode_head(opcode: u8, len: usize) -> Vec<u8> {
    let mut h = vec![0x80 | opcode];
    if len < 126 {
        h.push(len as u8);
    } else if len <= u16::MAX as usize {
        h.push(126);
        h.extend((len as u16).to_be_bytes());
    } else {
        h.push(127);
        h.extend((len as u64).to_be_bytes());
    }
    h
}

pub enum Message {
    Text(String),
    Close,
}

/// The reading half.
pub struct Reader<R> {
    r: R,
    writer: Writer,
}

impl<R: Read> Reader<R> {
    pub fn new(r: R, writer: Writer) -> Self {
        Self { r, writer }
    }

    /// The next text message; pings are answered on the way.
    pub fn next(&mut self) -> io::Result<Message> {
        let mut message = Vec::new();
        loop {
            let mut h = [0u8; 2];
            self.r.read_exact(&mut h)?;
            let (fin, opcode) = (h[0] & 0x80 != 0, h[0] & 0x0F);
            let masked = h[1] & 0x80 != 0;
            let len = match h[1] & 0x7F {
                126 => {
                    let mut b = [0u8; 2];
                    self.r.read_exact(&mut b)?;
                    u16::from_be_bytes(b) as usize
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.r.read_exact(&mut b)?;
                    u64::from_be_bytes(b) as usize
                }
                n => n as usize,
            };
            if !masked {
                // Clients must mask (RFC 6455 5.1).
                return Err(bad("unmasked frame"));
            }
            if len > MAX_MESSAGE || message.len() + len > MAX_MESSAGE {
                return Err(bad("message too large"));
            }
            let mut mask = [0u8; 4];
            self.r.read_exact(&mut mask)?;
            let mut data = vec![0u8; len];
            self.r.read_exact(&mut data)?;
            for (i, b) in data.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
            match opcode {
                0x0 | 0x1 => {
                    message.extend_from_slice(&data);
                    if fin {
                        return String::from_utf8(message)
                            .map(Message::Text)
                            .map_err(|_| bad("text isn't UTF-8"));
                    }
                }
                0x8 => return Ok(Message::Close),
                0x9 => self.writer.pong(&data)?,
                0xA => {}
                _ => return Err(bad("unsupported frame")),
            }
        }
    }
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_example_accept_key() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn frame_heads() {
        assert_eq!(encode_head(1, 5), vec![0x81, 5]);
        assert_eq!(encode_head(1, 300), vec![0x81, 126, 1, 44]);
        assert_eq!(encode_head(1, 70_000)[..2], [0x81, 127]);
    }

    #[test]
    fn reads_masked_fragmented_text() {
        // "Hel" + "lo", masked with 37 fa 21 3d (RFC 6455 5.7).
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let frame = |fin: bool, op: u8, text: &[u8]| {
            let mut f = vec![if fin { 0x80 } else { 0 } | op, 0x80 | text.len() as u8];
            f.extend(mask);
            f.extend(text.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
            f
        };
        let mut bytes = frame(false, 1, b"Hel");
        bytes.extend(frame(true, 0, b"lo"));
        // A writer is needed only for pings; give it a socket nobody reads.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let w = Writer::new(TcpStream::connect(listener.local_addr().unwrap()).unwrap());
        let mut r = Reader::new(&bytes[..], w);
        match r.next().unwrap() {
            Message::Text(t) => assert_eq!(t, "Hello"),
            Message::Close => panic!("close"),
        }
    }
}
