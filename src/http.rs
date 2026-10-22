use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use url::Url;

/// Bounded plain-HTTP probe for a local DSH origin. No redirects or TLS.
/// TCP is a byte stream: neither a request nor a status line is one packet.
pub fn status(url: &Url) -> Option<u16> {
    status_until(url, Instant::now() + Duration::from_secs(6))
}

/// Share an absolute budget with owned-service readiness. Its caller uses a
/// numeric loopback origin, so no unbounded DNS resolution enters that budget.
pub(crate) fn status_until(url: &Url, deadline: Instant) -> Option<u16> {
    fn remaining(deadline: Instant) -> Option<Duration> {
        let time = deadline.checked_duration_since(Instant::now())?;
        (!time.is_zero()).then_some(time.min(Duration::from_secs(2)))
    }
    remaining(deadline)?;
    if url.scheme() != "http" {
        return None;
    }
    let host = url.host_str()?;
    let addrs = url.socket_addrs(|| None).ok()?;
    let mut stream = None;
    for addr in addrs {
        if let Ok(candidate) = TcpStream::connect_timeout(&addr, remaining(deadline)?) {
            stream = Some(candidate);
            break;
        }
    }
    let mut stream = stream?;
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
    let mut bytes = request.as_bytes();
    while !bytes.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?)).ok()?;
        let written = stream.write(bytes).ok()?;
        if written == 0 {
            return None;
        }
        bytes = &bytes[written..];
    }

    let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
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
    remaining(deadline)?;
    (100..=599).contains(&code).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_readiness_budget_bounds_a_slow_http_response() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let (done, stopped) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            if stopped.recv_timeout(Duration::from_millis(800)).is_err() {
                let _ = socket.write_all(b"HTTP/1.1 303 See Other\r\nContent-Length: 0\r\n\r\n");
            }
        });
        let start = Instant::now();
        let status = status_until(&url, start + Duration::from_millis(100));
        let elapsed = start.elapsed();
        let _ = done.send(());
        thread.join().unwrap();
        assert_eq!(
            status, None,
            "a response after the shared deadline must not count as ready"
        );
        assert!(
            elapsed < Duration::from_millis(700),
            "probe ignored its readiness budget: {elapsed:?}"
        );
    }
}
