//! Just enough HTTP/1.1 for the phone bridge: one request per connection
//! (`Connection: close`), read up to its body, and plain responses. The only
//! client is `tailscale serve` on this machine, proxying the phone.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpStream,
    time::Duration,
};

/// Request heads larger than this are refused.
const MAX_HEAD: usize = 32 << 10;
/// Bodies larger than this (push subscriptions are ~500 bytes) are refused.
const MAX_BODY: usize = 64 << 10;

pub struct Request {
    pub method: String,
    /// The path without its query.
    pub path: String,
    /// Header names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Asked to become a WebSocket.
    pub fn upgrade(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
    }
}

/// Read one request from `stream`. The reader may have read past the head
/// into a WebSocket's first frames only if the client sent them before the
/// handshake answer, which browsers don't.
pub fn read(stream: &TcpStream) -> io::Result<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    let mut head = 0;
    let mut next = |r: &mut BufReader<&TcpStream>, line: &mut String| -> io::Result<()> {
        line.clear();
        let n = r.read_line(line)?;
        head += n;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed"));
        }
        if head > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too large"));
        }
        Ok(())
    };
    next(&mut r, &mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad request line",
        ));
    };
    let method = method.to_string();
    let path = target.split('?').next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    loop {
        next(&mut r, &mut line)?;
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let mut req = Request {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let len: usize = req
        .header("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    if len > 0 {
        req.body = vec![0; len];
        r.read_exact(&mut req.body)?;
    }
    stream.set_read_timeout(None)?;
    Ok(req)
}

/// Write a whole response and end the connection's use.
pub fn respond(
    mut stream: &TcpStream,
    status: u16,
    content_type: &str,
    extra: &[(&str, &str)],
    body: &[u8],
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n",
        reason(status),
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

pub fn json(stream: &TcpStream, status: u16, v: &serde_json::Value) -> io::Result<()> {
    respond(
        stream,
        status,
        "application/json",
        &[("Cache-Control", "no-store")],
        v.to_string().as_bytes(),
    )
}

/// A short page saying why not, for a person opening the URL.
pub fn refuse(stream: &TcpStream, status: u16, why: &str) -> io::Result<()> {
    let esc = why
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\">\
         <title>abromux</title><body style=\"font:16px -apple-system,sans-serif;padding:24px;\
         background:#111;color:#eee\"><h1 style=\"font-size:20px\">abromux</h1><p>{esc}</p>"
    );
    respond(
        stream,
        status,
        "text/html; charset=utf-8",
        &[("Cache-Control", "no-store")],
        body.as_bytes(),
    )
}

fn reason(status: u16) -> &'static str {
    match status {
        101 => "Switching Protocols",
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "",
    }
}
