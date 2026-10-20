//! Minimal loopback HTTP server for the shipped-page tests.
//!
//! `file://` loads were intermittently slow to finish on CI runners, so the pages
//! are served over a throwaway loopback port instead — the same load path the
//! download tests use. Nothing leaves the machine; each request is answered once.
#![allow(dead_code)]
use std::io::{Read, Write};
use std::net::TcpListener;

/// Serves `html` for every request until the test process exits.
///
/// Returns the loopback port; requests for other paths receive a tiny 404 so the
/// page's optional icon request never stalls the load.
pub fn serve(html: String) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the page server");
    let port = listener.local_addr().expect("page server address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let html = html.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let want_page = !request.contains("dsh-icon.svg");
                let (status, body) = if want_page {
                    ("200 OK", html.as_bytes().to_vec())
                } else {
                    ("404 Not Found", b"not found".to_vec())
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            });
        }
    });
    port
}
