//! Minimal loopback HTTP server for the shipped-page tests.
//!
//! `file://` loads were intermittently slow to finish on CI runners, so the pages
//! are served over a throwaway loopback port instead — the same load path the
//! download tests use. Nothing leaves the machine; each request is answered once.
#![allow(dead_code)]
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn owned_page_localizer_is_served_as_javascript_not_html() {
    let port = serve("<html>synthetic page</html>".into());
    let mut connection = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    connection
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    connection
        .write_all(b"GET /i18n.js HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    assert!(
        response.contains("Content-Type: application/javascript"),
        "{response}"
    );
    assert!(response.contains("window.OceanI18n"));
    assert!(!response.contains("<html>synthetic page</html>"));
}

/// Serves the page and its fixed shipped localizer asset until the test exits.
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
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("");
                let (status, mime, body) = match path {
                    "/page.html" => ("200 OK", "text/html", html.as_bytes().to_vec()),
                    "/i18n.js" => (
                        "200 OK",
                        "application/javascript",
                        include_bytes!("../../ui/i18n.js").to_vec(),
                    ),
                    _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
