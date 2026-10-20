use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use dsh_ocean::{auth, http};
use url::Url;

fn server(chunks: Vec<Vec<u8>>) -> Url {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake DSH");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // Drain the full request before closing. Reading only one TCP chunk can
        // leave unread bytes and cause a reset while the client is still writing.
        let mut request = Vec::new();
        let mut buf = [0u8; 256];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 || request.len() + n > 8192 {
                return;
            }
            request.extend_from_slice(&buf[..n]);
        }
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 {
                thread::sleep(Duration::from_millis(5));
            }
            if stream.write_all(chunk).is_err() {
                break;
            }
        }
    });
    Url::parse(&format!("http://{addr}/")).unwrap()
}

fn one_shot_server(status: u16) -> Url {
    server(vec![format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()])
}

#[test]
fn accepts_a_valid_token_exchange() {
    assert!(auth::validate(&one_shot_server(303), "test-token"));
}

#[test]
fn rejects_a_401_token_exchange() {
    assert!(!auth::validate(&one_shot_server(401), "stale-token"));
}

#[test]
fn probes_the_clean_origin_without_a_token() {
    for _ in 0..32 {
        assert_eq!(http::status(&one_shot_server(401)), Some(401));
    }
}

#[test]
fn reads_a_fragmented_status_line() {
    let url = server(vec![
        b"HTTP/1.".to_vec(),
        b"1 3".to_vec(),
        b"03 See Other\r".to_vec(),
        b"\n\r\n".to_vec(),
    ]);
    assert_eq!(http::status(&url), Some(303));
}

#[test]
fn rejects_malformed_or_unbounded_status_lines() {
    for line in [
        "NOTHTTP 303 Test\r\n".to_owned(),
        "HTTP/1.1 999 Test\r\n".to_owned(),
        "HTTP/1.1 30X Test\r\n".to_owned(),
        format!("HTTP/1.1 303 {}\r\n", "x".repeat(8192)),
    ] {
        assert_eq!(http::status(&server(vec![line.into_bytes()])), None);
    }
}

#[test]
fn does_not_send_plaintext_to_https_origins() {
    assert_eq!(
        http::status(&Url::parse("https://127.0.0.1:1/?token=synthetic").unwrap()),
        None
    );
}
