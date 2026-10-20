use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use url::Url;

/// Bounded plain-HTTP probe for a local DSH origin. No redirects or TLS.
/// TCP is a byte stream: neither a request nor a status line is one packet.
pub fn status(url: &Url) -> Option<u16> {
    if url.scheme() != "http" {
        return None;
    }
    let host = url.host_str()?;
    let addrs = url.socket_addrs(|| None).ok()?;
    let mut stream = None;
    for addr in addrs {
        if let Ok(candidate) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            stream = Some(candidate);
            break;
        }
    }
    let mut stream = stream?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    let path = if path.is_empty() { "/" } else { &path };
    let host_header = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\nUser-Agent: dsh-ocean\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut line = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        if remaining.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(remaining)).ok()?;
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        let newline = chunk[..n].iter().position(|byte| *byte == b'\n');
        let end = newline.unwrap_or(n);
        if line.len() + end > 8192 {
            return None;
        }
        line.extend_from_slice(&chunk[..end]);
        if newline.is_some() {
            break;
        }
    }
    let line = std::str::from_utf8(&line).ok()?;
    let mut fields = line.split_whitespace();
    if !matches!(fields.next()?, "HTTP/1.0" | "HTTP/1.1") {
        return None;
    }
    let code = fields.next()?;
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u16 = code.parse().ok()?;
    (100..=599).contains(&code).then_some(code)
}
