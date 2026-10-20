//! Real WebSocket -> isolated WebKit world -> Rust -> private fake D-Bus.
//! No real DSH service/session/model request or real desktop notice is used.
#[path = "support/notification_bus.rs"]
mod bus;
use dsh_ocean::{
    notifications::{Client, Kind},
    task_notifications,
};
use gtk::{gio, glib, prelude::*};
use javascriptcore::ValueExt;
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;
use webkit2gtk::{WebContext, WebView, WebViewExt};

struct Server {
    base: Url,
    stop: Arc<AtomicBool>,
    joins: Option<std::thread::JoinHandle<()>>,
    sockets: Arc<AtomicUsize>,
}
fn read_frame(stream: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header)?;
    let mut len = (header[1] & 127) as usize;
    if len == 126 {
        let mut n = [0u8; 2];
        stream.read_exact(&mut n)?;
        len = u16::from_be_bytes(n) as usize;
    }
    if len > 8192 || header[1] & 128 == 0 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut mask = [0u8; 4];
    stream.read_exact(&mut mask)?;
    let mut data = vec![0; len];
    stream.read_exact(&mut data)?;
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    Ok((header[0] & 15, data))
}
fn send_frame(stream: &mut TcpStream, value: serde_json::Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&value).unwrap();
    stream.write_all(&[0x81])?;
    if bytes.len() < 126 {
        stream.write_all(&[bytes.len() as u8])?;
    } else {
        stream.write_all(&[126])?;
        stream.write_all(&(bytes.len() as u16).to_be_bytes())?;
    }
    stream.write_all(&bytes)
}
fn serve(mut stream: TcpStream, stop: Arc<AtomicBool>, sockets: Arc<AtomicUsize>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|v| v == b"\r\n\r\n") {
        let Ok(n) = stream.read(&mut buf) else {
            return;
        };
        if n == 0 || request.len() > 16384 {
            return;
        }
        request.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&request);
    let key = text
        .lines()
        .filter_map(|s| s.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key"))
        .map(|(_, v)| v.trim());
    if let Some(key) = key {
        sockets.fetch_add(1, Ordering::Relaxed);
        let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha1).unwrap();
        checksum.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes());
        let accept = glib::base64_encode(&checksum.digest());
        if stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).is_err() { return; }
        let Ok((1, open)) = read_frame(&mut stream) else {
            return;
        };
        let frame: serde_json::Value = serde_json::from_slice(&open).unwrap();
        assert_eq!(frame["endpoint"], "$events");
        assert_eq!(frame["type"], "open");
        let id = frame["streamId"].as_str().unwrap();
        let ready = serde_json::json!({"type":"item","streamId":id,"value":{"type":"ready","clientId":"synthetic-private-generation","host":{"home":"/synthetic-private-home"}}});
        let _ = send_frame(&mut stream, ready);
        for running in [false, true, false, false] {
            let _ = send_frame(
                &mut stream,
                serde_json::json!({"type":"item","streamId":id,"value":{"type":"emit","event":"api-session/status","args":["synthetic-private-agent",running]}}),
            );
        }
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        while !stop.load(Ordering::Relaxed) {
            match read_frame(&mut stream) {
                Ok((8, _)) => {
                    let _ = stream.write_all(&[0x88, 2, 0x03, 0xe8]);
                    break;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
                _ => {}
            }
        }
    } else {
        let body = r#"<!doctype html><html><body><script>
window.fixtureFrames=0;
const socket=new WebSocket('ws://'+location.host+'/prefix/api/remote.mux?token=synthetic-private-query');
socket.addEventListener('message',()=>{window.fixtureFrames++});
socket.addEventListener('open',()=>socket.send(JSON.stringify({type:'open',streamId:'fixture-stream',endpoint:'$events',payload:{args:{}}})));
</script></body></html>"#;
        let _=stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes());
    }
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let sockets = Arc::new(AtomicUsize::new(0));
        let count = sockets.clone();
        let joins = std::thread::spawn(move || {
            let mut children = Vec::new();
            while !flag.load(Ordering::Relaxed) {
                if let Ok((stream, _)) = listener.accept() {
                    let stop = flag.clone();
                    let sockets = count.clone();
                    children.push(std::thread::spawn(move || serve(stream, stop, sockets)));
                } else {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            for child in children {
                child.join().unwrap();
            }
        });
        Self {
            base,
            stop,
            joins: Some(joins),
            sockets,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.joins.take() {
            join.join().unwrap();
        }
    }
}
fn tick() {
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
}
fn wait(test: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !test() {
        tick();
        assert!(Instant::now() < end, "task bridge fixture timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    tick();
}
fn js(view: &WebView, source: &str) -> bool {
    js_world(view, source, None)
}
fn js_world(view: &WebView, source: &str, world: Option<&str>) -> bool {
    let result = Rc::new(Cell::new(None));
    let copy = result.clone();
    view.evaluate_javascript(
        source,
        world,
        None,
        None::<&gio::Cancellable>,
        move |value| copy.set(Some(value.is_ok_and(|v| v.is_boolean() && v.to_boolean()))),
    );
    wait(|| result.get().is_some());
    result.get().unwrap()
}
fn load(view: &WebView, uri: &Url, loaded: &Cell<bool>) {
    loaded.set(false);
    view.load_uri(uri.as_str());
    wait(|| loaded.get());
    let end = Instant::now() + Duration::from_secs(10);
    while !js(view, "window.fixtureFrames === 5") {
        assert!(Instant::now() < end, "task frames missing");
        tick();
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn only_current_http_origin_is_a_task_document() {
    let base = Url::parse("http://127.0.0.1:3080/").unwrap();
    assert!(task_notifications::trusted_document(&base, &base));
    for uri in [
        "http://127.0.0.1:3081/",
        "http://localhost:3080/",
        "http://user:pass@127.0.0.1:3080/",
        "blob:http://127.0.0.1:3080/opaque",
        "file:///tmp/data",
        "tauri://localhost/",
        "data:text/html,private",
    ] {
        assert!(!task_notifications::trusted_document(
            &Url::parse(uri).unwrap(),
            &base
        ));
    }
}
#[test]
#[ignore = "requires a GTK display and private fake D-Bus session"]
fn websocket_status_reaches_desktop_without_exposing_ids_or_content() {
    bus::assert_private();
    gtk::init().unwrap();
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
    let received = Arc::new(Mutex::new(bus::Received::default()));
    let registration = bus::register(&connection, received.clone());
    bus::request_name(&connection);
    let client = Arc::new(Client::default());
    client.set_enabled(true);
    let server = Server::new();
    let foreign = Server::new();
    let context = WebContext::new_ephemeral();
    let view = WebView::with_context(&context);
    let window = if std::env::var_os("DSH_OCEAN_TEST_MAP_WEBVIEW").is_some() {
        let w = gtk::Window::new(gtk::WindowType::Toplevel);
        w.set_default_size(640, 480);
        w.add(&view);
        w.show_all();
        Some(w)
    } else {
        None
    };
    let signals = Rc::new(Cell::new(0));
    let counter = signals.clone();
    let sender = client.clone();
    let expected = server.base.clone();
    task_notifications::attach(
        &view,
        Rc::new(move || Some(expected.clone())),
        Rc::new(move || {
            counter.set(counter.get() + 1);
            sender.post(Kind::Task);
        }),
    )
    .unwrap();
    let loaded = Rc::new(Cell::new(false));
    let flag = loaded.clone();
    view.connect_load_changed(move |_, event| {
        if event == webkit2gtk::LoadEvent::Finished {
            flag.set(true)
        }
    });
    load(&view, &server.base, &loaded);
    wait(|| signals.get() == 1 && received.lock().unwrap().packets.len() == 1);
    assert!(
        js(
            &view,
            "typeof window.webkit?.messageHandlers?.oceanTaskActivity === 'undefined'"
        ),
        "no native handler in the page world"
    );
    assert!(js_world(&view, "window.webkit.messageHandlers.oceanTaskActivity.postMessage('synthetic-private-message'); window.webkit.messageHandlers.oceanTaskActivity.postMessage(2); window.webkit.messageHandlers.oceanTaskActivity.postMessage(1.5); true", Some("dev.dsh.ocean.task-notifications")));
    tick();
    assert_eq!(
        signals.get(),
        1,
        "only the constant numeric signal is accepted"
    );
    let packet = received.lock().unwrap().packets[0].print(true).to_string();
    assert!(!packet.contains("synthetic-private") && !packet.contains("127.0.0.1"));
    load(&view, &foreign.base, &loaded);
    assert_eq!(
        signals.get(),
        1,
        "reject a different origin before rate limiting"
    );
    client.set_enabled(false);
    wait(|| received.lock().unwrap().closed.len() == 1);
    load(&view, &server.base, &loaded);
    wait(|| signals.get() == 2);
    assert_eq!(
        received.lock().unwrap().packets.len(),
        1,
        "disabled client must not send"
    );
    assert_eq!(server.sockets.load(Ordering::Relaxed), 2);
    assert_eq!(foreign.sockets.load(Ordering::Relaxed), 1);
    if let Some(window) = window {
        window.close();
    }
    drop(view);
    drop(context);
    connection.unregister_object(registration).unwrap();
}
