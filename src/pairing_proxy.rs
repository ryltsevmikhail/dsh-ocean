//! Short-lived loopback-to-LAN pairing proxy.
//!
//! The DSH server keeps listening on loopback; this proxy accepts TCP connections on
//! a separate LAN-bound port and forwards them, so a phone can reach an otherwise
//! unreachable server for a bounded window. It is deliberately small:
//!
//! * every proxy object carries its own TTL and a `revoke` handle;
//! * the listener stops accepting and open connections are closed on revoke or TTL;
//! * nothing is logged — no request lines, headers, tokens or payloads;
//! * it adds no authentication of its own: whoever can reach the port still needs the
//!   Harness launch token, and the proxy does not make the server trusted by itself.
//!
//! Not supported (and must not be implied): TLS, HTTP rewriting, per-client identify,
//! rate limiting beyond the connection cap, and surviving a client restart.
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;

/// Hard cap so a proxy cannot be used to exhaust local file descriptors.
pub const MAX_CONNECTIONS: usize = 64;
pub const MAX_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Address the proxy listens on (the LAN-facing one).
    pub listen: String,
    /// Loopback target the bytes are forwarded to.
    pub target: String,
    pub ttl_seconds: u64,
    /// Seconds left before the proxy closes itself.
    pub remaining_seconds: u64,
    pub active_connections: usize,
    pub accepted: usize,
    pub revoked: bool,
}

struct Shared {
    revoked: AtomicBool,
    accepted: AtomicUsize,
    active: AtomicUsize,
    deadline: Instant,
    ttl: Duration,
    started: Instant,
}

pub struct Proxy {
    listen: SocketAddr,
    target: SocketAddr,
    shared: Arc<Shared>,
    sockets: Arc<Mutex<Vec<(u64, TcpStream)>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Proxy {
    /// Binds `bind`, forwards to `target`, and stops itself after `ttl`.
    ///
    /// `bind` must not be a loopback address for the pairing use case; a loopback
    /// bind still works so tests can run without exposing anything.
    pub fn start(bind: SocketAddr, target: SocketAddr, ttl: Duration) -> Result<Self> {
        anyhow::ensure!(
            !ttl.is_zero() && ttl <= MAX_TTL,
            "pairing TTL is out of range"
        );
        anyhow::ensure!(
            bind.port() != target.port(),
            "proxy and target share a port"
        );
        let listener = TcpListener::bind(bind).context("bind the pairing proxy")?;
        let listen = listener.local_addr().context("read the proxy address")?;
        listener
            .set_nonblocking(true)
            .context("configure the proxy listener")?;
        let shared = Arc::new(Shared {
            revoked: AtomicBool::new(false),
            accepted: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            deadline: Instant::now() + ttl,
            ttl,
            started: Instant::now(),
        });
        let sockets: Arc<Mutex<Vec<(u64, TcpStream)>>> = Arc::new(Mutex::new(Vec::new()));
        let next_id = Arc::new(AtomicUsize::new(1));
        let shared_for_thread = shared.clone();
        let sockets_for_thread = sockets.clone();
        let thread = std::thread::spawn(move || {
            while !shared_for_thread.revoked.load(Ordering::SeqCst) {
                if Instant::now() >= shared_for_thread.deadline {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _peer)) => {
                        if shared_for_thread.active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                            let _ = stream.shutdown(Shutdown::Both);
                            continue;
                        }
                        let _ = stream.set_nonblocking(false);
                        shared_for_thread.accepted.fetch_add(1, Ordering::SeqCst);
                        shared_for_thread.active.fetch_add(1, Ordering::SeqCst);
                        let id = next_id.fetch_add(1, Ordering::SeqCst) as u64;
                        if let Ok(clone) = stream.try_clone() {
                            if let Ok(mut list) = sockets_for_thread.lock() {
                                list.push((id, clone));
                            }
                        }
                        let shared = shared_for_thread.clone();
                        let sockets_for_worker = sockets_for_thread.clone();
                        std::thread::spawn(move || {
                            forward(stream, target);
                            shared.active.fetch_sub(1, Ordering::SeqCst);
                            // Drop the retained descriptor as soon as this
                            // connection is done, so idle connections cannot
                            // exhaust the process's file descriptors.
                            if let Ok(mut list) = sockets_for_worker.lock() {
                                list.retain(|(existing, _)| *existing != id);
                            }
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
            // TTL or revoke: stop accepting and drop every live connection.
            drop(listener);
            if let Ok(mut list) = sockets_for_thread.lock() {
                for (_, socket) in list.drain(..) {
                    let _ = socket.shutdown(Shutdown::Both);
                }
            }
        });
        Ok(Self {
            listen,
            target,
            shared,
            sockets,
            thread: Some(thread),
        })
    }

    pub fn listen_address(&self) -> SocketAddr {
        self.listen
    }

    /// Stops accepting, closes live connections and waits for the listener thread.
    pub fn revoke(&mut self) {
        self.shared.revoked.store(true, Ordering::SeqCst);
        if let Ok(mut list) = self.sockets.lock() {
            for (_, socket) in list.drain(..) {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// True once the listener has stopped (TTL expired or revoked).
    pub fn is_stopped(&self) -> bool {
        self.shared.revoked.load(Ordering::SeqCst)
            || self
                .shared
                .deadline
                .saturating_duration_since(Instant::now())
                .is_zero()
            || self
                .thread
                .as_ref()
                .map(|thread| thread.is_finished())
                .unwrap_or(true)
    }

    pub fn status(&self) -> Status {
        let now = Instant::now();
        let remaining = self
            .shared
            .deadline
            .saturating_duration_since(now)
            .as_secs();
        let _ = self.shared.started;
        Status {
            listen: self.listen.to_string(),
            target: self.target.to_string(),
            ttl_seconds: self.shared.ttl.as_secs(),
            remaining_seconds: remaining,
            active_connections: self.shared.active.load(Ordering::SeqCst),
            accepted: self.shared.accepted.load(Ordering::SeqCst),
            revoked: self.shared.revoked.load(Ordering::SeqCst),
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.revoke();
    }
}

fn forward(client: TcpStream, target: SocketAddr) {
    let upstream = match TcpStream::connect_timeout(&target, Duration::from_secs(5)) {
        Ok(stream) => stream,
        Err(_) => {
            let _ = client.shutdown(Shutdown::Both);
            return;
        }
    };
    let (Ok(mut client_read), Ok(mut upstream_write)) = (client.try_clone(), upstream.try_clone())
    else {
        return;
    };
    let (Ok(mut upstream_read), Ok(mut client_write)) = (upstream.try_clone(), client.try_clone())
    else {
        return;
    };
    let upstream_to_client = std::thread::spawn(move || {
        copy(&mut upstream_read, &mut client_write);
        let _ = client_write.shutdown(Shutdown::Write);
    });
    copy(&mut client_read, &mut upstream_write);
    let _ = upstream_write.shutdown(Shutdown::Write);
    // Bounded wait: the peer side closes when either direction ends.
    let _ = upstream_to_client.join();
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = client.shutdown(Shutdown::Both);
}

fn copy(from: &mut TcpStream, to: &mut TcpStream) {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match from.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if to.write_all(&buffer[..read]).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_server() -> (SocketAddr, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if stop_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut buffer = [0u8; 1024];
                    while let Ok(read) = stream.read(&mut buffer) {
                        if read == 0 || stream.write_all(&buffer[..read]).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (addr, stop)
    }

    fn roundtrip(proxy_addr: SocketAddr, payload: &[u8]) -> Vec<u8> {
        let mut client = TcpStream::connect(proxy_addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.write_all(payload).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut received = Vec::new();
        client.read_to_end(&mut received).unwrap();
        received
    }

    #[test]
    fn bytes_are_forwarded_both_ways_and_counted() {
        let (target, stop) = echo_server();
        let proxy = Proxy::start(
            "127.0.0.1:0".parse().unwrap(),
            target,
            Duration::from_secs(30),
        )
        .unwrap();
        let status = proxy.status();
        assert_eq!(status.target, target.to_string());
        assert_eq!(status.accepted, 0);
        assert_eq!(
            roundtrip(proxy.listen_address(), b"hello pairing"),
            b"hello pairing"
        );
        let status = proxy.status();
        assert_eq!(status.accepted, 1);
        assert!(status.remaining_seconds <= 30);
        assert!(!status.revoked);
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn revoke_stops_accepting_and_closes_live_connections() {
        let (target, stop) = echo_server();
        let mut proxy = Proxy::start(
            "127.0.0.1:0".parse().unwrap(),
            target,
            Duration::from_secs(30),
        )
        .unwrap();
        let address = proxy.listen_address();
        assert_eq!(roundtrip(address, b"before revoke"), b"before revoke");
        proxy.revoke();
        assert!(proxy.status().revoked);
        // New connections fail once the listener is gone.
        assert!(TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_err());
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn ttl_closes_the_proxy_without_an_explicit_revoke() {
        let (target, stop) = echo_server();
        let proxy = Proxy::start(
            "127.0.0.1:0".parse().unwrap(),
            target,
            Duration::from_millis(200),
        )
        .unwrap();
        let address = proxy.listen_address();
        assert_eq!(roundtrip(address, b"during ttl"), b"during ttl");
        std::thread::sleep(Duration::from_millis(400));
        assert!(TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_err());
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn completed_connections_release_their_descriptors() {
        let (target, stop) = echo_server();
        let proxy = Proxy::start(
            "127.0.0.1:0".parse().unwrap(),
            target,
            Duration::from_secs(60),
        )
        .unwrap();
        let address = proxy.listen_address();
        let descriptors = || {
            std::fs::read_dir("/proc/self/fd")
                .map(|it| it.count())
                .unwrap_or(0)
        };
        // Warm up, then run more connections than the concurrent cap so a per
        // connection leak would be unmistakable.
        for _ in 0..3 {
            assert_eq!(roundtrip(address, b"warm"), b"warm");
        }
        let before = descriptors();
        for _ in 0..150 {
            assert_eq!(roundtrip(address, b"ping"), b"ping");
        }
        // Give the forwarding threads a moment to drop their retained clones.
        std::thread::sleep(Duration::from_millis(200));
        let after = descriptors();
        assert!(
            after <= before + 4,
            "descriptors grew from {before} to {after} over 150 completed connections"
        );
        assert_eq!(proxy.status().accepted, 153);
        stop.store(true, Ordering::SeqCst);
    }

    #[test]
    fn out_of_range_ttl_and_shared_ports_are_refused() {
        let (target, stop) = echo_server();
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert!(Proxy::start(bind, target, Duration::ZERO).is_err());
        assert!(Proxy::start(bind, target, MAX_TTL + Duration::from_secs(1)).is_err());
        // Same port on both sides cannot work (bind would collide with the target).
        let same: SocketAddr = format!("127.0.0.1:{}", target.port()).parse().unwrap();
        assert!(Proxy::start(same, target, Duration::from_secs(5)).is_err());
        stop.store(true, Ordering::SeqCst);
    }
}
