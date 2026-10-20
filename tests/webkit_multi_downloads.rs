//! Real overlapping transfers, gated EOF and two ephemeral WebKit contexts.
//! Synthetic loopback auth only; injected chooser exists only in this test.
#![cfg(target_os = "linux")]
use dsh_ocean::{
    download_engine, download_history,
    downloads::{History, Status},
};
use gtk::{glib, prelude::*};
use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use webkit2gtk::{WebContext, WebView, WebViewExt};
const PREFIX: usize = 65536;
const TOTAL: usize = PREFIX * 2;
const COOKIE: &str = "ocean_multi=synthetic-cookie-never-store";
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.0.is_absolute()
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("dsh-ocean-multi-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Server {
    base: url::Url,
    stop: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.release.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn respond(mut stream: TcpStream, stop: Arc<AtomicBool>, release: Arc<AtomicBool>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
        let Ok(size) = stream.read(&mut buf) else {
            return;
        };
        if size == 0 || request.len() > 16384 {
            return;
        }
        request.extend_from_slice(&buf[..size]);
    }
    let raw = String::from_utf8_lossy(&request);
    let target = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");
    let path = target.split('?').next().unwrap_or("");
    if path == "/session" {
        let body = b"<html><body>private fixture</body></html>";
        let header=format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: {COOKIE}; HttpOnly; Path=/; SameSite=Lax\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(body);
        return;
    }
    if !raw
        .to_ascii_lowercase()
        .contains(&format!("cookie: {COOKIE}"))
    {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let (name, byte, gated) = match path {
        "/first" => ("first.bin", b'1', true),
        "/second" => ("second.bin", b'2', true),
        "/foreign" => ("foreign.bin", b'3', true),
        "/after" => ("after.bin", b'4', false),
        "/nested" => ("nested.bin", b'5', false),
        "/after-nested" => ("after-nested.bin", b'6', false),
        _ => {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
    };
    let header=format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename={name}\r\nContent-Length: {TOTAL}\r\nConnection: close\r\n\r\n");
    if stream.write_all(header.as_bytes()).is_err()
        || stream.write_all(&vec![byte; PREFIX]).is_err()
    {
        return;
    }
    while gated && !release.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(2));
    }
    if !stop.load(Ordering::Acquire) {
        let _ = stream.write_all(&vec![byte; PREFIX]);
    }
}
fn server() -> Server {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let gate = release.clone();
    let thread = std::thread::spawn(move || {
        let mut workers = Vec::new();
        while !stopping.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if workers.len() >= 32 {
                        continue;
                    }
                    let stop = stopping.clone();
                    let release = gate.clone();
                    workers.push(std::thread::spawn(move || respond(stream, stop, release)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(_) => break,
            }
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    Server {
        base,
        stop,
        release,
        thread: Some(thread),
    }
}
fn tick() {
    for _ in 0..64 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
}
fn wait(mut condition: impl FnMut() -> bool, phase: &str) {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        tick();
        if condition() {
            break;
        }
        assert!(Instant::now() < end, "multi-stream timeout during {phase}");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn load_session(view: &WebView, base: &url::Url) {
    let loaded = Rc::new(Cell::new(false));
    let flag = loaded.clone();
    let signal = view.connect_load_changed(move |_, event| {
        if event == webkit2gtk::LoadEvent::Finished {
            flag.set(true);
        }
    });
    view.load_uri(base.join("session").unwrap().as_str());
    wait(|| loaded.get(), "synthetic session load");
    view.disconnect(signal);
}
fn cancel_inside_real_chooser(
    root: &std::path::Path,
    source: &str,
    history: download_engine::SharedHistory,
    control: download_engine::Cancellation,
) -> Option<PathBuf> {
    eprintln!("native-multi: constructing real nested chooser");
    let dialog = dsh_ocean::download_chooser::build(None, "nested.bin", source, Some(root));
    let target = root.join("nested.bin");
    let requested = Rc::new(Cell::new(false));
    let fenced = Rc::new(Cell::new(false));
    let clicked = Rc::new(Cell::new(false));
    let popup = dialog.clone();
    let folder = root.to_owned();
    let queued = requested.clone();
    let fence = fenced.clone();
    let accepted = clicked.clone();
    let driver = glib::timeout_add_local(Duration::from_millis(20), move || {
        if !popup.is_mapped() || popup.current_folder().as_ref() != Some(&folder) {
            return glib::ControlFlow::Continue;
        }
        if !queued.get() {
            popup.set_current_name("nested.bin");
            let ids = {
                let records = history.lock().unwrap();
                records
                    .entries()
                    .iter()
                    .filter(|entry| entry.status == Status::Receiving)
                    .map(|entry| entry.id.clone())
                    .collect::<Vec<_>>()
            };
            if ids.len() == 1 && control.request(ids) {
                eprintln!("native-multi: nested cancellation queued");
                queued.set(true);
                let ready = fence.clone();
                // Cancellation uses DEFAULT priority. This lower-priority idle
                // fence proves its handler ran while dialog.run() is nested;
                // failed/finished emission itself may be deferred by WebKit.
                glib::idle_add_local_once(move || {
                    eprintln!("native-multi: cancellation priority fence reached");
                    ready.set(true);
                });
            }
            return glib::ControlFlow::Continue;
        }
        if fence.get() {
            if let Some(button) = popup
                .widget_for_response(gtk::ResponseType::Accept)
                .and_then(|widget| widget.downcast::<gtk::Button>().ok())
            {
                if button.is_sensitive() {
                    eprintln!("native-multi: clicking late Save after cancellation");
                    accepted.set(true);
                    button.emit_clicked();
                }
            }
        }
        glib::ControlFlow::Continue
    });
    let timed_out = Rc::new(Cell::new(false));
    let timeout = timed_out.clone();
    let popup = dialog.clone();
    let watchdog = glib::timeout_add_local(Duration::from_secs(8), move || {
        eprintln!("native-multi: nested chooser watchdog fired");
        timeout.set(true);
        popup.response(gtk::ResponseType::Cancel);
        glib::ControlFlow::Continue
    });
    eprintln!("native-multi: enter nested chooser run");
    let selected = dsh_ocean::download_chooser::run(&dialog);
    eprintln!("native-multi: nested chooser returned");
    driver.remove();
    watchdog.remove();
    assert!(
        !timed_out.get(),
        "nested chooser cancellation/late Accept timed out"
    );
    assert!(requested.get() && fenced.get() && clicked.get());
    assert_eq!(selected.as_ref(), Some(&target));
    selected
}
fn status(history: &download_engine::SharedHistory, name: &str) -> Option<Status> {
    history
        .lock()
        .unwrap()
        .entries()
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.status)
}
fn id(history: &download_engine::SharedHistory, name: &str) -> String {
    history
        .lock()
        .unwrap()
        .entries()
        .iter()
        .find(|entry| entry.name == name)
        .unwrap()
        .id
        .clone()
}
#[test]
#[ignore = "requires explicitly isolated GTK/WebKit virtual display"]
fn selected_cancellation_isolated_from_other_live_streams_and_contexts() {
    assert_eq!(std::env::var("DSH_OCEAN_TEST_DIALOGS").as_deref(), Ok("1"));
    assert!(std::env::var("XDG_RUNTIME_DIR")
        .unwrap()
        .starts_with("/tmp/dsh-ocean-native-test."));
    assert_eq!(
        std::env::var("DSH_OCEAN_TEST_MAP_WEBVIEW").as_deref(),
        Ok("1")
    );
    gtk::init().unwrap();
    let root = std::env::temp_dir().join(format!(
        "dsh-ocean-multi-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let f = Fixture(root);
    let server = server();
    fs::write(f.0.join("preexisting"), b"unrelated bytes survive").unwrap();
    let context_a = WebContext::new_ephemeral();
    let context_b = WebContext::new_ephemeral();
    let a = WebView::with_context(&context_a);
    let b = WebView::with_context(&context_b);
    let canvas = gtk::Box::new(gtk::Orientation::Vertical, 0);
    canvas.pack_start(&a, true, true, 0);
    canvas.pack_start(&b, true, true, 0);
    let parent = gtk::Window::new(gtk::WindowType::Toplevel);
    parent.set_title("Ocean multi-stream fixture");
    parent.set_default_size(640, 480);
    parent.add(&canvas);
    parent.show_all();
    let history_path = f.0.join("history.json");
    let history = Arc::new(Mutex::new(History::open(&history_path).unwrap()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let terminal: download_engine::Terminal =
        Arc::new(move |entry| observed.lock().unwrap().push((entry.id, entry.status)));
    let nested_control = Rc::new(RefCell::new(None::<download_engine::Cancellation>));
    let late_accept = Rc::new(Cell::new(false));
    let attach = |view: &WebView| {
        let root = f.0.clone();
        let origin = server.base.clone();
        let control = nested_control.clone();
        let records = history.clone();
        let accepted = late_accept.clone();
        download_engine::attach(
            view,
            Rc::new(move || Some(origin.clone())),
            Rc::new(move |name, source| {
                if name == "nested.bin" {
                    let cancellation = control.borrow().as_ref().unwrap().clone();
                    let selected =
                        cancel_inside_real_chooser(&root, source, records.clone(), cancellation);
                    accepted.set(selected.is_some());
                    return selected;
                }
                assert!([
                    "first.bin",
                    "second.bin",
                    "foreign.bin",
                    "after.bin",
                    "after-nested.bin"
                ]
                .contains(&name));
                Some(root.join(name))
            }),
            history.clone(),
            terminal.clone(),
        )
    };
    let control_a = attach(&a);
    *nested_control.borrow_mut() = Some(control_a.clone());
    let control_b = attach(&b);
    load_session(&a, &server.base);
    load_session(&b, &server.base);
    let mut handles = vec![
        a.download_uri(
            server
                .base
                .join("first?token=synthetic-private-query")
                .unwrap()
                .as_str(),
        )
        .unwrap(),
        a.download_uri(server.base.join("second").unwrap().as_str())
            .unwrap(),
        b.download_uri(server.base.join("foreign").unwrap().as_str())
            .unwrap(),
    ];
    wait(
        || {
            let records = history.lock().unwrap();
            records.entries().len() == 3
                && records.entries().iter().all(|entry| {
                    entry.status == Status::Receiving
                        && entry.received_bytes > 0
                        && entry.reported_total == Some(TOTAL as u64)
                })
        },
        "three overlapping partial transfers",
    );
    assert!(!server.release.load(Ordering::Acquire));
    let first = id(&history, "first.bin");
    let second = id(&history, "second.bin");
    let foreign = id(&history, "foreign.bin");
    // Exact IDs belonging to a different attach/context are non-authoritative.
    assert!(control_a.request(vec![foreign.clone()]));
    assert!(control_b.request(vec![second.clone()]));
    tick();
    let cancel = control_a.clone();
    let window = download_history::build_with_cancel(
        Some(&parent),
        history.clone(),
        Some(Rc::new(move |ids| cancel.request(ids))),
    );
    window.dialog.show_all();
    assert!(window.selection.active_id().is_none());
    assert!(window.selection.set_active_id(Some(&first)));
    assert!(window.cancel_selected.is_sensitive());
    window.cancel_selected.emit_clicked();
    wait(
        || status(&history, "first.bin") == Some(Status::Cancelled),
        "one selected cancellation",
    );
    assert_eq!(status(&history, "second.bin"), Some(Status::Receiving));
    assert_eq!(status(&history, "foreign.bin"), Some(Status::Receiving));
    assert!(
        !f.0.join("first.bin").exists()
            && !f.0.join("second.bin").exists()
            && !f.0.join("foreign.bin").exists()
    );
    wait(
        || window.selection.active_id().is_none(),
        "selection clears without choosing another live row",
    );
    assert!(!window.cancel_selected.is_sensitive());
    window
        .dialog
        .widget_for_response(gtk::ResponseType::Close)
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap()
        .emit_clicked();
    server.release.store(true, Ordering::Release);
    wait(
        || {
            status(&history, "second.bin") == Some(Status::Saved)
                && status(&history, "foreign.bin") == Some(Status::Saved)
        },
        "nonselected and foreign saves",
    );
    assert_eq!(fs::read(f.0.join("second.bin")).unwrap(), vec![b'2'; TOTAL]);
    assert_eq!(
        fs::read(f.0.join("foreign.bin")).unwrap(),
        vec![b'3'; TOTAL]
    );
    // A stale selected ID must not acquire authority over a future operation.
    assert!(control_a.request(vec![first]));
    handles.push(
        a.download_uri(server.base.join("after").unwrap().as_str())
            .unwrap(),
    );
    wait(
        || status(&history, "after.bin") == Some(Status::Saved),
        "later same-context save",
    );
    assert_eq!(fs::read(f.0.join("after.bin")).unwrap(), vec![b'4'; TOTAL]);
    handles.push(
        a.download_uri(
            server
                .base
                .join("nested?token=synthetic-private-query")
                .unwrap()
                .as_str(),
        )
        .unwrap(),
    );
    wait(
        || status(&history, "nested.bin") == Some(Status::Cancelled) && late_accept.get(),
        "cancel inside actual Save chooser, then late Accept",
    );
    assert!(!f.0.join("nested.bin").exists());
    {
        let records = history.lock().unwrap();
        let entry = records
            .entries()
            .iter()
            .find(|entry| entry.name == "nested.bin")
            .unwrap();
        assert!(
            entry.destination.is_none() && entry.recovery.is_none(),
            "late chooser selection must not start staging"
        );
    }
    handles.push(
        a.download_uri(server.base.join("after-nested").unwrap().as_str())
            .unwrap(),
    );
    wait(
        || status(&history, "after-nested.bin") == Some(Status::Saved),
        "successful transfer after nested chooser cancellation",
    );
    assert_eq!(
        fs::read(f.0.join("after-nested.bin")).unwrap(),
        vec![b'6'; TOTAL]
    );
    wait(|| events.lock().unwrap().len() == 6, "terminal callbacks");
    {
        let records = history.lock().unwrap();
        let observed = events.lock().unwrap();
        for entry in records.entries() {
            assert_eq!(
                observed
                    .iter()
                    .filter(|(id, _)| id == &entry.id)
                    .map(|(_, status)| *status)
                    .collect::<Vec<_>>(),
                vec![entry.status]
            );
        }
    }
    for name in ["second.bin", "foreign.bin", "after.bin", "after-nested.bin"] {
        assert_eq!(
            fs::metadata(f.0.join(name)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        fs::read(f.0.join("preexisting")).unwrap(),
        b"unrelated bytes survive"
    );
    assert!(!fs::read_dir(&f.0).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".dsh-ocean-download-")));
    let raw = fs::read_to_string(history_path).unwrap();
    assert!(!raw.contains("synthetic-cookie") && !raw.contains("synthetic-private-query"));
    parent.close();
    drop(handles);
}
